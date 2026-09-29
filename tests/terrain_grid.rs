use roblox_toolchain::terrain_grid::{Cell, Chunk, Grid, Limits, decode, encode};
use std::{collections::BTreeMap, fs};

#[test]
fn independent_rojo_smooth_grid_decodes_and_reencodes_exactly() {
    let dom = rbx_binary::from_reader(fs::File::open("tests/fixtures/terrain-rojo.rbxm").unwrap())
        .unwrap();
    let terrain = dom.get_by_ref(dom.root().children()[0]).unwrap();
    let rbx_dom_weak::types::Variant::BinaryString(value) =
        &terrain.properties[&"SmoothGrid".into()]
    else {
        panic!("missing grid");
    };
    let bytes: &[u8] = value.as_ref();
    let limits = Limits {
        max_chunks: 16,
        max_cells: 524288,
    };
    let grid = decode(bytes, &limits).unwrap();
    assert_eq!(grid.chunk_exponent, 5);
    assert_eq!(grid.chunks.len(), 16);
    assert_eq!(grid.chunks[0].coordinate, [-1, -1, -1]);
    assert_eq!(grid.chunks[15].coordinate, [1, 0, 1]);
    let mut materials = BTreeMap::new();
    for cell in &grid.chunks[0].cells {
        *materials.entry(cell.material).or_insert(0) += 1;
    }
    assert_eq!(materials, BTreeMap::from([(0, 32570), (2, 198)]));
    let encoded = encode(&grid, &limits).unwrap();
    assert_eq!(
        encoded, bytes,
        "canonical run encoding differs from independent native fixture"
    );
    assert_eq!(decode(&encoded, &limits).unwrap(), grid);
}

#[test]
fn preserves_raw_channels_and_wrapping_negative_coordinate_deltas() {
    let cells = vec![
        Cell::default(),
        Cell {
            material: 1,
            occupancy: 255,
            auxiliary: 0,
        },
        Cell {
            material: 2,
            occupancy: 17,
            auxiliary: 93,
        },
        Cell {
            material: 63,
            occupancy: 254,
            auxiliary: 255,
        },
        Cell {
            material: 8,
            occupancy: 0,
            auxiliary: 0,
        },
        Cell {
            material: 2,
            occupancy: 255,
            auxiliary: 0,
        },
        Cell::default(),
        Cell::default(),
    ];
    let grid = Grid {
        chunk_exponent: 1,
        chunks: vec![
            Chunk {
                coordinate: [i32::MIN, -1, i32::MAX],
                cells: cells.clone(),
            },
            Chunk {
                coordinate: [i32::MAX, 0, i32::MIN],
                cells,
            },
        ],
    };
    let limits = Limits {
        max_chunks: 2,
        max_cells: 16,
    };
    let encoded = encode(&grid, &limits).unwrap();
    assert_eq!(decode(&encoded, &limits).unwrap(), grid);
    assert_eq!(encode(&grid, &limits).unwrap(), encoded);
    assert!(
        decode(
            &encoded,
            &Limits {
                max_chunks: 1,
                max_cells: 16
            }
        )
        .is_err()
    );
    assert!(
        decode(
            &encoded,
            &Limits {
                max_chunks: 2,
                max_cells: 15
            }
        )
        .is_err()
    );
}

#[test]
fn rejects_truncation_run_overflow_versions_and_unrepresentable_channels() {
    let limits = Limits {
        max_chunks: 1,
        max_cells: 8,
    };
    let mut bytes = vec![1, 1];
    bytes.extend([0; 12]);
    bytes.extend([0x80, 7]);
    assert_eq!(decode(&bytes, &limits).unwrap().chunks[0].cells.len(), 8);
    for end in 3..bytes.len() {
        assert!(decode(&bytes[..end], &limits).is_err());
    }
    *bytes.last_mut().unwrap() = 8;
    assert!(
        decode(&bytes, &limits)
            .unwrap_err()
            .to_string()
            .contains("run exceeds")
    );
    assert!(decode(&[2, 1], &limits).is_err());
    assert!(decode(&[1, 9], &limits).is_err());
    for cell in [
        Cell {
            material: 64,
            occupancy: 255,
            auxiliary: 0,
        },
        Cell {
            material: 0,
            occupancy: 1,
            auxiliary: 0,
        },
        Cell {
            material: 1,
            occupancy: 1,
            auxiliary: 1,
        },
    ] {
        let grid = Grid {
            chunk_exponent: 1,
            chunks: vec![Chunk {
                coordinate: [0; 3],
                cells: vec![cell; 8],
            }],
        };
        assert!(encode(&grid, &limits).is_err());
    }
}
