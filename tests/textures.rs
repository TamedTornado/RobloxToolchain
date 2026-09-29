use image::{Rgba, RgbaImage};
use roblox_toolchain::texture::{Config, Operation, convert};
use std::{fs, process::Command};

fn config(operation: Operation) -> Config {
    Config {
        operation,
        max_width: 2,
        max_height: 2,
        max_decoded_bytes: 4096,
        output: roblox_toolchain::texture::Output::Png,
    }
}

#[test]
fn rgba_dds_preserves_base_pixels_and_emits_semantic_mips_offline() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let image =
        RgbaImage::from_raw(3, 1, vec![255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0]).unwrap();
    image.save(root.join("source.png")).unwrap();
    let json = serde_json::json!({"operation":"color","maxWidth":3,"maxHeight":1,"maxDecodedBytes":4096,
        "output":{"format":"ddsRgba8","mipmaps":true,"maxOutputBytes":144,"mipFilter":"colorStraightAlpha"}});
    fs::write(root.join("config.json"), json.to_string()).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_roblox"))
        .env_clear()
        .args(["convert", "texture"])
        .arg(root.join("source.png"))
        .arg("--config")
        .arg(root.join("config.json"))
        .arg("--output")
        .arg(root.join("first"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let config: Config = serde_json::from_value(json.clone()).unwrap();
    convert(&root.join("source.png"), &root.join("second"), &config).unwrap();
    let path = root.join("first/color.dds");
    let bytes = fs::read(&path).unwrap();
    assert_eq!(bytes, fs::read(root.join("second/color.dds")).unwrap());
    assert_eq!(&bytes[140..], &[156, 156, 156, 128]);
    ffmpeg_next::init().unwrap();
    let mut input = ffmpeg_next::format::input(&path).unwrap();
    let mut decoder = ffmpeg_next::codec::context::Context::from_parameters(
        input.stream(0).unwrap().parameters(),
    )
    .unwrap()
    .decoder()
    .video()
    .unwrap();
    let mut packet = ffmpeg_next::Packet::empty();
    packet.read(&mut input).unwrap();
    decoder.send_packet(&packet).unwrap();
    let mut frame = ffmpeg_next::frame::Video::empty();
    decoder.receive_frame(&mut frame).unwrap();
    assert_eq!(frame.format(), ffmpeg_next::format::Pixel::RGBA);
    assert_eq!((frame.width(), frame.height()), (3, 1));
    assert_eq!(&frame.data(0)[..12], image.as_raw());
    let mut wrong = json;
    wrong["output"]["mipFilter"] = serde_json::json!("normal");
    let policy = serde_json::from_value(wrong).unwrap();
    assert!(convert(&root.join("source.png"), &root.join("bad"), &policy).is_err());
    assert!(!root.join("bad").exists());
}

#[test]
fn bc4_scalar_blocks_decode_independently_with_bounded_loss_and_full_mips() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let image = image::GrayImage::from_fn(5, 3, |x, y| image::Luma([(x * 40 + y * 20) as u8]));
    image.save(root.join("source.png")).unwrap();
    let mut policy = config(Operation::Roughness);
    policy.max_width = 5;
    policy.max_height = 3;
    policy.output = roblox_toolchain::texture::Output::DdsBc4 {
        mipmaps: true,
        max_output_bytes: 160,
    };
    convert(&root.join("source.png"), &root.join("first"), &policy).unwrap();
    convert(&root.join("source.png"), &root.join("second"), &policy).unwrap();
    let path = root.join("first/roughness.dds");
    let bytes = fs::read(&path).unwrap();
    assert_eq!(bytes, fs::read(root.join("second/roughness.dds")).unwrap());
    assert_eq!(bytes.len(), 160); // 5x3: 16 bytes; 2x1: 8; 1x1: 8
    assert_eq!(&bytes[84..88], b"ATI1");
    assert_eq!(u32::from_le_bytes(bytes[28..32].try_into().unwrap()), 3);
    ffmpeg_next::init().unwrap();
    let mut input = ffmpeg_next::format::input(&path).unwrap();
    let mut decoder = ffmpeg_next::codec::context::Context::from_parameters(
        input.stream(0).unwrap().parameters(),
    )
    .unwrap()
    .decoder()
    .video()
    .unwrap();
    let mut packet = ffmpeg_next::Packet::empty();
    packet.read(&mut input).unwrap();
    decoder.send_packet(&packet).unwrap();
    let mut frame = ffmpeg_next::frame::Video::empty();
    decoder.receive_frame(&mut frame).unwrap();
    assert_eq!(frame.format(), ffmpeg_next::format::Pixel::RGBA);
    assert_eq!((frame.width(), frame.height()), (5, 3));
    for (x, y, pixel) in image.enumerate_pixels() {
        let actual = frame.data(0)[y as usize * frame.stride(0) + x as usize * 4];
        assert!(
            actual.abs_diff(pixel[0]) <= 15,
            "({x},{y}): {actual} != {}",
            pixel[0]
        );
    }
    policy.output = roblox_toolchain::texture::Output::DdsBc4 {
        mipmaps: true,
        max_output_bytes: 159,
    };
    assert!(convert(&root.join("source.png"), &root.join("bad"), &policy).is_err());
    assert!(!root.join("bad").exists());
}

#[test]
fn native_scalar_dds_cli_is_decodable_and_rejects_wrong_semantics() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    image::GrayImage::from_raw(3, 1, vec![0, 128, 255])
        .unwrap()
        .save(root.join("scalar.png"))
        .unwrap();
    let config = serde_json::json!({
        "operation":"roughness", "maxWidth":3, "maxHeight":1,
        "maxDecodedBytes":4096,
        "output":{"format":"ddsL8", "mipmaps":true, "maxOutputBytes":132}
    });
    fs::write(root.join("config.json"), config.to_string()).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_roblox"))
        .env_clear()
        .args(["convert", "texture"])
        .arg(root.join("scalar.png"))
        .arg("--config")
        .arg(root.join("config.json"))
        .arg("--output")
        .arg(root.join("out"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let path = root.join("out/roughness.dds");
    assert_eq!(&fs::read(&path).unwrap()[128..], &[0, 128, 255, 128]);

    // Independent native library decoder, not a reader paired with our encoder.
    ffmpeg_next::init().unwrap();
    let mut input = ffmpeg_next::format::input(&path).unwrap();
    let mut decoder = ffmpeg_next::codec::context::Context::from_parameters(
        input.stream(0).unwrap().parameters(),
    )
    .unwrap()
    .decoder()
    .video()
    .unwrap();
    let mut packet = ffmpeg_next::Packet::empty();
    packet.read(&mut input).unwrap();
    decoder.send_packet(&packet).unwrap();
    let mut frame = ffmpeg_next::frame::Video::empty();
    decoder.receive_frame(&mut frame).unwrap();
    assert_eq!((frame.width(), frame.height()), (3, 1));
    assert_eq!(frame.format(), ffmpeg_next::format::Pixel::GRAY8);
    assert_eq!(&frame.data(0)[..3], &[0, 128, 255]);

    let mut wrong = config;
    wrong["operation"] = serde_json::json!("color");
    let parsed: Config = serde_json::from_value(wrong).unwrap();
    assert!(convert(&root.join("scalar.png"), &root.join("bad"), &parsed).is_err());
    assert!(!root.join("bad").exists());
}

#[test]
fn color_alpha_normal_and_pbr_channels_are_preserved_or_transformed_exactly() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("source.png");
    let mut pixels = RgbaImage::new(2, 1);
    pixels.put_pixel(0, 0, Rgba([10, 20, 30, 40]));
    pixels.put_pixel(1, 0, Rgba([50, 60, 70, 80]));
    pixels.save(&source).unwrap();

    let color = temporary.path().join("color");
    convert(&source, &color, &config(Operation::Color)).unwrap();
    assert_eq!(
        image::open(color.join("color.png")).unwrap().to_rgba8(),
        pixels
    );
    let before = fs::read(color.join("color.png")).unwrap();
    assert!(convert(&source, &color, &config(Operation::Color)).is_err());
    assert_eq!(fs::read(color.join("color.png")).unwrap(), before);

    let normal = temporary.path().join("normal");
    convert(&source, &normal, &config(Operation::NormalDirectX)).unwrap();
    assert_eq!(
        image::open(normal.join("normal.png"))
            .unwrap()
            .to_rgb8()
            .get_pixel(0, 0)
            .0,
        [10, 235, 30]
    );

    let pbr = temporary.path().join("pbr");
    convert(&source, &pbr, &config(Operation::GltfMetallicRoughness)).unwrap();
    assert_eq!(
        image::open(pbr.join("roughness.png"))
            .unwrap()
            .into_luma8()
            .into_raw(),
        [20, 60]
    );
    assert_eq!(
        image::open(pbr.join("metalness.png"))
            .unwrap()
            .into_luma8()
            .into_raw(),
        [30, 70]
    );
    let repeat = temporary.path().join("repeat");
    convert(&source, &repeat, &config(Operation::GltfMetallicRoughness)).unwrap();
    for name in ["roughness.png", "metalness.png", "manifest.json"] {
        assert_eq!(
            fs::read(pbr.join(name)).unwrap(),
            fs::read(repeat.join(name)).unwrap()
        );
    }
}

#[test]
fn explicit_decode_limits_and_bit_depth_rejection_are_enforced() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("source.png");
    RgbaImage::new(2, 1).save(&source).unwrap();
    let out = temporary.path().join("rejected");
    let mut policy = config(Operation::Color);
    policy.max_width = 1;
    assert!(convert(&source, &out, &policy).is_err());
    assert!(!out.exists());
    policy.max_width = 2;
    policy.max_decoded_bytes = 1;
    assert!(convert(&source, &out, &policy).is_err());
    assert!(!out.exists());
    let hdr = image::ImageBuffer::<Rgba<u16>, Vec<u16>>::new(1, 1);
    hdr.save(&source).unwrap();
    assert!(
        convert(&source, &out, &config(Operation::Color))
            .err()
            .unwrap()
            .to_string()
            .contains("8-bit")
    );
    assert!(!out.exists());
}

#[test]
fn standalone_scalar_maps_preserve_linear_samples_without_luminance_conversion() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("scalar.png");
    image::GrayImage::from_raw(2, 1, vec![37, 191])
        .unwrap()
        .save(&source)
        .unwrap();
    for (operation, semantic) in [
        (Operation::Roughness, "roughness"),
        (Operation::Metalness, "metalness"),
    ] {
        let output = temp.path().join(semantic);
        let manifest = convert(&source, &output, &config(operation)).unwrap();
        assert_eq!(manifest.textures[0].color_space, "linear");
        assert_eq!(
            image::open(output.join(format!("{semantic}.png")))
                .unwrap()
                .to_luma8()
                .into_raw(),
            [37, 191]
        );
    }
}

#[test]
fn texture_cli_returns_structured_results_without_credentials() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("source.png");
    RgbaImage::new(2, 1).save(&source).unwrap();
    let policy = temporary.path().join("config.json");
    fs::write(
        &policy,
        r#"{"operation":"normalOpenGl","maxWidth":2,"maxHeight":2,"maxDecodedBytes":4096}"#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_roblox"))
        .env_clear()
        .args(["convert", "texture"])
        .arg(source)
        .arg("--config")
        .arg(policy)
        .arg("--output")
        .arg(temporary.path().join("out"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["result"]["textures"][0]["semantic"], "normal");
    assert_eq!(value["result"]["textures"][0]["colorSpace"], "linear");
}
