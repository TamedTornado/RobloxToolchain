use roblox_toolchain::{
    terrain_grid,
    terrain_voxels::{self, Config, Voxel},
};
use serde_json::json;

fn config() -> Config {
    serde_json::from_value(json!({"metresPerStud":0.5,"voxelSizeMetres":2,"originMetres":[-2,0,2],
        "alignmentToleranceMetres":0.00001,"chunkExponent":1,"limits":{"maxChunks":4,"maxCells":32},
        "materials":{"soil":"Grass","stone":"Rock","empty":"Air","water":"Water"}
    })).unwrap()
}

#[test]
fn native_material_slots_match_independent_public_enum_numbers() {
    let values = [
        1792, 2048, 1280, 800, 816, 848, 1296, 528, 896, 1552, 1328, 912, 1344, 788, 1360, 804,
        1376, 880, 1536, 1284, 1392, 820, 836,
    ];
    let materials = &rbx_reflection_database::get_bundled().enums["Material"].items;
    for (slot, value) in values.into_iter().enumerate() {
        let (name, _) = materials.iter().find(|(_, v)| **v == value).unwrap();
        assert_eq!(terrain_voxels::material_slot(name).unwrap(), slot as u8);
    }
    assert!(terrain_voxels::material_slot("Plastic").is_err());
}

#[test]
fn quantization_matches_native_single_precision_operations() {
    for (input, expected) in [
        (0.00001, 0),
        (1. / 256., 0),
        (0.5, 127),
        (128.5 / 256., 128),
        (1., 255),
    ] {
        let cell = terrain_voxels::cell(2, input).unwrap();
        assert_eq!(cell.material, 2);
        assert_eq!(cell.occupancy, expected);
    }
    assert_eq!(
        terrain_voxels::cell(2, 0.).unwrap(),
        terrain_grid::Cell::default()
    );
    for value in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
        assert!(terrain_voxels::cell(2, value).is_err());
    }
    assert!(terrain_voxels::cell(0, 1.).is_err());
}

#[test]
fn metric_origin_negative_chunks_and_native_axis_order_are_preserved() {
    let config = config();
    let voxels = vec![
        Voxel {
            position: [0, 0, 0],
            material: "soil".into(),
            occupancy: 0.5,
        },
        Voxel {
            position: [1, 1, 1],
            material: "stone".into(),
            occupancy: 1.,
        },
    ];
    let grid = terrain_voxels::build(&voxels, &config).unwrap();
    assert_eq!(grid.chunks[0].coordinate, [-1, 0, 0]);
    assert_eq!(
        grid.chunks[0].cells[3],
        terrain_voxels::cell(2, 0.5).unwrap()
    );
    assert_eq!(grid.chunks[1].coordinate, [0, 0, 1]);
    assert_eq!(
        grid.chunks[1].cells[4],
        terrain_voxels::cell(8, 1.).unwrap()
    );
    let bytes = terrain_grid::encode(&grid, &config.limits).unwrap();
    assert_eq!(terrain_grid::decode(&bytes, &config.limits).unwrap(), grid);
    let mut reversed = voxels;
    reversed.reverse();
    assert_eq!(
        terrain_grid::encode(
            &terrain_voxels::build(&reversed, &config).unwrap(),
            &config.limits
        )
        .unwrap(),
        bytes
    );
}

#[test]
fn rejects_implicit_resampling_bad_aliases_duplicate_cells_and_budget_overruns() {
    let mut config = config();
    let voxels = vec![Voxel {
        position: [0; 3],
        material: "soil".into(),
        occupancy: 1.,
    }];
    config.voxel_size_metres = 1.;
    assert!(terrain_voxels::build(&voxels, &config).is_err());
    config.voxel_size_metres = 2.;
    config.origin_metres[0] = 0.5;
    assert!(terrain_voxels::build(&voxels, &config).is_err());
    config.origin_metres[0] = 0.;
    config.materials.insert("bad".into(), "Plastic".into());
    assert!(terrain_voxels::build(&voxels, &config).is_err());
    config.materials.remove("bad");
    config.limits.max_cells = 7;
    assert!(terrain_voxels::build(&voxels, &config).is_err());
    config.limits.max_cells = 32;
    let duplicate = vec![
        Voxel {
            position: [0; 3],
            material: "soil".into(),
            occupancy: 0.,
        },
        Voxel {
            position: [0; 3],
            material: "soil".into(),
            occupancy: 1.,
        },
    ];
    assert!(terrain_voxels::build(&duplicate, &config).is_err());
}

#[test]
fn named_sparse_voxels_recreate_the_independent_native_payload() {
    let dom =
        rbx_binary::from_reader(std::fs::File::open("tests/fixtures/terrain-rojo.rbxm").unwrap())
            .unwrap();
    let terrain = dom.get_by_ref(dom.root().children()[0]).unwrap();
    let rbx_dom_weak::types::Variant::BinaryString(value) =
        &terrain.properties[&"SmoothGrid".into()]
    else {
        panic!("missing grid");
    };
    let bytes: &[u8] = value.as_ref();
    let mut config = config();
    config.origin_metres = [0.; 3];
    config.chunk_exponent = 5;
    config.limits = terrain_grid::Limits {
        max_chunks: 16,
        max_cells: 524288,
    };
    let native = terrain_grid::decode(bytes, &config.limits).unwrap();
    let mut voxels = Vec::new();
    for chunk in native.chunks {
        for (index, cell) in chunk.cells.iter().enumerate() {
            if cell.material == 0 {
                continue;
            }
            let local = [index % 32, index / (32 * 32), (index / 32) % 32];
            let position =
                std::array::from_fn(|axis| chunk.coordinate[axis] * 32 + local[axis] as i32);
            let material = match cell.material {
                2 => "soil",
                8 => "stone",
                other => panic!("unexpected fixture material {other}"),
            };
            // Interior representative of the native quantization bin, including
            // raw occupancy zero on a non-Air cell (not an empty input voxel).
            let occupancy = (f32::from(cell.occupancy) + 0.5) / 256.;
            voxels.push(Voxel {
                position,
                material: material.into(),
                occupancy,
            });
        }
    }
    let grid = terrain_voxels::build(&voxels, &config).unwrap();
    assert_eq!(terrain_grid::encode(&grid, &config.limits).unwrap(), bytes);
}
