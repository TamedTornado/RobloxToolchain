use glam::{Mat4, Vec3};
use roblox_toolchain::skin_import::{self, Config};
use std::{fs, path::Path};

fn matrix(value: &ufbx::Matrix) -> Mat4 {
    Mat4::from_cols_array(&[
        value.m00 as f32,
        value.m10 as f32,
        value.m20 as f32,
        0.,
        value.m01 as f32,
        value.m11 as f32,
        value.m21 as f32,
        0.,
        value.m02 as f32,
        value.m12 as f32,
        value.m22 as f32,
        0.,
        value.m03 as f32,
        value.m13 as f32,
        value.m23 as f32,
        1.,
    ])
}

fn verify_deformation(
    native: &rbx_mesh::mesh::Mesh4,
    scene: &ufbx::Scene,
    node: &ufbx::Node,
    manifest: &skin_import::Manifest,
    material: u32,
    policy: &Config,
) {
    let mesh = node.mesh.as_ref().unwrap();
    let skin = &mesh.skin_deformers[0];
    let first = &skin.clusters[0];
    let bind_geometry = ufbx::matrix_mul(&first.bind_to_world, &first.geometry_to_bone);
    let mut expected = Vec::new();
    let mut corners = Vec::new();
    for (index, face) in mesh.faces.iter().enumerate() {
        if mesh.face_material.get(index).copied().unwrap_or(0) != material {
            continue;
        }
        let count = ufbx::triangulate_face_vec(&mut corners, mesh, *face) as usize * 3;
        for triangle in corners[..count].chunks_exact(3) {
            let order = if ufbx::matrix_determinant(&bind_geometry) < 0. {
                [0, 2, 1]
            } else {
                [0, 1, 2]
            };
            for corner in order {
                let index = triangle[corner] as usize;
                let point = mesh.vertex_indices[index] as usize;
                let transform = ufbx::get_skin_vertex_matrix(skin, point, &node.geometry_to_world);
                let position = ufbx::transform_position(&transform, mesh.vertex_position[index]);
                expected.push(
                    Vec3::new(position.x as f32, position.y as f32, position.z as f32)
                        / policy.metres_per_stud,
                );
            }
        }
    }
    let units = Mat4::from_scale(Vec3::splat(1. / policy.metres_per_stud));
    assert_eq!(expected.len(), native.faces.len() * 3);
    for (face_index, face) in native.faces.iter().enumerate() {
        let subset = native
            .subsets
            .iter()
            .find(|subset| {
                face_index >= subset.faces_offset as usize
                    && face_index < (subset.faces_offset + subset.faces_len) as usize
            })
            .unwrap();
        for (corner, vertex) in face.0.iter().enumerate() {
            let index = vertex.0 as usize;
            let envelope = &native.envelopes[index];
            let mut actual = Vec3::ZERO;
            for slot in 0..4 {
                if envelope.weights[slot] == 0 {
                    continue;
                }
                let bone_index = usize::from(
                    subset.bones[usize::from(envelope.bones[slot])]
                        .get()
                        .unwrap(),
                );
                let bone = &native.bones[bone_index].cframe;
                let bind = Mat4::from_cols_array(&[
                    bone.r00, bone.r10, bone.r20, 0., bone.r01, bone.r11, bone.r21, 0., bone.r02,
                    bone.r12, bone.r22, 0., bone.x, bone.y, bone.z, 1.,
                ]);
                let source_node = &scene.nodes[manifest.rig[bone_index].source_node];
                let pose = units * matrix(&source_node.node_to_world) * units.inverse();
                actual += (pose * bind.inverse())
                    .transform_point3(Vec3::from_array(native.vertices[index].pos))
                    * (f32::from(envelope.weights[slot]) / 255.);
            }
            let reference = expected[face_index * 3 + corner];
            assert!(
                actual.distance(reference) < 0.02,
                "native {actual:?}, ufbx {reference:?}"
            );
        }
    }
}

fn load(source: &Path) -> ufbx::SceneRoot {
    ufbx::load_memory(
        &fs::read(source).unwrap(),
        ufbx::LoadOpts {
            file_format: ufbx::FileFormat::Fbx,
            target_axes: ufbx::CoordinateAxes::right_handed_y_up(),
            target_unit_meters: 1.,
            space_conversion: ufbx::SpaceConversion::ModifyGeometry,
            load_external_files: false,
            use_blender_pbr_material: true,
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn external_fbx_skin_preserves_clusters_and_deterministic_native_geometry() {
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/maya-transformed-skin.fbx");
    let scene = load(&source);
    let node = scene
        .nodes
        .iter()
        .find(|node| {
            node.mesh
                .as_ref()
                .is_some_and(|mesh| !mesh.skin_deformers.is_empty())
        })
        .unwrap();
    let mesh = node.mesh.as_ref().unwrap();
    let policy = Config {
        mesh_node: node.element.typed_id as usize,
        metres_per_stud: 0.28,
        cull_distance_metres: 30.,
        rigid_tolerance: 0.0001,
    };
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("native");
    let manifest = skin_import::convert(&source, &output, &policy).unwrap();
    assert!(manifest.rig.len() >= mesh.skin_deformers[0].clusters.len());
    for cluster in &mesh.skin_deformers[0].clusters {
        let bone = cluster.bone_node.as_ref().unwrap();
        assert!(
            manifest
                .rig
                .iter()
                .any(|binding| binding.source_node == bone.element.typed_id as usize)
        );
    }
    assert!(!manifest.meshes.is_empty());
    for entry in &manifest.meshes {
        let bytes = fs::read(output.join(&entry.file)).unwrap();
        let rbx_mesh::mesh::Mesh::V4(native) =
            rbx_mesh::read_mesh_versioned(std::io::Cursor::new(bytes)).unwrap()
        else {
            panic!("not v4");
        };
        assert_eq!(native.bones.len(), manifest.rig.len());
        assert_eq!(native.faces.len(), entry.triangles);
        for axis in 0..3 {
            let min = native
                .vertices
                .iter()
                .map(|vertex| vertex.pos[axis])
                .fold(f32::INFINITY, f32::min);
            let max = native
                .vertices
                .iter()
                .map(|vertex| vertex.pos[axis])
                .fold(f32::NEG_INFINITY, f32::max);
            assert_eq!(entry.bounds.min[axis], min);
            assert_eq!(entry.bounds.max[axis], max);
            assert_eq!(entry.bounds.size[axis], max - min);
        }
        let material: u32 = entry
            .file
            .rsplit("primitive-")
            .next()
            .unwrap()
            .trim_end_matches(".mesh")
            .parse()
            .unwrap();
        verify_deformation(&native, &scene, node, &manifest, material, &policy);
        assert!(
            native.envelopes.iter().all(|envelope| envelope
                .weights
                .iter()
                .map(|v| u16::from(*v))
                .sum::<u16>()
                == 255)
        );
    }
    let repeated = directory.path().join("repeated");
    let config_path = directory.path().join("config.json");
    fs::write(&config_path, serde_json::to_vec(&policy).unwrap()).unwrap();
    let cli_output = directory.path().join("cli");
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_roblox"))
        .env_clear()
        .args(["convert", "skin"])
        .arg(&source)
        .arg("--config")
        .arg(config_path)
        .arg("--output")
        .arg(&cli_output)
        .output()
        .unwrap();
    assert!(result.status.success(), "{:?}", result.stderr);
    assert_eq!(
        fs::read(output.join("manifest.json")).unwrap(),
        fs::read(cli_output.join("manifest.json")).unwrap()
    );
    let second = skin_import::convert(&source, &repeated, &policy).unwrap();
    assert_eq!(
        serde_json::to_vec(&manifest).unwrap(),
        serde_json::to_vec(&second).unwrap()
    );
    for entry in &manifest.meshes {
        assert_eq!(
            fs::read(output.join(&entry.file)).unwrap(),
            fs::read(repeated.join(&entry.file)).unwrap()
        );
    }
}
