use roblox_toolchain::{bundle, bundle_verify::verify};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::Path, process::Command};

fn fixture(root: &Path) -> std::path::PathBuf {
    image::RgbaImage::from_pixel(2, 2, image::Rgba([20, 40, 60, 255]))
        .save(root.join("color.png"))
        .unwrap();
    let scene = json!({"kind":"model","roots":[{
        "id":"surface","class":"SurfaceAppearance","name":"Paint",
        "properties":{},"references":{},"children":[],
        "assets":{"ColorMapContent":{"asset":"paint","file":"color.png"}}
    },{
        "id":"sound","class":"Sound","name":"External",
        "properties":{"SoundId":{"ContentId":"rbxassetid://42"}},
        "references":{},"children":[]
    }]});
    write_json(&root.join("scene.json"), &scene);
    let plan = json!({"assets":[{
        "id":"paint","conversion":{"kind":"texture","source":"color.png",
        "config":{"operation":"color","maxWidth":2,"maxHeight":2,"maxDecodedBytes":4096}}
    }],"scenes":[{"id":"model","source":"scene.json"}]});
    write_json(&root.join("build.json"), &plan);
    let output = root.join("out");
    bundle::build(&root.join("build.json"), &output).unwrap();
    output
}

fn write_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn manifest(output: &Path) -> Value {
    serde_json::from_slice(&fs::read(output.join("manifest.json")).unwrap()).unwrap()
}

#[test]
fn verifies_real_bundle_read_only_and_reports_external_dependencies() {
    let temporary = tempfile::tempdir().unwrap();
    let output = fixture(temporary.path());
    let before = fs::read(output.join("manifest.json")).unwrap();
    let result = verify(&output).unwrap();
    assert_eq!(result.artifacts_verified, 2);
    assert_eq!(result.native_models_verified, 1);
    assert_eq!(result.owned_references_verified, 1);
    assert_eq!(
        result.external_references.into_iter().collect::<Vec<_>>(),
        ["rbxassetid://42"]
    );
    assert!(!result.engine_verified);
    assert_eq!(
        result.manifest_sha256,
        format!("{:x}", Sha256::digest(&before))
    );

    let cli = Command::new(env!("CARGO_BIN_EXE_roblox"))
        .env_clear()
        .arg("verify-bundle")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let response: Value = serde_json::from_slice(&cli.stdout).unwrap();
    assert_eq!(response["result"]["artifactsVerified"], 2);
    assert_eq!(before, fs::read(output.join("manifest.json")).unwrap());
}

#[test]
fn rejects_manifest_inconsistencies() {
    let temporary = tempfile::tempdir().unwrap();
    let output = fixture(temporary.path());
    let original = manifest(&output);
    let cases: Vec<(&str, Value)> = vec![("version", json!(2)), ("format", json!("different"))];
    for (key, value) in cases {
        let mut changed = original.clone();
        changed[key] = value;
        write_json(&output.join("manifest.json"), &changed);
        assert!(verify(&output).is_err(), "accepted {key}");
    }
    for (key, value) in [
        ("sha256", json!("incorrect")),
        ("localUri", json!("rbxasset://different")),
        ("asset", json!("")),
    ] {
        let mut changed = original.clone();
        changed["files"][0][key] = value;
        write_json(&output.join("manifest.json"), &changed);
        assert!(verify(&output).is_err(), "accepted {key}");
    }
    let mut duplicate = original.clone();
    duplicate["files"]
        .as_array_mut()
        .unwrap()
        .push(original["files"][0].clone());
    write_json(&output.join("manifest.json"), &duplicate);
    assert!(
        verify(&output)
            .unwrap_err()
            .to_string()
            .contains("identity")
    );
    duplicate["files"][1]["asset"] = json!("another");
    write_json(&output.join("manifest.json"), &duplicate);
    assert!(
        verify(&output)
            .unwrap_err()
            .to_string()
            .contains("duplicate bundle artifact path")
    );
}

#[test]
fn rejects_unlisted_references_and_corrupt_native_containers_even_with_valid_hashes() {
    let temporary = tempfile::tempdir().unwrap();
    let output = fixture(temporary.path());
    let original = manifest(&output);
    let mut changed = original.clone();
    changed["files"] = json!([]);
    write_json(&output.join("manifest.json"), &changed);
    assert!(
        verify(&output)
            .unwrap_err()
            .to_string()
            .contains("unlisted local")
    );

    let mut changed = original;
    let path = changed["scenes"][0]["path"].as_str().unwrap();
    fs::write(output.join(path), b"not a native model").unwrap();
    changed["scenes"][0]["sha256"] = json!(format!("{:x}", Sha256::digest(b"not a native model")));
    write_json(&output.join("manifest.json"), &changed);
    assert!(verify(&output).is_err());
}

#[test]
fn rejects_missing_modified_and_escaping_artifacts() {
    let temporary = tempfile::tempdir().unwrap();
    let output = fixture(temporary.path());
    let original = manifest(&output);
    let path = original["files"][0]["path"].as_str().unwrap();
    let bytes = fs::read(output.join(path)).unwrap();
    fs::write(output.join(path), b"changed").unwrap();
    assert!(
        verify(&output)
            .unwrap_err()
            .to_string()
            .contains("hash mismatch")
    );
    fs::remove_file(output.join(path)).unwrap();
    assert!(verify(&output).is_err());
    fs::write(output.join(path), &bytes).unwrap();

    for path in [
        "../color.png",
        "/color.png",
        "assets/%2e%2e/color.png",
        "assets/../color.png",
    ] {
        let mut changed = original.clone();
        changed["files"][0]["path"] = json!(path);
        changed["files"][0]["localUri"] = json!(format!("rbxasset://{path}"));
        write_json(&output.join("manifest.json"), &changed);
        assert!(
            verify(&output)
                .unwrap_err()
                .to_string()
                .contains("invalid contained")
        );
    }
    #[cfg(unix)]
    {
        write_json(&output.join("manifest.json"), &original);
        fs::remove_file(output.join(path)).unwrap();
        std::os::unix::fs::symlink(temporary.path().join("color.png"), output.join(path)).unwrap();
        assert!(verify(&output).unwrap_err().to_string().contains("escapes"));
    }
}
