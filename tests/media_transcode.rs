use roblox_toolchain::{audio, media_transcode, video};
use std::{fs, path::Path};

fn config() -> media_transcode::Config {
    media_transcode::Config {
        video: video::Config {
            max_width: 64,
            max_height: 48,
            max_frames: 4,
            bitrate: 100000,
            crf: 20,
        },
        audio: audio::Config {
            quality: 0.6,
            max_decoded_frames: 8820,
        },
        max_packets: 100,
    }
}

#[test]
fn combined_mp4_retains_video_and_aac_audio_without_subprocesses() {
    let temp = tempfile::tempdir().unwrap();
    let source = Path::new("tests/fixtures/video-aac.mp4");
    let output = temp.path().join("combined.webm");
    let result = media_transcode::convert(source, &output, &config()).unwrap();
    assert_eq!(result.video.frames, 4);
    assert_eq!(result.audio.frames, 8820);
    assert_eq!(result.audio.sample_rate, 22050);
    assert_eq!(result.source_audio_padding_frames_removed, 396);
    assert_eq!(result.combined.audio_offset_millis, 0);
    let repeated = temp.path().join("repeated.webm");
    media_transcode::convert(source, &repeated, &config()).unwrap();
    assert_eq!(fs::read(&output).unwrap(), fs::read(repeated).unwrap());
    let input = ffmpeg_next::format::input(&output).unwrap();
    assert_eq!(input.nb_streams(), 2);
    assert_eq!(
        input.stream(0).unwrap().parameters().id(),
        ffmpeg_next::codec::Id::VP9
    );
    assert_eq!(
        input.stream(1).unwrap().parameters().id(),
        ffmpeg_next::codec::Id::VORBIS
    );
    assert!(media_transcode::convert(source, &output, &config()).is_err());
    let mut input = ffmpeg_next::format::input(&output).unwrap();
    let parameters = input.stream(1).unwrap().parameters();
    let mut decoder = ffmpeg_next::codec::context::Context::from_parameters(parameters)
        .unwrap()
        .decoder()
        .audio()
        .unwrap();
    let mut samples = Vec::new();
    let mut drain = |decoder: &mut ffmpeg_next::decoder::Audio| loop {
        let mut frame = ffmpeg_next::frame::Audio::empty();
        match decoder.receive_frame(&mut frame) {
            Ok(()) => samples.extend_from_slice(frame.plane::<f32>(0)),
            Err(ffmpeg_next::Error::Eof) => break,
            Err(ffmpeg_next::Error::Other { errno }) if errno == ffmpeg_next::error::EAGAIN => {
                break;
            }
            Err(error) => panic!("audio decode failed: {error}"),
        }
    };
    loop {
        let mut packet = ffmpeg_next::Packet::empty();
        match packet.read(&mut input) {
            Ok(()) if packet.stream() == 1 => {
                decoder.send_packet(&packet).unwrap();
                drain(&mut decoder);
            }
            Ok(()) => {}
            Err(ffmpeg_next::Error::Eof) => break,
            Err(error) => panic!("demux failed: {error}"),
        }
    }
    decoder.send_eof().unwrap();
    drain(&mut decoder);
    assert_eq!(samples.len(), 8820);
    let mse = samples
        .iter()
        .enumerate()
        .map(|(index, sample)| {
            let expected = (std::f64::consts::TAU * 440.0 * index as f64 / 22050.0).sin() * 0.125;
            (*sample as f64 - expected).powi(2)
        })
        .sum::<f64>()
        / samples.len() as f64;
    assert!(mse < 0.0001, "combined audio distortion: {mse}");
}

#[test]
fn combined_source_cli_and_bundle_are_equivalent_without_external_tools() {
    use serde_json::json;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::copy("tests/fixtures/video-aac.mp4", root.join("source.mp4")).unwrap();
    let config = json!({"video":{"maxWidth":64,"maxHeight":48,"maxFrames":4,"bitrate":100000,"crf":20},"audio":{"quality":0.6,"maxDecodedFrames":8820},"maxPackets":100});
    fs::write(root.join("config.json"), config.to_string()).unwrap();
    let cli = std::process::Command::new(env!("CARGO_BIN_EXE_roblox"))
        .env_clear()
        .args(["convert", "media"])
        .arg(root.join("source.mp4"))
        .arg("--config")
        .arg(root.join("config.json"))
        .arg("--output")
        .arg(root.join("cli.webm"))
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let response: serde_json::Value = serde_json::from_slice(&cli.stdout).unwrap();
    assert_eq!(response["result"]["audio"]["frames"], 8820);
    let plan = json!({"assets":[{"id":"clip","conversion":{"kind":"mediaSource","source":"source.mp4","config":config}}],"scenes":[]});
    fs::write(root.join("build.json"), plan.to_string()).unwrap();
    let bundle = root.join("bundle");
    let result = roblox_toolchain::bundle::build(&root.join("build.json"), &bundle).unwrap();
    assert_eq!(
        fs::read(root.join("cli.webm")).unwrap(),
        fs::read(bundle.join(&result.files[0].path)).unwrap()
    );
}

#[test]
fn combined_conversion_does_not_drop_audio_or_leave_partial_failures() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("rejected.webm");
    let mut policy = config();
    policy.audio.max_decoded_frames = 8819;
    assert!(
        media_transcode::convert(Path::new("tests/fixtures/video-aac.mp4"), &output, &policy)
            .is_err()
    );
    assert!(!output.exists());
    assert!(
        media_transcode::convert(
            Path::new("tests/fixtures/video-64x48.mp4"),
            &output,
            &config()
        )
        .is_err()
    );
    assert!(!output.exists());
    assert!(
        video::convert(
            Path::new("tests/fixtures/video-aac.mp4"),
            &output,
            &config().video
        )
        .is_err()
    );
    assert!(!output.exists());
}
