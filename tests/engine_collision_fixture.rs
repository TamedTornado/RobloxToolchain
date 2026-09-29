use roblox_toolchain::{bundle, bundle_verify};
use std::{fs, path::Path};

#[test]
fn engine_acceptance_fixture_builds_offline_and_preserves_distinct_collision_recipes() {
    let temporary = tempfile::tempdir().unwrap();
    let output = temporary.path().join("bundle");
    let plan = Path::new("tests/fixtures/engine-collision/build.json");
    let manifest = bundle::build(plan, &output).unwrap();

    assert_eq!(manifest.scenes.len(), 1);
    assert!(!manifest.engine_verified);
    let read = |asset: &str, file: &str| {
        let artifact = manifest
            .files
            .iter()
            .find(|entry| entry.asset == asset && entry.file == file)
            .unwrap();
        fs::read(output.join(&artifact.path)).unwrap()
    };
    assert_eq!(
        read("engine-acceptance-hull", "node-1-primitive-0.mesh"),
        read("engine-acceptance-box", "node-1-primitive-0.mesh")
    );
    assert_ne!(
        read("engine-acceptance-hull", "node-1-primitive-0.mesh.physics"),
        read("engine-acceptance-box", "node-1-primitive-0.mesh.physics")
    );
    bundle_verify::verify(&output).unwrap();

    mlua::Compiler::new()
        .compile(include_str!("fixtures/engine-collision/validate.luau"))
        .unwrap();
}
