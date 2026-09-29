#![cfg(target_os = "linux")]
use roblox_toolchain::bundle;
use serde_json::{Value, json};
use std::{fs, path::Path};

fn write(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn fixture(root: &Path) -> std::path::PathBuf {
    for (name, value) in [("one", 20), ("two", 40)] {
        image::RgbaImage::from_pixel(2, 2, image::Rgba([value, 0, 0, 255]))
            .save(root.join(format!("{name}.png")))
            .unwrap();
    }
    let assets: Vec<_> = ["one", "two"]
        .into_iter()
        .map(|name| {
            json!({
                "id":name,"conversion":{"kind":"texture","source":format!("{name}.png"),
                "config":{"operation":"color","maxWidth":2,"maxHeight":2,"maxDecodedBytes":4096}}
            })
        })
        .collect();
    let path = root.join("build.json");
    write(
        &path,
        &json!({"assets":assets,"scenes":[],"cache":{"directory":"cache"}}),
    );
    path
}

fn report(root: &Path, source: &Path, name: &str) -> Value {
    bundle::build(source, &root.join(name)).unwrap();
    read(&root.join(name).join("cache-report.json"))
}

#[test]
fn reuses_unchanged_assets_and_invalidates_only_changed_sources_or_configuration() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let source = fixture(root);
    assert_eq!(
        report(root, &source, "first"),
        json!({"hits":[],"misses":["one","two"]})
    );
    assert_eq!(
        report(root, &source, "second"),
        json!({"hits":["one","two"],"misses":[]})
    );
    assert_eq!(
        read(&root.join("first/manifest.json")),
        read(&root.join("second/manifest.json"))
    );

    image::RgbaImage::from_pixel(2, 2, image::Rgba([80, 0, 0, 255]))
        .save(root.join("one.png"))
        .unwrap();
    assert_eq!(
        report(root, &source, "third"),
        json!({"hits":["two"],"misses":["one"]})
    );
    let mut plan = read(&source);
    plan["assets"][1]["conversion"]["config"]["maxWidth"] = json!(3);
    write(&source, &plan);
    assert_eq!(
        report(root, &source, "fourth"),
        json!({"hits":["one"],"misses":["two"]})
    );
    fs::write(root.join("unrelated.txt"), b"unrelated").unwrap();
    assert_eq!(
        report(root, &source, "fifth"),
        json!({"hits":["one","two"],"misses":[]})
    );
}

#[test]
fn corrupt_cache_fails_visibly_and_removes_only_the_new_bundle() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let source = fixture(root);
    report(root, &source, "first");
    let entry = fs::read_dir(root.join("cache"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(entry.join("files/color.png"), b"corrupted cached pixels").unwrap();
    let output = root.join("second");
    let error = bundle::build(&source, &output).err().unwrap().to_string();
    assert!(
        error.contains("cache output inventory/hash mismatch"),
        "{error}"
    );
    assert!(!output.exists());
    assert!(root.join("first/manifest.json").is_file());
    assert!(root.join("one.png").is_file());
    assert!(entry.is_dir());
}

#[test]
fn cache_copies_are_independent_and_missing_sources_cannot_hit() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let source = fixture(root);
    report(root, &source, "first");
    let manifest = read(&root.join("first/manifest.json"));
    let artifact = manifest["files"][0]["path"].as_str().unwrap();
    fs::write(root.join("first").join(artifact), b"edited output").unwrap();
    assert_eq!(
        report(root, &source, "second")["hits"],
        json!(["one", "two"])
    );
    roblox_toolchain::bundle_verify::verify(&root.join("second")).unwrap();
    fs::remove_file(root.join("one.png")).unwrap();
    assert!(bundle::build(&source, &root.join("third")).is_err());
    assert!(!root.join("third").exists());
}

#[test]
fn cached_skin_and_animation_restore_scene_bindings_and_follow_dependency_keys() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    fs::copy("tests/fixtures/rigged-simple.glb", root.join("rig.glb")).unwrap();
    let mut plan = json!({"cache":{"directory":"cache"},"assets":[
        {"id":"motion","conversion":{"kind":"animationGltf","source":"rig.glb","bindTo":"skin",
        "config":{"animationIndex":0,"rootNode":3,"metresPerStud":0.5,"rigidTolerance":0.0001,"name":"Motion","looped":true,"priority":"Action"}}},
        {"id":"skin","conversion":{"kind":"skin","source":"rig.glb",
        "config":{"meshNode":2,"metresPerStud":0.5,"cullDistanceMetres":20,"rigidTolerance":0.0001}}}
    ],"scenes":[]});
    let source = root.join("build.json");
    write(&source, &plan);
    assert_eq!(
        report(root, &source, "first"),
        json!({"hits":[],"misses":["skin","motion"]})
    );
    let manifest = read(&root.join("first/manifest.json"));
    let mesh = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["asset"] == "skin" && file["file"].as_str().unwrap().ends_with(".mesh"))
        .unwrap();
    let parsed = gltf::Gltf::open(root.join("rig.glb")).unwrap();
    let bone = parsed.nodes().nth(3).unwrap().name().unwrap();
    let scene = json!({"kind":"model","animationBindings":[{
        "animation":{"asset":"motion","file":"animation.rbxm"},"root":"mesh","bone":bone,"rigidTolerance":0.0001
    }],"roots":[{
        "id":"mesh","class":"MeshPart","name":"Mesh","properties":{},"references":{},"children":[],
        "assets":{"MeshContent":{"asset":"skin","file":mesh["file"]}},"rig":{"asset":"skin","file":"rig.rbxm"}
    }]});
    write(&root.join("scene.json"), &scene);
    plan["scenes"] = json!([{"id":"rig","source":"scene.json"}]);
    write(&source, &plan);
    assert_eq!(
        report(root, &source, "second"),
        json!({"hits":["skin","motion"],"misses":[]})
    );
    roblox_toolchain::bundle_verify::verify(&root.join("second")).unwrap();

    plan["assets"][0]["conversion"]["config"]["name"] = json!("Changed motion");
    write(&source, &plan);
    assert_eq!(
        report(root, &source, "third"),
        json!({"hits":["skin"],"misses":["motion"]})
    );
    plan["assets"][1]["conversion"]["config"]["cullDistanceMetres"] = json!(30);
    write(&source, &plan);
    assert_eq!(
        report(root, &source, "fourth"),
        json!({"hits":[],"misses":["skin","motion"]})
    );
    roblox_toolchain::bundle_verify::verify(&root.join("fourth")).unwrap();
}

#[test]
fn material_maps_are_real_cache_dependencies() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let source = fixture(root);
    let material = json!({"name":"Paint","alphaMode":"Overlay","color":[1,1,1],"localUriPrefix":"rbxasset://paint/",
        "maps":[{"source":"one.png","config":{"operation":"color","maxWidth":2,"maxHeight":2,"maxDecodedBytes":4096}}]});
    write(&root.join("material.json"), &material);
    let mut plan = read(&source);
    plan["assets"][0] =
        json!({"id":"material","conversion":{"kind":"material","source":"material.json"}});
    write(&source, &plan);
    report(root, &source, "first");
    assert_eq!(
        report(root, &source, "second")["hits"],
        json!(["material", "two"])
    );
    image::RgbaImage::from_pixel(2, 2, image::Rgba([99, 0, 0, 255]))
        .save(root.join("one.png"))
        .unwrap();
    assert_eq!(
        report(root, &source, "third"),
        json!({"hits":["two"],"misses":["material"]})
    );
    roblox_toolchain::bundle_verify::verify(&root.join("third")).unwrap();
}

#[test]
fn cli_reuses_assets_but_rechecks_changed_scripts_without_credentials() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let source = fixture(root);
    let mut plan = read(&source);
    plan["scenes"] = json!([{"id":"scripts","source":"scene.json"}]);
    write(&source, &plan);
    write(
        &root.join("scene.json"),
        &json!({"kind":"model",
            "scriptCompiler":{"optimizationLevel":1,"debugLevel":1,"typeInfoLevel":0,"coverageLevel":0},
            "roots":[{"id":"module","class":"ModuleScript","name":"Module","properties":{},"references":{},"children":[],"scriptSource":"module.luau"}]
        }),
    );
    fs::write(root.join("module.luau"), "return 1").unwrap();
    let invoke = |name: &str| {
        std::process::Command::new(env!("CARGO_BIN_EXE_roblox"))
            .env_clear()
            .args(["build", "bundle"])
            .arg(&source)
            .arg("--output")
            .arg(root.join(name))
            .output()
            .unwrap()
    };
    for name in ["first", "second"] {
        let result = invoke(name);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let response: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(response["scope"], "offlineBundleBuild");
    }
    assert_eq!(
        read(&root.join("second/cache-report.json"))["hits"],
        json!(["one", "two"])
    );
    fs::write(root.join("module.luau"), "return 2").unwrap();
    assert!(invoke("changed").status.success());
    assert_eq!(
        read(&root.join("changed/cache-report.json"))["hits"],
        json!(["one", "two"])
    );
    assert_ne!(
        read(&root.join("first/manifest.json"))["scenes"],
        read(&root.join("changed/manifest.json"))["scenes"]
    );
    fs::write(root.join("module.luau"), "local =").unwrap();
    assert!(!invoke("invalid").status.success());
    assert!(!root.join("invalid").exists());
    assert!(root.join("first/manifest.json").exists());
}
