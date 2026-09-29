use rbx_dom_weak::InstanceBuilder;
use rbx_dom_weak::types::Variant;
use roblox_toolchain::{
    bundle, bundle_verify,
    deployment::{self, Binding, Mapping},
};
use sha2::{Digest, Sha256};
use std::{fs, path::Path, process::Command};

fn fixture(root: &Path) -> (std::path::PathBuf, Mapping) {
    let directory = root.join("bundle");
    let mut manifest = bundle::build(
        Path::new("tests/fixtures/engine-collision/build.json"),
        &directory,
    )
    .unwrap();
    let scene_path = directory.join(&manifest.scenes[0].path);
    let mut dom = rbx_binary::from_reader(fs::File::open(&scene_path).unwrap()).unwrap();
    let uri = manifest
        .files
        .iter()
        .find(|file| file.file.ends_with(".mesh"))
        .unwrap()
        .local_uri
        .clone();
    dom.insert(
        dom.root_ref(),
        InstanceBuilder::new("StringValue")
            .with_name("LiteralUri")
            .with_property("Value", uri.clone()),
    );
    dom.insert(
        dom.root_ref(),
        InstanceBuilder::new("Sound")
            .with_name("TypedUri")
            .with_property("SoundId", Variant::ContentId(uri.into())),
    );
    dom.insert(
        dom.root_ref(),
        InstanceBuilder::new("Sound")
            .with_name("BuiltIn")
            .with_property(
                "SoundId",
                Variant::ContentId("rbxasset://sounds/electronicpingshort.wav".into()),
            ),
    );
    let mut bytes = Vec::new();
    rbx_binary::to_writer(&mut bytes, &dom, dom.root().children()).unwrap();
    fs::write(&scene_path, &bytes).unwrap();
    manifest.scenes[0].sha256 = format!("{:x}", Sha256::digest(&bytes));
    fs::write(
        directory.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let verification = bundle_verify::verify(&directory).unwrap();
    let bindings = manifest
        .files
        .iter()
        .filter(|file| file.file.ends_with(".mesh"))
        .enumerate()
        .map(|(index, file)| Binding {
            asset: file.asset.clone(),
            file: file.file.clone(),
            source_artifact_sha256: file.sha256.clone(),
            remote_id: (100 + index).to_string(),
        })
        .collect();
    (
        directory,
        Mapping {
            bundle_manifest_sha256: verification.manifest_sha256,
            bindings,
        },
    )
}

#[test]
fn links_typed_mesh_ids_without_altering_embedded_physics_or_claiming_upload() {
    let temporary = tempfile::tempdir().unwrap();
    let (directory, mapping) = fixture(temporary.path());
    let output = temporary.path().join("linked.rbxl");
    let result =
        deployment::link_scene(&directory, "collision-acceptance", &mapping, &output).unwrap();
    assert_eq!(result.references_linked, 4);
    assert!(!result.published);
    assert!(!result.remote_ids_verified);
    assert!(!result.engine_verified);
    let manifest: bundle::Manifest =
        serde_json::from_slice(&fs::read(directory.join("manifest.json")).unwrap()).unwrap();
    let before =
        rbx_binary::from_reader(fs::File::open(directory.join(&manifest.scenes[0].path)).unwrap())
            .unwrap();
    let after = rbx_binary::from_reader(fs::File::open(&output).unwrap()).unwrap();
    for original in before.descendants().filter(|node| node.class == "MeshPart") {
        let linked = after
            .descendants()
            .find(|node| node.name == original.name)
            .unwrap();
        assert_eq!(
            original.properties[&"PhysicalConfigData".into()],
            linked.properties[&"PhysicalConfigData".into()]
        );
        assert!(
            matches!(&linked.properties[&"MeshContent".into()], Variant::Content(content) if content.as_uri().unwrap().starts_with("rbxassetid://"))
        );
    }
    for name in ["LiteralUri", "BuiltIn"] {
        let original = before.descendants().find(|node| node.name == name).unwrap();
        let linked = after.descendants().find(|node| node.name == name).unwrap();
        assert_eq!(original.properties, linked.properties);
    }
    let bytes = fs::read(&output).unwrap();
    assert!(deployment::link_scene(&directory, "collision-acceptance", &mapping, &output).is_err());
    assert_eq!(fs::read(output).unwrap(), bytes);
    assert_eq!(
        bundle_verify::verify(&directory).unwrap().manifest_sha256,
        mapping.bundle_manifest_sha256
    );
}

#[test]
fn rejects_stale_unknown_duplicate_incomplete_and_noncanonical_bindings() {
    let temporary = tempfile::tempdir().unwrap();
    let (directory, mut mapping) = fixture(temporary.path());
    let output = temporary.path().join("linked.rbxl");
    let check = |mapping: &Mapping| {
        assert!(
            deployment::link_scene(&directory, "collision-acceptance", mapping, &output).is_err()
        );
        assert!(!output.exists());
    };
    let hash = mapping.bundle_manifest_sha256.clone();
    mapping.bundle_manifest_sha256 = "stale".into();
    check(&mapping);
    mapping.bundle_manifest_sha256 = hash;
    let hash = mapping.bindings[0].source_artifact_sha256.clone();
    mapping.bindings[0].source_artifact_sha256 = "stale".into();
    check(&mapping);
    mapping.bindings[0].source_artifact_sha256 = hash;
    let asset = mapping.bindings[0].asset.clone();
    mapping.bindings[0].asset = "unknown".into();
    check(&mapping);
    mapping.bindings[0].asset = asset;
    for id in [
        "0",
        "01",
        "-1",
        "+1",
        "1.0",
        "rbxassetid://1",
        "18446744073709551616",
    ] {
        mapping.bindings[0].remote_id = id.into();
        check(&mapping);
    }
    mapping.bindings[0].remote_id = "100".into();
    let removed = mapping.bindings.pop().unwrap();
    check(&mapping);
    mapping.bindings.push(removed);
    let first = &mapping.bindings[0];
    mapping.bindings.push(Binding {
        asset: first.asset.clone(),
        file: first.file.clone(),
        source_artifact_sha256: first.source_artifact_sha256.clone(),
        remote_id: "123".into(),
    });
    check(&mapping);
}

#[test]
fn rejects_mutated_bundle_and_in_bundle_destination() {
    let temporary = tempfile::tempdir().unwrap();
    let (directory, mapping) = fixture(temporary.path());
    assert!(
        deployment::link_scene(
            &directory,
            "collision-acceptance",
            &mapping,
            &directory.join("linked.rbxl")
        )
        .is_err()
    );
    let manifest: bundle::Manifest =
        serde_json::from_slice(&fs::read(directory.join("manifest.json")).unwrap()).unwrap();
    fs::write(directory.join(&manifest.files[0].path), b"tampered").unwrap();
    assert!(
        deployment::link_scene(
            &directory,
            "collision-acceptance",
            &mapping,
            &temporary.path().join("linked.rbxl")
        )
        .is_err()
    );
}

#[test]
fn cli_reports_local_linking_without_network_or_credentials() {
    let temporary = tempfile::tempdir().unwrap();
    let (directory, mapping) = fixture(temporary.path());
    let map_path = temporary.path().join("mapping.json");
    let bindings: Vec<_> = mapping.bindings.iter().map(|binding| serde_json::json!({
        "asset":binding.asset,"file":binding.file,"sourceArtifactSha256":binding.source_artifact_sha256,"remoteId":binding.remote_id
    })).collect();
    fs::write(&map_path, serde_json::json!({"bundleManifestSha256":mapping.bundle_manifest_sha256,"bindings":bindings}).to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_roblox"))
        .env_clear()
        .args(["deploy", "link-scene"])
        .arg(directory)
        .args(["--scene", "collision-acceptance", "--mapping"])
        .arg(map_path)
        .arg("--output")
        .arg(temporary.path().join("linked.rbxl"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["scope"], "localDeploymentLinking");
    assert_eq!(response["result"]["remoteIdsVerified"], false);
    assert_eq!(response["result"]["published"], false);
}
