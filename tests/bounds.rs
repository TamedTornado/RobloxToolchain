use roblox_toolchain::{
    convert,
    mesh::{Bounds, Mesh, Vertex},
    skin, skin_import,
};
use std::{fs, path::Path};

fn vertex(position: [f32; 3]) -> Vertex {
    Vertex {
        position,
        normal: [0., 0., 1.],
        uv: [0., 0.],
        tangent: [0; 4],
        color: [255; 4],
    }
}

fn check(bounds: &Bounds, positions: &[[f32; 3]]) {
    for axis in 0..3 {
        let min = positions
            .iter()
            .map(|point| point[axis])
            .fold(f32::INFINITY, f32::min);
        let max = positions
            .iter()
            .map(|point| point[axis])
            .fold(f32::NEG_INFINITY, f32::max);
        assert_eq!(bounds.min[axis], min);
        assert_eq!(bounds.max[axis], max);
        assert_eq!(bounds.size[axis], max - min);
        assert_eq!(
            bounds.center[axis],
            ((f64::from(min) + f64::from(max)) * 0.5) as f32
        );
    }
}

#[test]
fn bounds_preserve_planar_extents_and_reject_unrepresentable_geometry() {
    let mut mesh = Mesh {
        vertices: vec![
            vertex([-3., 2., 7.]),
            vertex([5., -2., 7.]),
            vertex([0., 1., 7.]),
        ],
        triangles: vec![[0, 1, 2]],
    };
    let bounds = mesh.bounds().unwrap();
    assert_eq!(bounds.size, [8., 4., 0.]);
    assert_eq!(bounds.center, [1., 0., 7.]);
    mesh.vertices[0].position[0] = f32::NAN;
    assert!(mesh.bounds().is_err());
    mesh.vertices[0].position[0] = -f32::MAX;
    mesh.vertices[1].position[0] = f32::MAX;
    assert!(mesh.bounds().is_err());
    for position in [[f32::MAX; 3], [f32::from_bits(1); 3]] {
        let point = Mesh {
            vertices: vec![vertex(position)],
            triangles: vec![],
        };
        assert_eq!(point.bounds().unwrap().center, position);
        assert!(point.surface_bounds().is_err());
    }
}

#[test]
fn skin_bounds_exclude_source_vertices_that_native_subsets_do_not_write() {
    let mesh = Mesh {
        vertices: vec![
            vertex([-1., 0., 0.]),
            vertex([2., 0., 0.]),
            vertex([0., 3., 0.]),
            vertex([1000.; 3]),
        ],
        triangles: vec![[0, 1, 2]],
    };
    assert_eq!(mesh.bounds().unwrap().max, [1000.; 3]);
    let bone = skin::Bone {
        name: "Root".into(),
        parent: None,
        bind_cframe: [1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0.],
        cull_distance: 20.,
    };
    let weights = vec![
        skin::Influences {
            joints: [0; 4],
            weights: [1., 0., 0., 0.]
        };
        4
    ];
    let bytes = skin::encode(&mesh, &weights, &[bone]).unwrap();
    let rbx_mesh::mesh::Mesh::V4(native) =
        rbx_mesh::read_mesh_versioned(std::io::Cursor::new(bytes)).unwrap()
    else {
        panic!("expected v4");
    };
    assert_eq!(native.vertices.len(), 3);
    check(
        &mesh.surface_bounds().unwrap(),
        &native.vertices.iter().map(|v| v.pos).collect::<Vec<_>>(),
    );
}

#[test]
fn static_and_skinned_manifest_bounds_match_independent_native_decoding() {
    let temp = tempfile::tempdir().unwrap();
    for filename in ["doorway.glb", "doorway.fbx", "triangle.obj"] {
        let source = Path::new("tests/fixtures").join(filename);
        let output = temp.path().join(filename);
        let config = convert::Config {
            metres_per_stud: 0.28,
            obj_metres_per_unit: Some(1.),
            materials: None,
        };
        let result = convert::convert(&source, &output, &config).unwrap();
        for mesh in &result.meshes {
            let rbx_mesh::mesh::Mesh::V2(native) = rbx_mesh::read_mesh_versioned(
                std::io::Cursor::new(fs::read(output.join(&mesh.file)).unwrap()),
            )
            .unwrap() else {
                panic!("expected v2");
            };
            let rbx_mesh::mesh::Vertices2::Full(vertices) = native.vertices else {
                panic!("expected full vertices");
            };
            check(
                &mesh.bounds,
                &vertices.iter().map(|v| v.pos).collect::<Vec<_>>(),
            );
        }
        let half = convert::convert(
            &source,
            &temp.path().join(format!("half-{filename}")),
            &convert::Config {
                metres_per_stud: 0.56,
                ..config
            },
        )
        .unwrap();
        for (original, half) in result.meshes.iter().zip(&half.meshes) {
            assert_eq!(original.bounds.min.map(|v| v * 0.5), half.bounds.min);
            assert_eq!(original.bounds.max.map(|v| v * 0.5), half.bounds.max);
            assert_eq!(original.bounds.center.map(|v| v * 0.5), half.bounds.center);
            assert_eq!(original.bounds.size.map(|v| v * 0.5), half.bounds.size);
        }
    }
    let output = temp.path().join("skin");
    let policy = skin_import::Config {
        mesh_node: 2,
        metres_per_stud: 0.5,
        cull_distance_metres: 20.,
        rigid_tolerance: 0.0001,
    };
    let result = skin_import::convert(
        Path::new("tests/fixtures/rigged-simple.glb"),
        &output,
        &policy,
    )
    .unwrap();
    for mesh in result.meshes {
        let rbx_mesh::mesh::Mesh::V4(native) = rbx_mesh::read_mesh_versioned(std::io::Cursor::new(
            fs::read(output.join(&mesh.file)).unwrap(),
        ))
        .unwrap() else {
            panic!("expected v4");
        };
        check(
            &mesh.bounds,
            &native.vertices.iter().map(|v| v.pos).collect::<Vec<_>>(),
        );
    }
}
