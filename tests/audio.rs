use roblox_toolchain::audio::{Config, convert};
use std::{fs, io::Cursor};

fn wav(frames: u32, channels: u16) -> Vec<u8> {
    let rate = 16000_u32;
    let data_len = frames * u32::from(channels) * 2;
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
    bytes.extend_from_slice(&(channels * 2).to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for frame in 0..frames {
        let value =
            ((frame as f32 * 400. * std::f32::consts::TAU / rate as f32).sin() * 12000.) as i16;
        for _ in 0..channels {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    bytes
}

#[test]
fn wav_to_vorbis_is_deterministic_and_preserves_frames_rate_and_channels() {
    let temporary = tempfile::tempdir().unwrap();
    for channels in [1, 2] {
        let source = temporary.path().join(format!("{channels}.wav"));
        fs::write(&source, wav(4096, channels)).unwrap();
        let first = temporary.path().join(format!("{channels}.ogg"));
        let second = temporary.path().join(format!("{channels}-repeat.ogg"));
        let config = Config {
            quality: 0.4,
            max_decoded_frames: 4096,
        };
        let result = convert(&source, &first, &config).unwrap();
        assert_eq!(result.frames, 4096);
        assert_eq!(result.sample_rate, 16000);
        assert_eq!(result.channels, u8::try_from(channels).unwrap());
        convert(&source, &second, &config).unwrap();
        let bytes = fs::read(&first).unwrap();
        assert_eq!(bytes, fs::read(&second).unwrap());
        let mut decoder = vorbis_rs::VorbisDecoder::new(Cursor::new(&bytes)).unwrap();
        assert_eq!(decoder.sampling_frequency().get(), 16000);
        assert_eq!(decoder.channels().get(), result.channels);
        let mut samples = Vec::new();
        while let Some(block) = decoder.decode_audio_block().unwrap() {
            samples.extend_from_slice(block.samples()[0]);
        }
        assert_eq!(samples.len(), 4096);
        let mse: f32 = samples
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let expected =
                    (index as f32 * 400. * std::f32::consts::TAU / 16000.).sin() * 12000. / 32768.;
                (value - expected).powi(2)
            })
            .sum::<f32>()
            / samples.len() as f32;
        assert!(mse < 0.001, "audio distortion {mse}");
        let transcoded = temporary.path().join(format!("{channels}-transcoded.ogg"));
        let reconverted = convert(&first, &transcoded, &config).unwrap();
        assert_eq!(reconverted.frames, 4096);
    }
}

#[test]
fn audio_limits_truncation_and_invalid_settings_fail_without_artifacts() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("input.wav");
    let output = temporary.path().join("out.ogg");
    fs::write(&source, wav(4096, 1)).unwrap();
    assert!(
        convert(
            &source,
            &output,
            &Config {
                quality: 0.3,
                max_decoded_frames: 4095
            }
        )
        .is_err()
    );
    assert!(!output.exists());
    assert!(
        convert(
            &source,
            &output,
            &Config {
                quality: 2.,
                max_decoded_frames: 4096
            }
        )
        .is_err()
    );
    let mut bytes = wav(4096, 1);
    bytes.truncate(bytes.len() - 100);
    fs::write(&source, bytes).unwrap();
    assert!(
        convert(
            &source,
            &output,
            &Config {
                quality: 0.3,
                max_decoded_frames: 4096
            }
        )
        .is_err()
    );
    assert!(!output.exists());
}

#[test]
fn audio_cli_runs_without_credentials_or_external_codec_tools() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("input.wav");
    fs::write(&source, wav(1024, 1)).unwrap();
    let config = temporary.path().join("audio.json");
    fs::write(&config, r#"{"quality":0.3,"maxDecodedFrames":1024}"#).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_roblox"))
        .env_clear()
        .args(["convert", "audio"])
        .arg(source)
        .arg("--config")
        .arg(config)
        .arg("--output")
        .arg(temporary.path().join("out.ogg"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["result"]["format"], "ogg-vorbis");
}

#[test]
fn independently_encoded_flac_preserves_duration_and_waveform_and_detects_truncation() {
    let temp = tempfile::tempdir().unwrap();
    let source = std::path::Path::new("tests/fixtures/tone-22050.flac");
    let output = temp.path().join("tone.ogg");
    let config = Config {
        quality: 0.6,
        max_decoded_frames: 4410,
    };
    let result = convert(source, &output, &config).unwrap();
    assert_eq!(
        (result.sample_rate, result.channels, result.frames),
        (22050, 1, 4410)
    );
    assert_eq!(result.padding_frames_removed, 0);
    let mut decoder =
        vorbis_rs::VorbisDecoder::new(Cursor::new(fs::read(&output).unwrap())).unwrap();
    let mut samples = Vec::new();
    while let Some(block) = decoder.decode_audio_block().unwrap() {
        samples.extend_from_slice(block.samples()[0]);
    }
    assert_eq!(samples.len(), 4410);
    let error = samples
        .iter()
        .enumerate()
        .map(|(i, sample)| {
            let expected = (std::f64::consts::TAU * 440.0 * i as f64 / 22050.0).sin() * 0.125;
            (*sample as f64 - expected).powi(2)
        })
        .sum::<f64>()
        / samples.len() as f64;
    assert!(error < 0.0001, "FLAC/Vorbis waveform MSE: {error}");
    let repeated = temp.path().join("repeated.ogg");
    convert(source, &repeated, &config).unwrap();
    assert_eq!(fs::read(&output).unwrap(), fs::read(&repeated).unwrap());

    let limited = Config {
        quality: 0.6,
        max_decoded_frames: 4409,
    };
    let rejected = temp.path().join("rejected.ogg");
    assert!(convert(source, &rejected, &limited).is_err());
    assert!(!rejected.exists());
    let mut bytes = fs::read(source).unwrap();
    bytes.truncate(bytes.len() - 30);
    let truncated = temp.path().join("truncated.flac");
    fs::write(&truncated, bytes).unwrap();
    assert!(convert(&truncated, &rejected, &config).is_err());
    assert!(!rejected.exists());
}
