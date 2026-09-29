use base64::Engine;
use roblox_toolchain::convert::{Config, convert};
use serde_json::Value;
use std::{fs, path::Path, process::Command};

fn u32_at(bytes: &[u8], offset: usize) -> usize {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize
}

fn f32_at(bytes: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn triangle_corners(bytes: &[u8]) -> Vec<[f32; 8]> {
    let face_start = 25 + u32_at(bytes, 17) * 40;
    (face_start..bytes.len())
        .step_by(4)
        .map(|offset| {
            let vertex = 25 + u32_at(bytes, offset) * 40;
            std::array::from_fn(|i| f32_at(bytes, vertex + i * 4))
        })
        .collect()
}

#[test]
fn fbx_and_glb_match_geometry_normals_and_uvs() {
    let temporary = tempfile::tempdir().unwrap();
    let config = Config {
        metres_per_stud: 0.28,
        obj_metres_per_unit: None,
        materials: None,
    };
    let glb_path = temporary.path().join("glb");
    let fbx_path = temporary.path().join("fbx");
    let glb = convert(Path::new("tests/fixtures/doorway.glb"), &glb_path, &config).unwrap();
    let fbx = convert(Path::new("tests/fixtures/doorway.fbx"), &fbx_path, &config).unwrap();
    assert_eq!(glb.meshes.len(), fbx.meshes.len());
    for mesh in &glb.meshes {
        let counterpart = fbx
            .meshes
            .iter()
            .find(|item| item.name == mesh.name)
            .unwrap();
        assert_eq!(mesh.triangles, counterpart.triangles);
        assert_eq!(mesh.base_color, counterpart.base_color);
        let a = triangle_corners(&fs::read(glb_path.join(&mesh.file)).unwrap());
        let b = triangle_corners(&fs::read(fbx_path.join(&counterpart.file)).unwrap());
        // Exporters may reorder/retriangulate faces; compare attributed corners.
        for corner in a {
            assert!(
                b.iter().any(|candidate| corner
                    .iter()
                    .zip(candidate)
                    .all(|(a, b)| (a - b).abs() < 0.0001)),
                "unmatched corner {corner:?}"
            );
        }
    }
}

#[test]
fn obj_requires_explicit_source_units_and_encodes_uv_convention() {
    let temporary = tempfile::tempdir().unwrap();
    let source = Path::new("tests/fixtures/triangle.obj");
    let output = temporary.path().join("obj");
    let mut config = Config {
        metres_per_stud: 0.5,
        obj_metres_per_unit: None,
        materials: None,
    };
    assert!(
        convert(source, &output, &config)
            .err()
            .unwrap()
            .to_string()
            .contains("objMetresPerUnit")
    );
    assert!(!output.exists());
    config.obj_metres_per_unit = Some(2.);
    let manifest = convert(source, &output, &config).unwrap();
    assert_eq!(manifest.meshes.len(), 1);
    let bytes = fs::read(output.join(&manifest.meshes[0].file)).unwrap();
    let corners = triangle_corners(&bytes);
    assert_eq!(corners.len(), 3);
    assert_eq!(corners[0], [0., 0., 0., 0., 0., 1., 0., 1.]);
    assert_eq!(corners[1], [4., 0., 0., 0., 0., 1., 1., 1.]);
    assert_eq!(corners[2], [0., 4., 0., 0., 0., 1., 0., 0.]);
}

#[test]
fn gltf_local_and_embedded_buffers_match_glb_and_reject_remote_or_escaped_paths() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("source");
    fs::create_dir(&root).unwrap();
    let glb = fs::read("tests/fixtures/doorway.glb").unwrap();
    let json_length = u32_at(&glb, 12);
    let mut document: Value = serde_json::from_slice(&glb[20..20 + json_length]).unwrap();
    let binary = &glb[28 + json_length..];
    let source = root.join("model.gltf");
    fs::write(root.join("mesh data.bin"), binary).unwrap();
    let config = Config {
        metres_per_stud: 0.28,
        obj_metres_per_unit: None,
        materials: None,
    };
    let baseline_dir = temporary.path().join("baseline");
    let baseline = convert(
        Path::new("tests/fixtures/doorway.glb"),
        &baseline_dir,
        &config,
    )
    .unwrap();

    for (index, uri) in [
        "mesh%20data.bin".to_owned(),
        format!(
            "data:application/octet-stream;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(binary)
        ),
    ]
    .iter()
    .enumerate()
    {
        document["buffers"][0]["uri"] = serde_json::json!(uri);
        fs::write(&source, serde_json::to_vec(&document).unwrap()).unwrap();
        let directory = temporary.path().join(format!("out-{index}"));
        let result = convert(&source, &directory, &config).unwrap();
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::to_value(&baseline).unwrap()
        );
        for mesh in &baseline.meshes {
            assert_eq!(
                fs::read(directory.join(&mesh.file)).unwrap(),
                fs::read(baseline_dir.join(&mesh.file)).unwrap()
            );
        }
    }

    fs::write(temporary.path().join("escape.bin"), binary).unwrap();
    for uri in [
        "https://example.com/mesh.bin",
        "../escape.bin",
        "%2e%2e/escape.bin",
        "file:///tmp/mesh.bin",
        "data:application/octet-stream;base64,invalid!",
    ] {
        document["buffers"][0]["uri"] = serde_json::json!(uri);
        fs::write(&source, serde_json::to_vec(&document).unwrap()).unwrap();
        let directory = temporary.path().join("rejected");
        assert!(
            convert(&source, &directory, &config).is_err(),
            "accepted {uri}"
        );
        assert!(!directory.exists());
    }
}

#[test]
fn gltf_preserves_colors_and_transforms_tangent_handedness() {
    let temporary = tempfile::tempdir().unwrap();
    let mut binary = Vec::new();
    let attributes: [&[f32]; 5] = [
        &[0., 0., 0., 1., 0., 0., 0., 1., 0.],
        &[0., 0., 1., 0., 0., 1., 0., 0., 1.],
        &[0., 0., 1., 0., 0., 1.],
        &[1., 0., 0., 1., 1., 0., 0., 1., 1., 0., 0., 1.],
        &[0.2, 0.4, 0.6, 0.8, 0.2, 0.4, 0.6, 0.8, 0.2, 0.4, 0.6, 0.8],
    ];
    let mut views = Vec::new();
    for attribute in attributes {
        let offset = binary.len();
        for component in attribute {
            binary.extend_from_slice(&component.to_le_bytes());
        }
        views.push(
            serde_json::json!({"buffer":0,"byteOffset":offset,"byteLength":binary.len()-offset}),
        );
    }
    let document = serde_json::json!({
        "asset":{"version":"2.0"}, "scene":0, "scenes":[{"nodes":[0]}],
        "nodes":[{"name":"colored","mesh":0,"scale":[-2,3,1]}],
        "meshes":[{"primitives":[{"attributes":{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2,"TANGENT":3,"COLOR_0":4}}]}],
        "buffers":[{"byteLength":binary.len(),"uri":format!("data:application/octet-stream;base64,{}",base64::engine::general_purpose::STANDARD.encode(&binary))}],
        "bufferViews":views,
        "accessors":[
            {"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]},
            {"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"},
            {"bufferView":2,"componentType":5126,"count":3,"type":"VEC2"},
            {"bufferView":3,"componentType":5126,"count":3,"type":"VEC4"},
            {"bufferView":4,"componentType":5126,"count":3,"type":"VEC4"}
        ]
    });
    let source = temporary.path().join("colored.gltf");
    fs::write(&source, serde_json::to_vec(&document).unwrap()).unwrap();
    let output = temporary.path().join("out");
    let manifest = convert(
        &source,
        &output,
        &Config {
            metres_per_stud: 1.,
            obj_metres_per_unit: None,
            materials: None,
        },
    )
    .unwrap();
    let bytes = fs::read(output.join(&manifest.meshes[0].file)).unwrap();
    for vertex in 0..3 {
        assert_eq!(
            &bytes[25 + vertex * 40 + 32..25 + vertex * 40 + 40],
            &[0, 127, 127, 0, 51, 102, 153, 204]
        );
    }
    let face = 25 + 3 * 40;
    assert_eq!(
        [
            u32_at(&bytes, face),
            u32_at(&bytes, face + 4),
            u32_at(&bytes, face + 8)
        ],
        [0, 2, 1]
    );
}

#[test]
fn fbx_preserves_color_alpha_and_tangent_sign_across_uv_and_transform_reflections() {
    let temporary = tempfile::tempdir().unwrap();
    let output = temporary.path().join("out");
    let manifest = convert(
        Path::new("tests/fixtures/attributes.fbx"),
        &output,
        &Config {
            metres_per_stud: 1.,
            obj_metres_per_unit: None,
            materials: None,
        },
    )
    .unwrap();
    assert_eq!(manifest.meshes.len(), 1);
    let bytes = fs::read(output.join(&manifest.meshes[0].file)).unwrap();
    assert_eq!(manifest.meshes[0].vertices, 3);
    for vertex in 0..3 {
        assert_eq!(
            &bytes[25 + vertex * 40 + 32..25 + vertex * 40 + 40],
            &[0, 127, 127, 254, 51, 102, 153, 204]
        );
    }
}

#[test]
fn real_assets_encode_deterministically_with_consistent_native_layout() {
    let temporary = tempfile::tempdir().unwrap();
    let config = Config {
        metres_per_stud: 0.28,
        obj_metres_per_unit: None,
        materials: None,
    };
    for name in ["doorway", "stair"] {
        let source = format!("tests/fixtures/{name}.glb");
        let first = temporary.path().join(name);
        let second = temporary.path().join(format!("{name}-again"));
        let manifest = convert(Path::new(&source), &first, &config).unwrap();
        convert(Path::new(&source), &second, &config).unwrap();
        assert!(!manifest.collision_generated);
        assert!(!manifest.meshes.is_empty());
        assert_eq!(
            fs::read(first.join("manifest.json")).unwrap(),
            fs::read(second.join("manifest.json")).unwrap()
        );

        for entry in manifest.meshes {
            let bytes = fs::read(first.join(&entry.file)).unwrap();
            let mut cursor = std::io::Cursor::new(&bytes);
            let decoded = rbx_mesh::read_mesh_versioned(&mut cursor).unwrap();
            assert_eq!(cursor.position() as usize, bytes.len());
            let rbx_mesh::mesh::Mesh::V2(decoded) = decoded else {
                panic!("wrong native mesh version");
            };
            assert_eq!(decoded.faces.len(), entry.triangles);
            let rbx_mesh::mesh::Vertices2::Full(vertices) = decoded.vertices else {
                panic!("missing native attributes");
            };
            assert_eq!(vertices.len(), entry.vertices);
            assert!(
                vertices
                    .iter()
                    .all(|v| v.color == [255; 4] && v.tangent == [0; 4])
            );
            assert_eq!(bytes, fs::read(second.join(&entry.file)).unwrap());
            assert_eq!(&bytes[..13], b"version 2.00\n");
            assert_eq!(&bytes[13..17], &[12, 0, 40, 12]);
            assert_eq!(u32_at(&bytes, 17), entry.vertices);
            assert_eq!(u32_at(&bytes, 21), entry.triangles);
            let face_start = 25 + entry.vertices * 40;
            assert_eq!(bytes.len(), face_start + entry.triangles * 12);
            for index in (face_start..bytes.len()).step_by(4) {
                assert!(u32_at(&bytes, index) < entry.vertices);
            }
            for vertex in 0..entry.vertices {
                let offset = 25 + vertex * 40;
                let normal_length: f32 = (0..3)
                    .map(|axis| f32_at(&bytes, offset + 12 + axis * 4).powi(2))
                    .sum();
                assert!((normal_length - 1.).abs() < 0.00001);
            }
        }
    }
}

#[test]
fn metric_configuration_is_respected_and_existing_outputs_are_preserved() {
    let temporary = tempfile::tempdir().unwrap();
    let source = Path::new("tests/fixtures/stair.glb");
    let a = temporary.path().join("a");
    let b = temporary.path().join("b");
    let manifest = convert(
        source,
        &a,
        &Config {
            metres_per_stud: 0.28,
            obj_metres_per_unit: None,
            materials: None,
        },
    )
    .unwrap();
    convert(
        source,
        &b,
        &Config {
            metres_per_stud: 0.56,
            obj_metres_per_unit: None,
            materials: None,
        },
    )
    .unwrap();
    for entry in manifest.meshes {
        let first = fs::read(a.join(&entry.file)).unwrap();
        let second = fs::read(b.join(&entry.file)).unwrap();
        for vertex in 0..entry.vertices {
            for axis in 0..3 {
                let offset = 25 + vertex * 40 + axis * 4;
                assert_eq!(f32_at(&first, offset) / 2., f32_at(&second, offset));
            }
        }
    }
    let before = fs::read(a.join("manifest.json")).unwrap();
    assert!(
        convert(
            source,
            &a,
            &Config {
                metres_per_stud: 1.,
                obj_metres_per_unit: None,
                materials: None,
            }
        )
        .is_err()
    );
    assert_eq!(before, fs::read(a.join("manifest.json")).unwrap());
    for scale in [0., -1., f32::NAN, f32::INFINITY] {
        let output = temporary.path().join("invalid");
        assert!(
            convert(
                source,
                &output,
                &Config {
                    metres_per_stud: scale,
                    obj_metres_per_unit: None,
                    materials: None,
                }
            )
            .is_err()
        );
        assert!(!output.exists());
    }
}

#[test]
fn unsupported_materials_fail_without_silently_losing_transmission() {
    let temporary = tempfile::tempdir().unwrap();
    let output = temporary.path().join("glass");
    let error = convert(
        Path::new("tests/fixtures/glass.glb"),
        &output,
        &Config {
            metres_per_stud: 0.28,
            obj_metres_per_unit: None,
            materials: None,
        },
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("extensions"));
    assert!(!output.exists());
}

#[test]
fn cli_conversion_needs_no_catalog_server_or_credentials() {
    let temporary = tempfile::tempdir().unwrap();
    let output_path = temporary.path().join("converted");
    let output = Command::new(env!("CARGO_BIN_EXE_roblox"))
        .env_clear()
        .args([
            "convert",
            "mesh",
            "tests/fixtures/doorway.glb",
            "--config",
            "examples/conversion.json",
            "--output",
        ])
        .arg(&output_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["scope"], "offlineConversion");
    assert!(output_path.join("manifest.json").exists());

    let unsupported = temporary.path().join("example.fbx");
    fs::write(&unsupported, b"not an implemented input adapter").unwrap();
    let failed = temporary.path().join("failed");
    assert!(
        convert(
            &unsupported,
            &failed,
            &Config {
                metres_per_stud: 1.,
                obj_metres_per_unit: None,
                materials: None,
            }
        )
        .is_err()
    );
    assert!(!failed.exists());
}

#[test]
fn collision_cli_emits_native_sidecars_and_honest_acceptance_metadata() {
    let temporary = tempfile::tempdir().unwrap();
    let output_path = temporary.path().join("converted");
    let output = Command::new(env!("CARGO_BIN_EXE_roblox"))
        .env_clear()
        .args([
            "convert",
            "mesh",
            "tests/fixtures/doorway.fbx",
            "--config",
            "examples/conversion.json",
            "--collision-config",
            "examples/collision-hull.json",
            "--output",
        ])
        .arg(&output_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["result"]["collisionGenerated"], true);
    for mesh in response["result"]["meshes"].as_array().unwrap() {
        assert_eq!(mesh["collision"]["engineVerified"], false);
        let bytes =
            fs::read(output_path.join(mesh["collision"]["file"].as_str().unwrap())).unwrap();
        let decoded = rbx_mesh::read_union_physics_versioned(std::io::Cursor::new(&bytes)).unwrap();
        let rbx_mesh::union_physics::UnionPhysics::V5(decoded) = decoded else {
            panic!("wrong collision format");
        };
        assert_eq!(decoded.meshes.len(), 1);
    }
}
