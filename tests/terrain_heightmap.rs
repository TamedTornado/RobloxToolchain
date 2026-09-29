use roblox_toolchain::{
    terrain_grid,
    terrain_heightmap::{self, Config},
};
use serde_json::json;

fn config() -> Config {
    serde_json::from_value(json!({"terrain":{
        "metresPerStud":0.5,"voxelSizeMetres":2,"originMetres":[0,0,0],"alignmentToleranceMetres":0.00001,
        "chunkExponent":1,"limits":{"maxChunks":8,"maxCells":64},"materials":{"ground":"Grass"}},
        "material":"ground","pixelSizeMetres":2,"floorMetres":0,"heightMinMetres":0,"heightMaxMetres":2,
        "rowDirection":"negative","maxWidth":2,"maxHeight":2,"maxDecodedBytes":4096
    })).unwrap()
}

#[test]
fn scalar_pixels_make_metric_columns_with_explicit_row_direction() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("height.png");
    image::GrayImage::from_raw(2, 2, vec![128, 255, 0, 255])
        .unwrap()
        .save(&source)
        .unwrap();
    let config = config();
    let grid = terrain_heightmap::read(&source, &config).unwrap();
    assert_eq!(grid.chunks.len(), 2);
    let negative = grid
        .chunks
        .iter()
        .find(|c| c.coordinate == [0, 0, -1])
        .unwrap();
    assert_eq!(negative.cells[3].material, 2);
    assert_eq!(negative.cells[3].occupancy, 255);
    let positive = grid
        .chunks
        .iter()
        .find(|c| c.coordinate == [0, 0, 0])
        .unwrap();
    assert_eq!(positive.cells[0].occupancy, 128);
    assert_eq!(positive.cells[1].occupancy, 255);
    let bytes = terrain_grid::encode(&grid, &config.terrain.limits).unwrap();
    assert_eq!(
        terrain_grid::decode(&bytes, &config.terrain.limits).unwrap(),
        grid
    );
    assert_eq!(
        terrain_grid::encode(
            &terrain_heightmap::read(&source, &config).unwrap(),
            &config.terrain.limits
        )
        .unwrap(),
        bytes
    );
}

#[test]
fn sixteen_bit_samples_preserve_fractional_surface_cells_above_negative_floor() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("height.png");
    image::ImageBuffer::<image::Luma<u16>, Vec<u16>>::from_raw(1, 1, vec![32768])
        .unwrap()
        .save(&source)
        .unwrap();
    let mut config = config();
    config.floor_metres = -2.;
    config.height_min_metres = -2.;
    config.height_max_metres = 2.;
    let grid = terrain_heightmap::read(&source, &config).unwrap();
    let lower = grid
        .chunks
        .iter()
        .find(|c| c.coordinate == [0, -1, 0])
        .unwrap();
    assert_eq!(lower.cells[4].occupancy, 255);
    let upper = grid
        .chunks
        .iter()
        .find(|c| c.coordinate == [0, 0, 0])
        .unwrap();
    assert_eq!(upper.cells[0].material, 2);
    assert_eq!(upper.cells[0].occupancy, 0); // tiny positive height, not empty
}

#[test]
fn rejects_ambiguous_pixels_dimensions_resampling_and_over_budget_columns() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("height.png");
    image::RgbImage::from_pixel(1, 1, image::Rgb([1, 2, 3]))
        .save(&source)
        .unwrap();
    assert!(terrain_heightmap::read(&source, &config()).is_err());
    image::GrayImage::from_pixel(2, 2, image::Luma([255]))
        .save(&source)
        .unwrap();
    let mut config = config();
    config.max_width = 1;
    assert!(terrain_heightmap::read(&source, &config).is_err());
    config.max_width = 2;
    config.pixel_size_metres = 1.;
    assert!(terrain_heightmap::read(&source, &config).is_err());
    config.pixel_size_metres = 2.;
    config.height_max_metres = 1000.;
    assert!(
        terrain_heightmap::read(&source, &config)
            .unwrap_err()
            .to_string()
            .contains("cell budget")
    );
    config.height_max_metres = 2.;
    config.floor_metres = -0.5;
    assert!(terrain_heightmap::read(&source, &config).is_err());
}
