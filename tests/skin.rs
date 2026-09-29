use roblox_toolchain::{
    mesh::{Mesh, Vertex},
    skin::{Bone, Influences, encode},
};

fn bone(index: usize) -> Bone {
    Bone {
        name: format!("Bone{index}"),
        parent: if index == 0 { None } else { Some(0) },
        bind_cframe: [1., 0., 0., 0., 1., 0., 0., 0., 1., index as f32, 0., 0.],
        cull_distance: 100.,
    }
}

fn triangle() -> Mesh {
    Mesh {
        vertices: [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]]
            .into_iter()
            .map(|position| Vertex {
                position,
                normal: [0., 0., 1.],
                uv: [0., 0.],
                tangent: [254, 127, 127, 254],
                color: [255; 4],
            })
            .collect(),
        triangles: vec![[0, 1, 2]],
    }
}

#[test]
fn native_skin_round_trips_bones_bind_poses_names_and_normalized_weights() {
    let source = triangle();
    let influences = vec![
        Influences {
            joints: [0, 1, 0, 0],
            weights: [0.25, 0.75, 0., 0.]
        };
        3
    ];
    let bones = vec![bone(0), bone(1)];
    let bytes = encode(&source, &influences, &bones).unwrap();
    assert_eq!(bytes, encode(&source, &influences, &bones).unwrap());
    let mut cursor = std::io::Cursor::new(&bytes);
    let decoded = rbx_mesh::read_mesh_versioned(&mut cursor).unwrap();
    assert_eq!(cursor.position() as usize, bytes.len());
    let rbx_mesh::mesh::Mesh::V4(decoded) = decoded else {
        panic!("not native v4");
    };
    assert_eq!(decoded.vertices.len(), 3);
    assert_eq!(decoded.faces.len(), 1);
    assert_eq!(decoded.bones.len(), 2);
    assert_eq!(decoded.bones[1].parent.get(), Some(0));
    assert_eq!(decoded.bones[1].cframe.x, 1.);
    assert_eq!(decoded.bone_names, b"Bone0\0Bone1\0");
    assert_eq!(decoded.envelopes[0].weights, [64, 191, 0, 0]);
    assert_eq!(decoded.subsets.len(), 1);
    assert_eq!(decoded.subsets[0].bone_count, 2);
}

#[test]
fn meshes_partition_at_the_native_palette_boundary_without_dropping_faces() {
    let bones: Vec<_> = (0..30).map(bone).collect();
    let mut source = Mesh {
        vertices: Vec::new(),
        triangles: Vec::new(),
    };
    let mut influences = Vec::new();
    for joint in 0..30 {
        let base = source.vertices.len() as u32;
        source.vertices.extend(triangle().vertices);
        source.triangles.push([base, base + 1, base + 2]);
        influences.extend(vec![
            Influences {
                joints: [joint, 0, 0, 0],
                weights: [1., 0., 0., 0.]
            };
            3
        ]);
    }
    let decoded = rbx_mesh::read_mesh_versioned(std::io::Cursor::new(
        encode(&source, &influences, &bones).unwrap(),
    ))
    .unwrap();
    let rbx_mesh::mesh::Mesh::V4(decoded) = decoded else {
        panic!("not native v4");
    };
    assert_eq!(decoded.faces.len(), 30);
    assert_eq!(decoded.subsets.len(), 2);
    assert_eq!(decoded.subsets[0].bone_count, 26);
    assert_eq!(decoded.subsets[1].bone_count, 4);
    for subset in &decoded.subsets {
        for vertex in subset.vertices_offset..subset.vertices_offset + subset.vertices_len {
            let envelope = &decoded.envelopes[vertex as usize];
            assert_eq!(
                envelope.weights.iter().map(|w| u16::from(*w)).sum::<u16>(),
                255
            );
            let global = subset.bones[envelope.bones[0] as usize].get().unwrap();
            assert_eq!(global as usize, vertex as usize / 3);
        }
    }
}

#[test]
fn invalid_skeletons_and_envelopes_fail_instead_of_truncating() {
    let source = triangle();
    let mut influences = vec![
        Influences {
            joints: [0; 4],
            weights: [1., 0., 0., 0.]
        };
        3
    ];
    let mut bones = vec![bone(0)];
    influences[0].joints[0] = 1;
    assert!(encode(&source, &influences, &bones).is_err());
    influences[0].joints[0] = 0;
    influences[0].weights = [0.; 4];
    assert!(encode(&source, &influences, &bones).is_err());
    influences[0].weights = [1., 0., 0., 0.];
    bones[0].parent = Some(0);
    assert!(encode(&source, &influences, &bones).is_err());
    bones[0].parent = None;
    influences[0].weights[0] = f32::NAN;
    assert!(encode(&source, &influences, &bones).is_err());
}
