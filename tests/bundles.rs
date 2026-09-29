use rbx_dom_weak::types::Variant;
use roblox_toolchain::bundle::build;
use serde_json::json;
use std::{fs, path::Path};

fn inputs(root: &Path) -> std::path::PathBuf {
    fs::copy("tests/fixtures/doorway.fbx", root.join("doorway.fbx")).unwrap();
    image::RgbaImage::from_pixel(2, 2, image::Rgba([20, 40, 60, 255]))
        .save(root.join("color.png"))
        .unwrap();
    let plan = json!({"assets":[
        {"id":"walls/../logical-id","conversion":{"kind":"mesh","source":"doorway.fbx","config":{"metresPerStud":0.28},"collision":{"mode":"hull"}}},
        {"id":"paint","conversion":{"kind":"texture","source":"color.png","config":{"operation":"color","maxWidth":2,"maxHeight":2,"maxDecodedBytes":4096}}}
    ],"scenes":[{"id":"test-model","source":"scene.json"}]});
    let scene = json!({"kind":"model","roots":[{
        "id":"part","class":"MeshPart","name":"Wall","properties":{"Anchored":{"Bool":true}},"references":{},
        "assets":{
            "MeshContent":{"asset":"walls/../logical-id","file":"node-1-primitive-0.mesh"},
            "PhysicalConfigData":{"asset":"walls/../logical-id","file":"node-1-primitive-0.mesh.physics"}
        },
        "children":[{"id":"surface","class":"SurfaceAppearance","name":"Paint","properties":{},"references":{},"children":[],
            "assets":{"ColorMapContent":{"asset":"paint","file":"color.png"}}}]
    }]});
    fs::write(
        root.join("scene.json"),
        serde_json::to_vec_pretty(&scene).unwrap(),
    )
    .unwrap();
    let path = root.join("build.json");
    fs::write(&path, serde_json::to_vec_pretty(&plan).unwrap()).unwrap();
    path
}

#[test]
fn bundle_converts_and_links_native_assets_without_remote_ids() {
    let temporary = tempfile::tempdir().unwrap();
    let source = inputs(temporary.path());
    let output = temporary.path().join("out");
    let manifest = build(&source, &output).unwrap();
    assert!(!manifest.published);
    assert!(!manifest.engine_verified);
    let inputs: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("build-inputs.json")).unwrap()).unwrap();
    assert!(inputs["assets"]["paint"]["color.png"].is_string());
    assert!(inputs["scenes"]["test-model"]["scene.json"].is_string());
    assert_eq!(manifest.files.len(), 7);
    assert_eq!(manifest.scenes.len(), 1);
    let bytes = fs::read(output.join(&manifest.scenes[0].path)).unwrap();
    let dom = rbx_binary::from_reader(bytes.as_slice()).unwrap();
    let part = dom.get_by_ref(dom.root().children()[0]).unwrap();
    let expected = manifest
        .files
        .iter()
        .find(|f| f.file == "node-1-primitive-0.mesh")
        .unwrap();
    assert_eq!(
        part.properties.get(&"MeshContent".into()),
        Some(&Variant::Content(rbx_dom_weak::types::Content::from_uri(
            &expected.local_uri
        )))
    );
    let physics = match part.properties.get(&"PhysicalConfigData".into()).unwrap() {
        Variant::SharedString(value) => value.data(),
        Variant::NetAssetRef(value) => value.data(),
        other => panic!("unexpected native collision type {other:?}"),
    };
    let expected_physics = manifest
        .files
        .iter()
        .find(|f| f.file == "node-1-primitive-0.mesh.physics")
        .unwrap();
    assert_eq!(
        physics,
        fs::read(output.join(&expected_physics.path)).unwrap()
    );
    let surface = dom.get_by_ref(part.children()[0]).unwrap();
    let expected_color = manifest.files.iter().find(|f| f.asset == "paint").unwrap();
    assert_eq!(
        surface.properties.get(&"ColorMapContent".into()),
        Some(&Variant::Content(rbx_dom_weak::types::Content::from_uri(
            &expected_color.local_uri
        )))
    );
    for file in &manifest.files {
        assert!(output.join(&file.path).is_file());
    }

    let second = temporary.path().join("again");
    build(&source, &second).unwrap();
    assert_eq!(
        fs::read(output.join("manifest.json")).unwrap(),
        fs::read(second.join("manifest.json")).unwrap()
    );
    assert_eq!(
        bytes,
        fs::read(second.join(&manifest.scenes[0].path)).unwrap()
    );
}

#[test]
fn missing_asset_bindings_roll_back_owned_output_and_preserve_sources() {
    let temporary = tempfile::tempdir().unwrap();
    let source = inputs(temporary.path());
    let scene_path = temporary.path().join("scene.json");
    let mut scene: serde_json::Value =
        serde_json::from_slice(&fs::read(&scene_path).unwrap()).unwrap();
    scene["roots"][0]["assets"]["MeshContent"]["file"] = "absent.mesh".into();
    fs::write(scene_path, serde_json::to_vec(&scene).unwrap()).unwrap();
    let original = fs::read(temporary.path().join("doorway.fbx")).unwrap();
    let output = temporary.path().join("out");
    assert!(
        build(&source, &output)
            .err()
            .unwrap()
            .to_string()
            .contains("missing local asset binding")
    );
    assert!(!output.exists());
    assert_eq!(
        original,
        fs::read(temporary.path().join("doorway.fbx")).unwrap()
    );
}

#[test]
fn bundle_cli_needs_no_credentials_and_never_overwrites_a_completed_bundle() {
    let temporary = tempfile::tempdir().unwrap();
    let source = inputs(temporary.path());
    let out = temporary.path().join("out");
    let invoke = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_roblox"))
            .env_clear()
            .args(["build", "bundle"])
            .arg(&source)
            .arg("--output")
            .arg(&out)
            .output()
            .unwrap()
    };
    let result = invoke();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let response: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(response["scope"], "offlineBundleBuild");
    let bytes = fs::read(out.join("manifest.json")).unwrap();
    assert!(!invoke().status.success());
    assert_eq!(bytes, fs::read(out.join("manifest.json")).unwrap());
}

#[test]
fn material_bundle_relinks_every_native_map_to_the_actual_artifact_location() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    image::RgbaImage::from_pixel(1, 1, image::Rgba([20, 40, 60, 255]))
        .save(root.join("color.png"))
        .unwrap();
    let material = json!({"name":"Paint","alphaMode":"Transparency","color":[1,1,1],
        "localUriPrefix":"rbxasset://standalone/paint/",
        "maps":[{"source":"color.png","config":{"operation":"color","maxWidth":1,"maxHeight":1,"maxDecodedBytes":4096}}]});
    fs::write(root.join("material.json"), material.to_string()).unwrap();
    let plan = json!({"assets":[{"id":"paint","conversion":{"kind":"material","source":"material.json"}}],"scenes":[{"id":"model","source":"scene.json"}]});
    let scene = json!({"kind":"model","roots":[{"id":"part","class":"MeshPart","name":"Wall","properties":{},"references":{},"children":[],"material":{"asset":"paint","file":"material.rbxm"}}]});
    fs::write(root.join("scene.json"), scene.to_string()).unwrap();
    let source = root.join("build.json");
    fs::write(&source, plan.to_string()).unwrap();
    let output = root.join("out");
    let manifest = build(&source, &output).unwrap();
    assert_eq!(manifest.files.len(), 3);
    let model = manifest
        .files
        .iter()
        .find(|f| f.file == "material.rbxm")
        .unwrap();
    let texture = manifest
        .files
        .iter()
        .find(|f| f.file == "map-0/color.png")
        .unwrap();
    let bytes = fs::read(output.join(&model.path)).unwrap();
    let dom = rbx_binary::from_reader(bytes.as_slice()).unwrap();
    let appearance = dom.get_by_ref(dom.root().children()[0]).unwrap();
    assert_eq!(
        appearance.properties.get(&"ColorMapContent".into()),
        Some(&Variant::Content(rbx_dom_weak::types::Content::from_uri(
            &texture.local_uri
        )))
    );
    assert_eq!(texture.local_uri, format!("rbxasset://{}", texture.path));
    assert!(output.join(&texture.path).is_file());
    let scene_bytes = fs::read(output.join(&manifest.scenes[0].path)).unwrap();
    let assembled = rbx_binary::from_reader(scene_bytes.as_slice()).unwrap();
    let part = assembled
        .get_by_ref(assembled.root().children()[0])
        .unwrap();
    let attached = assembled.get_by_ref(part.children()[0]).unwrap();
    assert_eq!(attached.class, appearance.class);
    assert_eq!(attached.properties, appearance.properties);
    let repeated = root.join("repeated");
    build(&source, &repeated).unwrap();
    for file in &manifest.files {
        assert_eq!(
            fs::read(output.join(&file.path)).unwrap(),
            fs::read(repeated.join(&file.path)).unwrap()
        );
    }
    assert_eq!(
        fs::read(root.join("material.json")).unwrap(),
        material.to_string().as_bytes()
    );
    for (class, file, children) in [
        ("Part", "material.rbxm", json!([])),
        ("MeshPart", "map-0/color.png", json!([])),
        (
            "MeshPart",
            "material.rbxm",
            json!([{"id":"other","class":"SurfaceAppearance","name":"Other","properties":{},"references":{},"children":[]}]),
        ),
    ] {
        let mut invalid = scene.clone();
        invalid["roots"][0]["class"] = class.into();
        invalid["roots"][0]["material"]["file"] = file.into();
        invalid["roots"][0]["children"] = children;
        fs::write(root.join("scene.json"), invalid.to_string()).unwrap();
        let rejected = root.join("rejected");
        assert!(build(&source, &rejected).is_err());
        assert!(!rejected.exists());
    }
}
