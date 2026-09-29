use roblox_toolchain::{bundle, plan_assets};
use serde_json::json;
use std::fs;

fn texture() -> serde_json::Value {
    json!({"kind":"texture","source":"source.png","config":{
        "operation":"color","maxWidth":1,"maxHeight":1,"maxDecodedBytes":4096
    }})
}

#[test]
fn edited_plan_builds_native_outputs_without_parallel_catalog_or_source_changes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let path = root.join("build.json");
    image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255]))
        .save(root.join("source.png"))
        .unwrap();
    let original = fs::read(root.join("source.png")).unwrap();
    let initial = plan_assets::initialize(&path).unwrap();
    assert!(plan_assets::initialize(&path).is_err());
    let revision = plan_assets::revision(&initial).unwrap();
    let added = plan_assets::put(&path, "paint", texture(), false, Some(&revision)).unwrap();
    assert_eq!(added["assets"][0]["id"], "paint");
    assert!(plan_assets::put(&path, "paint", texture(), false, None).is_err());
    assert!(plan_assets::put(&path, "paint", texture(), true, Some(&revision)).is_err());
    let before = fs::read(&path).unwrap();
    assert!(plan_assets::put(&path, "bad", json!({"kind":"unknown"}), false, None).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    let built = bundle::build(&path, &root.join("bundle")).unwrap();
    assert_eq!(built.files.len(), 1);
    plan_assets::put(&path, "paint", texture(), true, None).unwrap();
    plan_assets::remove(&path, "paint", None).unwrap();
    assert!(
        plan_assets::read(&path).unwrap()["assets"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(fs::read(root.join("source.png")).unwrap(), original);
    assert!(root.join("bundle/manifest.json").exists());
}

#[test]
fn scene_dependencies_block_deletion_without_mutating_plan() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let path = root.join("build.json");
    let scene = json!({"kind":"model","roots":[{"id":"part","class":"MeshPart","name":"Part",
        "properties":{},"references":{},"children":[],"material":{"asset":"paint","file":"material.rbxm"}}]});
    fs::write(root.join("scene.json"), scene.to_string()).unwrap();
    let plan = json!({"assets":[{"id":"paint","conversion":texture()}],"scenes":[{"id":"model","source":"scene.json"}]});
    fs::write(&path, plan.to_string()).unwrap();
    let before = fs::read(&path).unwrap();
    let error = plan_assets::remove(&path, "paint", None).err().unwrap();
    assert!(error.to_string().contains("still references"));
    assert_eq!(fs::read(&path).unwrap(), before);
    fs::remove_file(root.join("scene.json")).unwrap();
    assert!(plan_assets::remove(&path, "paint", None).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn concurrent_editors_preserve_all_assets_and_reject_legacy_documents() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("build.json");
    plan_assets::initialize(&path).unwrap();
    std::thread::scope(|scope| {
        for index in 0..8 {
            let path = &path;
            scope.spawn(move || {
                plan_assets::put(path, &format!("asset-{index}"), texture(), false, None).unwrap();
            });
        }
    });
    assert_eq!(
        plan_assets::read(&path).unwrap()["assets"]
            .as_array()
            .unwrap()
            .len(),
        8
    );
    fs::write(&path, r#"{"formatVersion":1,"config":{},"assets":{}}"#).unwrap();
    assert!(plan_assets::read(&path).is_err());
}

#[test]
fn deleting_skin_with_bound_animation_is_atomic_and_missing_sources_do_not_prevent_unreferenced_removal()
 {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("build.json");
    plan_assets::initialize(&path).unwrap();
    let skin = json!({"kind":"skin","source":"missing.glb","config":{
        "meshNode":0,"metresPerStud":0.28,"cullDistanceMetres":0,"rigidTolerance":0.001}});
    plan_assets::put(&path, "rig", skin, false, None).unwrap();
    let animation = json!({"kind":"animationGltf","source":"missing.glb","bindTo":"rig","config":{
        "animationIndex":0,"rootNode":0,"metresPerStud":0.28,"rigidTolerance":0.001,
        "name":"Walk","looped":true,"priority":"Movement"}});
    plan_assets::put(&path, "walk", animation, false, None).unwrap();
    let before = fs::read(&path).unwrap();
    let error = plan_assets::remove(&path, "rig", None).err().unwrap();
    assert!(error.to_string().contains("bindTo"));
    assert_eq!(fs::read(&path).unwrap(), before);
    plan_assets::remove(&path, "walk", None).unwrap();
    plan_assets::remove(&path, "rig", None).unwrap();
}
