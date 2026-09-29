use roblox_toolchain::{bundle, terrain};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

fn write(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn config() -> Value {
    json!({"metresPerStud":0.5,"voxelSizeMetres":2,"originMetres":[0,0,0],"alignmentToleranceMetres":0.00001,
        "chunkExponent":1,"limits":{"maxChunks":4,"maxCells":32},"materials":{"ground":"Grass"}})
}

#[test]
fn sparse_terrain_cli_emits_native_payload_without_overwriting_sources() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let source = root.join("terrain.json");
    write(
        &source,
        &json!({"kind":"voxels","config":config(),"voxels":[{"position":[-1,0,0],"material":"ground","occupancy":0.5}]}),
    );
    let original = fs::read(&source).unwrap();
    let output = root.join("output");
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_roblox"))
            .env_clear()
            .args(["convert", "terrain"])
            .arg(&source)
            .arg("--output")
            .arg(&output)
            .output()
            .unwrap()
    };
    let result = invoke();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let response: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(response["result"]["physicsGenerated"], false);
    assert_eq!(response["result"]["engineVerified"], false);
    assert_eq!(response["result"]["chunks"], 1);
    assert!(output.join("terrain.smoothgrid").is_file());
    assert!(!invoke().status.success());
    assert_eq!(fs::read(source).unwrap(), original);
}

#[test]
fn heightmap_bundle_embeds_native_grid_and_tracks_image_changes() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let image = root.join("height.png");
    image::GrayImage::from_raw(2, 1, vec![128, 255])
        .unwrap()
        .save(&image)
        .unwrap();
    let source = root.join("terrain.json");
    write(
        &source,
        &json!({"kind":"heightmap","source":"height.png","physics":{"maxEntries":64},"config":{
            "terrain":config(),"material":"ground","pixelSizeMetres":2,"floorMetres":0,"heightMinMetres":0,"heightMaxMetres":2,
            "rowDirection":"positive","maxWidth":2,"maxHeight":1,"maxDecodedBytes":4096
        }}),
    );
    let standalone = root.join("standalone");
    let converted = terrain::convert(&source, &standalone).unwrap();
    assert!(converted.heightmap_sha256.is_some());
    assert!(converted.physics_generated);
    assert_eq!(converted.physics_kind, Some("native-lazy-spatial-index"));
    assert!(!converted.engine_verified);
    let scene = json!({"kind":"place","roots":[{
        "id":"world","class":"Workspace","name":"Workspace","properties":{},"references":{},"children":[{
            "id":"terrain","class":"Terrain","name":"Terrain","properties":{},"references":{},"children":[],
            "assets":{"SmoothGrid":{"asset":"land","file":"terrain.smoothgrid"},"PhysicsGrid":{"asset":"land","file":"terrain.physicsgrid"}}
        }]
    }]});
    write(&root.join("scene.json"), &scene);
    let mut plan = json!({"assets":[{"id":"land","conversion":{"kind":"terrain","source":"terrain.json"}}],"scenes":[{"id":"map","source":"scene.json"}]});
    if cfg!(target_os = "linux") {
        plan["cache"] = json!({"directory":"cache"});
    }
    let build = root.join("build.json");
    write(&build, &plan);
    let first = root.join("first");
    let manifest = bundle::build(&build, &first).unwrap();
    let dom =
        rbx_binary::from_reader(fs::File::open(first.join(&manifest.scenes[0].path)).unwrap())
            .unwrap();
    let instance = dom
        .descendants()
        .find(|node| node.class.as_str() == "Terrain")
        .unwrap();
    let rbx_dom_weak::types::Variant::BinaryString(value) =
        &instance.properties[&"SmoothGrid".into()]
    else {
        panic!("missing SmoothGrid");
    };
    let bytes: &[u8] = value.as_ref();
    assert_eq!(bytes, fs::read(standalone.join(converted.file)).unwrap());
    let rbx_dom_weak::types::Variant::BinaryString(physics) =
        &instance.properties[&"PhysicsGrid".into()]
    else {
        panic!("missing PhysicsGrid");
    };
    let physics_bytes: &[u8] = physics.as_ref();
    assert_eq!(
        physics_bytes,
        fs::read(standalone.join(converted.physics_file.unwrap())).unwrap()
    );
    let decoded = roblox_toolchain::terrain_physics::decode(
        physics_bytes,
        &roblox_toolchain::terrain_physics::Limits { max_entries: 64 },
    )
    .unwrap();
    assert!(!decoded.coordinate_groups[0].is_empty());
    roblox_toolchain::bundle_verify::verify(&first).unwrap();
    let second = root.join("second");
    bundle::build(&build, &second).unwrap();
    if cfg!(target_os = "linux") {
        let report: Value =
            serde_json::from_slice(&fs::read(second.join("cache-report.json")).unwrap()).unwrap();
        assert_eq!(report["hits"], json!(["land"]));
    }
    image::GrayImage::from_raw(2, 1, vec![255, 255])
        .unwrap()
        .save(&image)
        .unwrap();
    let third = root.join("third");
    let changed = bundle::build(&build, &third).unwrap();
    assert_ne!(changed.files[0].sha256, manifest.files[0].sha256);
    if cfg!(target_os = "linux") {
        let report: Value =
            serde_json::from_slice(&fs::read(third.join("cache-report.json")).unwrap()).unwrap();
        assert_eq!(report["misses"], json!(["land"]));
    }
    fs::remove_file(&image).unwrap();
    assert!(bundle::build(&build, &root.join("missing")).is_err());
    assert!(!root.join("missing").exists());
}
