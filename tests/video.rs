use roblox_toolchain::video::{Config, convert};
use std::{fs, path::Path};

fn config() -> Config {
    Config {
        max_width: 64,
        max_height: 48,
        max_frames: 4,
        bitrate: 100000,
        crf: 20,
    }
}

fn decoded_planes(path: &Path) -> Vec<[Vec<u8>; 3]> {
    use ffmpeg_next as av;
    av::init().unwrap();
    let mut input = av::format::input(path).unwrap();
    let parameters = input.stream(0).unwrap().parameters();
    let mut decoder = av::codec::context::Context::from_parameters(parameters)
        .unwrap()
        .decoder()
        .video()
        .unwrap();
    let mut result = Vec::new();
    let mut drain = |decoder: &mut av::decoder::Video| {
        loop {
            let mut frame = av::frame::Video::empty();
            match decoder.receive_frame(&mut frame) {
                Ok(()) => {
                    assert!(!frame.is_corrupt());
                    assert_eq!(frame.format(), av::format::Pixel::YUV420P);
                    let planes = std::array::from_fn(|plane| {
                        let width = if plane == 0 {
                            frame.width()
                        } else {
                            frame.width() / 2
                        } as usize;
                        let height = if plane == 0 {
                            frame.height()
                        } else {
                            frame.height() / 2
                        } as usize;
                        let mut pixels = Vec::new();
                        for row in 0..height {
                            let offset = row * frame.stride(plane);
                            pixels.extend_from_slice(&frame.data(plane)[offset..offset + width]);
                        }
                        pixels
                    });
                    result.push(planes);
                }
                Err(av::Error::Eof) => break,
                Err(av::Error::Other { errno }) if errno == av::error::EAGAIN => break,
                Err(error) => panic!("frame decode failed: {error}"),
            }
        }
    };
    loop {
        let mut packet = av::Packet::empty();
        match packet.read(&mut input) {
            Ok(()) => {
                assert!(!packet.is_corrupt());
                decoder.send_packet(&packet).unwrap();
                drain(&mut decoder);
            }
            Err(av::Error::Eof) => break,
            Err(error) => panic!("packet decode failed: {error}"),
        }
    }
    decoder.send_eof().unwrap();
    drain(&mut decoder);
    result
}

#[test]
fn vp9_preserves_each_source_color_plane_with_bounded_fixture_distortion() {
    let temp = tempfile::tempdir().unwrap();
    let source = Path::new("tests/fixtures/video-64x48.mp4");
    let output = temp.path().join("video.webm");
    convert(source, &output, &config()).unwrap();
    let original = decoded_planes(source);
    let converted = decoded_planes(&output);
    assert_eq!(original.len(), converted.len());
    for (index, (original, converted)) in original.iter().zip(&converted).enumerate() {
        for plane in 0..3 {
            assert_eq!(original[plane].len(), converted[plane].len());
            let mse = original[plane]
                .iter()
                .zip(&converted[plane])
                .map(|(a, b)| (f64::from(*a) - f64::from(*b)).powi(2))
                .sum::<f64>()
                / original[plane].len() as f64;
            // Test-specific fidelity threshold, not a hidden production quality policy.
            assert!(mse < 25.0, "frame {index} plane {plane} MSE={mse}");
        }
    }
}

#[test]
fn fractional_frame_rate_roundtrips_through_webm_timestamp_quantization() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first.webm");
    let second = temp.path().join("second.webm");
    let policy = Config {
        max_frames: 6,
        ..config()
    };
    let result = convert(Path::new("tests/fixtures/video-ntsc.mp4"), &first, &policy).unwrap();
    assert_eq!(result.frame_rate, [30000, 1001]);
    assert_eq!(result.frames, 6);
    let roundtrip = convert(&first, &second, &policy).unwrap();
    assert_eq!(roundtrip.frame_rate, result.frame_rate);
    assert_eq!(roundtrip.frames, result.frames);
}

#[test]
fn h264_to_webm_is_local_deterministic_and_preserves_frame_timing() {
    let temp = tempfile::tempdir().unwrap();
    let source = Path::new("tests/fixtures/video-64x48.mp4");
    let first = temp.path().join("first.webm");
    let result = convert(source, &first, &config()).unwrap();
    assert_eq!(
        (
            result.width,
            result.height,
            result.frames,
            result.frame_rate
        ),
        (64, 48, 4, [10, 1])
    );
    assert!(!result.engine_verified);
    let second = temp.path().join("second.webm");
    convert(source, &second, &config()).unwrap();
    let before = fs::read(&first).unwrap();
    assert_eq!(before, fs::read(&second).unwrap());
    assert!(convert(source, &first, &config()).is_err());
    assert_eq!(before, fs::read(&first).unwrap());
    let mut input = ffmpeg_next::format::input(&first).unwrap();
    let stream = input.stream(0).unwrap();
    assert_eq!(stream.parameters().id(), ffmpeg_next::codec::Id::VP9);
    let time_base = stream.time_base();
    let mut decoder = ffmpeg_next::codec::context::Context::from_parameters(stream.parameters())
        .unwrap()
        .decoder()
        .video()
        .unwrap();
    let mut timestamps = Vec::new();
    for (_, packet) in input.packets() {
        decoder.send_packet(&packet).unwrap();
        let mut frame = ffmpeg_next::frame::Video::empty();
        while decoder.receive_frame(&mut frame).is_ok() {
            assert_eq!((frame.width(), frame.height()), (64, 48));
            timestamps.push(frame.timestamp().unwrap());
        }
    }
    decoder.send_eof().unwrap();
    let mut frame = ffmpeg_next::frame::Video::empty();
    while decoder.receive_frame(&mut frame).is_ok() {
        timestamps.push(frame.timestamp().unwrap());
    }
    assert_eq!(timestamps.len(), 4);
    for (index, timestamp) in timestamps.into_iter().enumerate() {
        assert!((timestamp as f64 * f64::from(time_base) - index as f64 / 10.0).abs() < 0.001);
    }
}

#[test]
fn video_limits_bad_input_and_truncation_never_publish_partial_output() {
    let temp = tempfile::tempdir().unwrap();
    let source = Path::new("tests/fixtures/video-64x48.mp4");
    for config in [
        Config {
            max_frames: 3,
            ..config()
        },
        Config {
            max_width: 32,
            ..config()
        },
        Config {
            crf: 64,
            ..config()
        },
    ] {
        let output = temp.path().join("rejected.webm");
        assert!(convert(source, &output, &config).is_err());
        assert!(!output.exists());
    }
    let mut bytes = fs::read(source).unwrap();
    bytes.truncate(bytes.len() / 2);
    let truncated = temp.path().join("truncated.mp4");
    fs::write(&truncated, bytes).unwrap();
    let output = temp.path().join("rejected.webm");
    assert!(convert(&truncated, &output, &config()).is_err());
    assert!(!output.exists());
}

#[test]
fn video_cli_and_bundle_need_no_codec_executable_and_bind_native_video_content() {
    use serde_json::json;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::copy("tests/fixtures/video-64x48.mp4", root.join("video.mp4")).unwrap();
    let config = json!({"maxWidth":64,"maxHeight":48,"maxFrames":4,"bitrate":100000,"crf":20});
    fs::write(root.join("config.json"), config.to_string()).unwrap();
    let cli = std::process::Command::new(env!("CARGO_BIN_EXE_roblox"))
        .env_clear()
        .args(["convert", "video"])
        .arg(root.join("video.mp4"))
        .arg("--config")
        .arg(root.join("config.json"))
        .arg("--output")
        .arg(root.join("video.webm"))
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let response: serde_json::Value = serde_json::from_slice(&cli.stdout).unwrap();
    assert_eq!(response["result"]["frames"], 4);
    let plan = json!({"assets":[{"id":"movie","conversion":{"kind":"video","source":"video.mp4","config":config}}],"scenes":[{"id":"screen","source":"scene.json"}]});
    fs::write(root.join("build.json"), plan.to_string()).unwrap();
    let scene = json!({"kind":"model","roots":[{"id":"screen","class":"VideoFrame","name":"Screen","properties":{},"references":{},"children":[],"assets":{"VideoContent":{"asset":"movie","file":"video.webm"}}}]});
    fs::write(root.join("scene.json"), scene.to_string()).unwrap();
    let output = root.join("bundle");
    let result = roblox_toolchain::bundle::build(&root.join("build.json"), &output).unwrap();
    assert_eq!(
        fs::read(root.join("video.webm")).unwrap(),
        fs::read(output.join(&result.files[0].path)).unwrap()
    );
    let dom = rbx_binary::from_reader(fs::File::open(output.join(&result.scenes[0].path)).unwrap())
        .unwrap();
    let screen = dom.get_by_ref(dom.root().children()[0]).unwrap();
    assert_eq!(
        screen.properties[&"VideoContent".into()],
        rbx_dom_weak::types::Variant::Content(rbx_dom_weak::types::Content::from_uri(
            &result.files[0].local_uri
        ))
    );
}
