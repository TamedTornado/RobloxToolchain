use roblox_toolchain::skin_import::{self, Config};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn config() -> Config {
    Config {
        mesh_node: 2,
        metres_per_stud: 0.5,
        cull_distance_metres: 20.,
        rigid_tolerance: 0.00001,
    }
}

#[test]
fn khronos_rigged_simple_imports_without_changing_the_external_fixture() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rigged-simple.glb");
    let original = fs::read(&source).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("native");
    let manifest = skin_import::convert(&source, &output, &config()).unwrap();
    assert_eq!(
        manifest
            .rig
            .iter()
            .map(|bone| bone.name.as_str())
            .collect::<Vec<_>>(),
        ["Bone", "Bone.001"]
    );
    assert_eq!(manifest.rig[1].parent, Some(0));
    assert_eq!(manifest.meshes[0].source_vertices, 160);
    assert_eq!(manifest.meshes[0].triangles, 188);
    let bytes = fs::read(output.join(&manifest.meshes[0].file)).unwrap();
    let rbx_mesh::mesh::Mesh::V4(decoded) =
        rbx_mesh::read_mesh_versioned(std::io::Cursor::new(bytes)).unwrap()
    else {
        panic!("not v4");
    };
    assert_eq!(decoded.vertices.len(), 160);
    assert_eq!(decoded.faces.len(), 188);
    assert_eq!(decoded.bones.len(), 2);
    assert!(decoded.vertices.iter().all(|v| v.tex == [0.; 2]));
    assert!(
        decoded
            .envelopes
            .iter()
            .all(|v| v.weights.iter().map(|w| u16::from(*w)).sum::<u16>() == 255)
    );
    assert_eq!(original, fs::read(source).unwrap());
}

#[test]
fn khronos_native_deformation_matches_source_inverse_bind_equations() {
    use glam::{Mat4, Vec3};
    use std::collections::BTreeMap;

    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rigged-simple.glb");
    let gltf = gltf::Gltf::from_slice(&fs::read(&source).unwrap()).unwrap();
    let data = |_: gltf::Buffer<'_>| gltf.blob.as_deref();
    let source_skin = gltf.skins().next().unwrap();
    let source_joints: Vec<_> = source_skin.joints().map(|node| node.index()).collect();
    let inverse_bind: Vec<_> = source_skin
        .reader(data)
        .read_inverse_bind_matrices()
        .unwrap()
        .map(|matrix| Mat4::from_cols_array_2d(&matrix))
        .collect();
    let primitive = gltf.meshes().next().unwrap().primitives().next().unwrap();
    let reader = primitive.reader(data);
    let positions: Vec<_> = reader
        .read_positions()
        .unwrap()
        .map(Vec3::from_array)
        .collect();
    let indices: Vec<_> = reader.read_indices().unwrap().into_u32().collect();
    let joints: Vec<_> = reader.read_joints(0).unwrap().into_u16().collect();
    let weights: Vec<_> = reader.read_weights(0).unwrap().into_f32().collect();

    let mut globals = BTreeMap::new();
    let mut pending: Vec<_> = gltf
        .default_scene()
        .unwrap()
        .nodes()
        .map(|node| (node, Mat4::IDENTITY))
        .collect();
    while let Some((node, parent)) = pending.pop() {
        let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        pending.extend(node.children().map(|child| (child, world)));
        globals.insert(node.index(), world);
    }
    // Exercise a non-rest pose as well as the imported fixture's coordinate
    // transforms. No production conversion helper is used for this oracle.
    let child = source_joints[1];
    globals.insert(child, globals[&child] * Mat4::from_rotation_x(0.7));
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("converted");
    let policy = config();
    let manifest = skin_import::convert(&source, &output, &policy).unwrap();
    let bytes = fs::read(output.join(&manifest.meshes[0].file)).unwrap();
    let rbx_mesh::mesh::Mesh::V4(native) =
        rbx_mesh::read_mesh_versioned(std::io::Cursor::new(bytes)).unwrap()
    else {
        panic!("not v4");
    };
    assert_eq!(native.subsets.len(), 1);
    let units = Mat4::from_scale(Vec3::splat(1. / policy.metres_per_stud));

    for (face, source_face) in native.faces.iter().zip(indices.chunks_exact(3)) {
        for (vertex, source_index) in face.0.iter().zip(source_face) {
            let source_index = *source_index as usize;
            let mut expected = Vec3::ZERO;
            let weight_sum: f32 = weights[source_index].iter().sum();
            for slot in 0..4 {
                let bone = usize::from(joints[source_index][slot]);
                let transform = globals[&source_joints[bone]] * inverse_bind[bone];
                expected += transform.transform_point3(positions[source_index])
                    * (weights[source_index][slot] / weight_sum);
            }
            expected /= policy.metres_per_stud;
            let vertex = vertex.0 as usize;
            let envelope = &native.envelopes[vertex];
            let position = Vec3::from_array(native.vertices[vertex].pos);
            let mut actual = Vec3::ZERO;
            for slot in 0..4 {
                if envelope.weights[slot] == 0 {
                    continue;
                }
                let bone_index = usize::from(
                    native.subsets[0].bones[usize::from(envelope.bones[slot])]
                        .get()
                        .unwrap(),
                );
                let bind = &native.bones[bone_index].cframe;
                let bind = Mat4::from_cols_array(&[
                    bind.r00, bind.r10, bind.r20, 0., bind.r01, bind.r11, bind.r21, 0., bind.r02,
                    bind.r12, bind.r22, 0., bind.x, bind.y, bind.z, 1.,
                ]);
                let pose = units * globals[&manifest.rig[bone_index].source_node] * units.inverse();
                actual += (pose * bind.inverse()).transform_point3(position)
                    * (f32::from(envelope.weights[slot]) / 255.);
            }
            // The target has byte weights, the source has float weights.
            assert!(
                actual.distance(expected) < 0.02,
                "native {actual:?}, source {expected:?}"
            );
        }
    }
}

fn fixture(root: &Path) -> Value {
    let mut binary = Vec::new();
    let geometry: &[f32] = &[
        0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 1., 0., 0., 1., 0., 0., 1., 0., 0.,
        1.,
    ];
    binary.extend(geometry.iter().flat_map(|v| v.to_le_bytes()));
    for _ in 0..3 {
        binary.extend([0u16, 1, 0, 0].iter().flat_map(|v| v.to_le_bytes()));
    }
    for _ in 0..3 {
        binary.extend([0.75f32, 0.25, 0., 0.].iter().flat_map(|v| v.to_le_bytes()));
    }
    // Skin list deliberately reverses hierarchy order: child, root. Child
    // inverse bind is T(-1,0,0), independent of the mesh-node's T(100,0,0).
    for x in [-1., 0.] {
        let matrix: [f32; 16] = [
            1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., x, 0., 0., 1.,
        ];
        binary.extend(matrix.iter().flat_map(|v| v.to_le_bytes()));
    }
    fs::write(root.join("skin.bin"), &binary).unwrap();
    json!({"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0,2]}],
        "nodes":[{"name":"Root","children":[1]}, {"name":"Child","translation":[1,0,0]},
            {"name":"Mesh","mesh":0,"skin":0,"translation":[100,0,0]}],
        "skins":[{"joints":[1,0],"inverseBindMatrices":5}],
        "meshes":[{"primitives":[{"attributes":{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2,"JOINTS_0":3,"WEIGHTS_0":4}}]}],
        "buffers":[{"uri":"skin.bin","byteLength":binary.len()}],
        "bufferViews":[
            {"buffer":0,"byteOffset":0,"byteLength":36}, {"buffer":0,"byteOffset":36,"byteLength":36},
            {"buffer":0,"byteOffset":72,"byteLength":24}, {"buffer":0,"byteOffset":96,"byteLength":24},
            {"buffer":0,"byteOffset":120,"byteLength":48}, {"buffer":0,"byteOffset":168,"byteLength":128}],
        "accessors":[
            {"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]},
            {"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"},
            {"bufferView":2,"componentType":5126,"count":3,"type":"VEC2"},
            {"bufferView":3,"componentType":5123,"count":3,"type":"VEC4"},
            {"bufferView":4,"componentType":5126,"count":3,"type":"VEC4"},
            {"bufferView":5,"componentType":5126,"count":2,"type":"MAT4"}]
    })
}

#[test]
fn source_skin_preserves_bind_space_units_joint_remapping_and_deformation() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let source = root.join("skin.gltf");
    fs::write(&source, serde_json::to_vec(&fixture(root)).unwrap()).unwrap();
    let output = root.join("converted");
    let manifest = skin_import::convert(&source, &output, &config()).unwrap();
    assert_eq!(manifest.rig[0].source_node, 0);
    assert_eq!(manifest.rig[1].source_node, 1);
    assert_eq!(manifest.rig[1].parent, Some(0));
    assert_eq!(manifest.rig[1].local_bind_cframe[9], 2.);
    let bytes = fs::read(output.join(&manifest.meshes[0].file)).unwrap();
    let rbx_mesh::mesh::Mesh::V4(decoded) =
        rbx_mesh::read_mesh_versioned(std::io::Cursor::new(&bytes)).unwrap()
    else {
        panic!("expected v4");
    };
    assert_eq!(decoded.bone_names, b"Root\0Child\0");
    assert_eq!(decoded.bones[1].cframe.x, 2.);
    assert_eq!(decoded.bones[1].parent.get(), Some(0));
    assert_eq!(decoded.bones[1].cull_distance, 40.);
    assert_eq!(decoded.vertices[0].pos, [0., 0., 0.]);
    assert_eq!(decoded.vertices[1].pos, [2., 0., 0.]);
    assert_eq!(decoded.envelopes[0].weights, [191, 64, 0, 0]);
    assert_eq!(decoded.envelopes[0].bones[..2], [1, 0]);

    // Independently evaluate the decoded envelope: move the child 1 metre
    // along Y. The expected 75% influence survives byte quantization.
    let envelope = &decoded.envelopes[0];
    let mut result_y = 0.;
    for slot in 0..4 {
        if envelope.weights[slot] == 0 {
            continue;
        }
        let bone = decoded.subsets[0].bones[usize::from(envelope.bones[slot])]
            .get()
            .unwrap();
        let offset = if bone == 1 { 2. } else { 0. };
        result_y += offset * f32::from(envelope.weights[slot]) / 255.;
    }
    assert!((result_y - 1.5).abs() <= 2. / 255.);
    let second = root.join("second");
    let repeated = skin_import::convert(&source, &second, &config()).unwrap();
    assert_eq!(
        serde_json::to_vec(&manifest).unwrap(),
        serde_json::to_vec(&repeated).unwrap()
    );
    assert_eq!(
        bytes,
        fs::read(second.join(&repeated.meshes[0].file)).unwrap()
    );
    assert!(skin_import::convert(&source, &output, &config()).is_err());
}

#[test]
fn invalid_weights_binds_and_unsupported_attributes_never_create_outputs() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let source = root.join("skin.gltf");
    let base = fixture(root);
    let original = fs::read(root.join("skin.bin")).unwrap();
    fs::write(&source, serde_json::to_vec(&base).unwrap()).unwrap();
    for (offset, value, expected) in [(120, -1f32, "nonnegative"), (168, 2., "rigid transforms")] {
        let mut binary = original.clone();
        binary[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        fs::write(root.join("skin.bin"), binary).unwrap();
        let output = root.join("bad");
        let error = skin_import::convert(&source, &output, &config())
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains(expected), "{error}");
        assert!(!output.exists());
    }
    fs::write(root.join("skin.bin"), original).unwrap();
    let mut extra = base.clone();
    extra["meshes"][0]["primitives"][0]["attributes"]["JOINTS_1"] = json!(3);
    let mut morph = base.clone();
    morph["meshes"][0]["primitives"][0]["targets"] = json!([{"POSITION":0}]);
    let mut duplicate = base;
    duplicate["nodes"][1]["name"] = "Root".into();
    for value in [extra, morph, duplicate] {
        fs::write(&source, serde_json::to_vec(&value).unwrap()).unwrap();
        let output = root.join("bad");
        assert!(skin_import::convert(&source, &output, &config()).is_err());
        assert!(!output.exists());
    }
}

#[test]
fn skin_cli_and_bundle_use_the_same_local_native_output() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let source = root.join("skin.gltf");
    fs::write(&source, serde_json::to_vec(&fixture(root)).unwrap()).unwrap();
    let policy = root.join("config.json");
    fs::write(&policy, serde_json::to_vec(&config()).unwrap()).unwrap();
    let output = root.join("cli");
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_roblox"))
        .env_clear()
        .args(["convert", "skin"])
        .arg(&source)
        .arg("--config")
        .arg(&policy)
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert!(result.status.success(), "{:?}", result.stderr);
    let response: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(response["result"]["format"], "roblox-skinned-mesh-v4.01");
    let plan = json!({"assets":[{"id":"character","conversion":{"kind":"skin","source":"skin.gltf","config":config()}}],"scenes":[]});
    let plan_path = root.join("build.json");
    fs::write(&plan_path, serde_json::to_vec(&plan).unwrap()).unwrap();
    let bundle = root.join("bundle");
    let manifest = roblox_toolchain::bundle::build(&plan_path, &bundle).unwrap();
    assert_eq!(
        fs::read(output.join("node-2-primitive-0.mesh")).unwrap(),
        fs::read(bundle.join(&manifest.files[0].path)).unwrap()
    );
}
