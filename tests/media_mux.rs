use roblox_toolchain::{audio, media_mux, video};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
};

fn inputs(root: &Path, offset: u32, max_packets: u64) -> PathBuf {
    video::convert(
        Path::new("tests/fixtures/video-64x48.mp4"),
        &root.join("video.webm"),
        &video::Config {
            max_width: 64,
            max_height: 48,
            max_frames: 4,
            bitrate: 100000,
            crf: 20,
        },
    )
    .unwrap();
    audio::convert(
        Path::new("tests/fixtures/tone-22050.flac"),
        &root.join("audio.ogg"),
        &audio::Config {
            quality: 0.6,
            max_decoded_frames: 4410,
        },
    )
    .unwrap();
    let spec = root.join("media.json");
    fs::write(&spec, json!({"video":"video.webm","audio":"audio.ogg","audioOffsetMillis":offset,"maxPackets":max_packets}).to_string()).unwrap();
    spec
}

fn audio_samples(path: &Path, index: usize) -> Vec<f32> {
    use ffmpeg_next as av;
    let mut input = av::format::input(path).unwrap();
    let parameters = input.stream(index).unwrap().parameters();
    let mut decoder = av::codec::context::Context::from_parameters(parameters)
        .unwrap()
        .decoder()
        .audio()
        .unwrap();
    let mut samples = Vec::new();
    let mut drain = |decoder: &mut av::decoder::Audio| loop {
        let mut frame = av::frame::Audio::empty();
        match decoder.receive_frame(&mut frame) {
            Ok(()) => samples.extend_from_slice(frame.plane::<f32>(0)),
            Err(av::Error::Eof) => break,
            Err(av::Error::Other { errno }) if errno == av::error::EAGAIN => break,
            Err(error) => panic!("audio frame decode failed: {error}"),
        }
    };
    loop {
        let mut packet = av::Packet::empty();
        match packet.read(&mut input) {
            Ok(()) if packet.stream() == index => {
                decoder.send_packet(&packet).unwrap();
                drain(&mut decoder);
            }
            Ok(()) => {}
            Err(av::Error::Eof) => break,
            Err(error) => panic!("audio demux failed: {error}"),
        }
    }
    decoder.send_eof().unwrap();
    drain(&mut decoder);
    samples
}

#[test]
fn mux_preserves_audio_samples_codec_payloads_offsets_and_deterministic_bytes() {
    use ffmpeg_next as av;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let spec = inputs(root, 125, 100);
    let first = root.join("first.webm");
    let manifest = media_mux::build(&spec, &first).unwrap();
    assert_eq!(manifest.video_packets, 4);
    assert!(!manifest.engine_verified);
    let second = root.join("second.webm");
    media_mux::build(&spec, &second).unwrap();
    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
    assert!(media_mux::build(&spec, &first).is_err());
    let source_audio = audio_samples(&root.join("audio.ogg"), 0);
    assert_eq!(source_audio.len(), 4410);
    assert_eq!(source_audio, audio_samples(&first, 1));
    let mut muxed = av::format::input(&first).unwrap();
    assert_eq!(muxed.nb_streams(), 2);
    assert_eq!(
        muxed.stream(0).unwrap().parameters().id(),
        av::codec::Id::VP9
    );
    assert_eq!(
        muxed.stream(1).unwrap().parameters().id(),
        av::codec::Id::VORBIS
    );
    let mut packets: [Vec<(f64, Vec<u8>)>; 2] = [Vec::new(), Vec::new()];
    for (stream, packet) in muxed.packets() {
        packets[stream.index()].push((
            packet.pts().unwrap() as f64 * f64::from(stream.time_base()),
            packet.data().unwrap().to_vec(),
        ));
    }
    for (index, name) in ["video.webm", "audio.ogg"].into_iter().enumerate() {
        let mut original = av::format::input(&root.join(name)).unwrap();
        let original: Vec<_> = original
            .packets()
            .map(|(stream, packet)| {
                (
                    packet.pts().unwrap() as f64 * f64::from(stream.time_base()),
                    packet.data().unwrap().to_vec(),
                )
            })
            .collect();
        assert_eq!(packets[index].len(), original.len());
        for ((pts, data), (original_pts, original_data)) in packets[index].iter().zip(&original) {
            assert_eq!(data, original_data);
            let offset = if index == 1 { 0.125 } else { 0.0 };
            assert!((pts - original_pts - offset).abs() <= 0.001);
        }
    }
}

#[test]
fn mux_policy_and_path_failures_leave_inputs_intact_and_no_partial_output() {
    let temp = tempfile::tempdir().unwrap();
    let spec = inputs(temp.path(), 0, 1);
    let output = temp.path().join("rejected.webm");
    assert!(media_mux::build(&spec, &output).is_err());
    assert!(!output.exists());
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&spec).unwrap()).unwrap();
    value["maxPackets"] = 100.into();
    value["audio"] = "../outside.ogg".into();
    fs::write(&spec, value.to_string()).unwrap();
    assert!(media_mux::build(&spec, &output).is_err());
    assert!(!output.exists());
    assert!(temp.path().join("audio.ogg").is_file());
    assert!(temp.path().join("video.webm").is_file());
}

#[test]
fn media_cli_and_bundle_preserve_identical_preconverted_outputs_offline() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let spec = inputs(root, 0, 100);
    let standalone = root.join("combined.webm");
    let cli = std::process::Command::new(env!("CARGO_BIN_EXE_roblox"))
        .env_clear()
        .args(["build", "media"])
        .arg(&spec)
        .arg("--output")
        .arg(&standalone)
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&cli.stdout).unwrap();
    assert_eq!(result["scope"], "offlineMediaBuild");
    let plan = json!({"assets":[{"id":"combined","conversion":{"kind":"media","source":"media.json"}}],"scenes":[]});
    fs::write(root.join("build.json"), plan.to_string()).unwrap();
    let bundle = root.join("bundle");
    let result = roblox_toolchain::bundle::build(&root.join("build.json"), &bundle).unwrap();
    assert_eq!(result.files.len(), 1);
    assert_eq!(
        fs::read(standalone).unwrap(),
        fs::read(bundle.join(&result.files[0].path)).unwrap()
    );
}

#[test]
fn opus_codec_delay_and_final_padding_survive_webm_mux() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let spec = inputs(root, 0, 100);
    fs::copy("tests/fixtures/tone-opus.ogg", root.join("audio.ogg")).unwrap();
    let output = root.join("opus.webm");
    media_mux::build(&spec, &output).unwrap();
    let source = audio_samples(&root.join("audio.ogg"), 0);
    let target = audio_samples(&output, 1);
    assert_eq!(source.len(), 9600);
    assert_eq!(target, source);
}
