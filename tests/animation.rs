use rbx_dom_weak::types::{CFrame, Variant};
use roblox_toolchain::animation::{Clip, Frame, Marker, Pose, encode};

fn clip() -> Clip {
    Clip {
        name: "Wave".into(),
        looped: true,
        priority: "Action".into(),
        frames: vec![Frame {
            name: "Start".into(),
            time: 0.,
            poses: vec![Pose {
                name: "Root".into(),
                cframe: [1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0.],
                weight: 1.,
                easing_style: "Linear".into(),
                easing_direction: "In".into(),
                children: vec![],
            }],
            markers: vec![Marker {
                name: "Event".into(),
                value: "value".into(),
            }],
        }],
    }
}

#[test]
fn native_animation_preserves_poses_markers_properties_and_bytes() {
    let source = clip();
    let bytes = encode(&source).unwrap();
    assert_eq!(bytes, encode(&source).unwrap());
    let dom = rbx_binary::from_reader(bytes.as_slice()).unwrap();
    let sequence = dom.get_by_ref(dom.root().children()[0]).unwrap();
    assert_eq!(sequence.class.as_str(), "KeyframeSequence");
    assert_eq!(sequence.name, "Wave");
    assert_eq!(
        sequence.properties.get(&"Loop".into()),
        Some(&Variant::Bool(true))
    );
    let frame = dom.get_by_ref(sequence.children()[0]).unwrap();
    assert_eq!(
        frame.properties.get(&"Time".into()),
        Some(&Variant::Float32(0.))
    );
    let pose = dom.get_by_ref(frame.children()[0]).unwrap();
    assert_eq!(pose.name, "Root");
    assert_eq!(
        pose.properties.get(&"CFrame".into()),
        Some(&Variant::CFrame(CFrame::identity()))
    );
    let marker = dom.get_by_ref(frame.children()[1]).unwrap();
    assert_eq!(marker.class.as_str(), "KeyframeMarker");
    assert_eq!(
        marker.properties.get(&"Value".into()),
        Some(&Variant::String("value".into()))
    );
}

#[test]
fn invalid_times_enums_and_pose_data_fail() {
    let mut source = clip();
    source.frames[0].time = f32::NAN;
    assert!(encode(&source).is_err());
    source = clip();
    source.priority = "Imaginary".into();
    assert!(encode(&source).is_err());
    source = clip();
    source.frames[0].poses[0].weight = 2.;
    assert!(encode(&source).is_err());
    source = clip();
    source.frames.push(clip().frames.remove(0));
    assert!(encode(&source).is_err());
}

#[test]
fn cli_converts_canonical_animation_and_preserves_existing_outputs() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("clip.json");
    let output = directory.path().join("clip.rbxm");
    let input = serde_json::to_vec(&clip()).unwrap();
    std::fs::write(&source, &input).unwrap();
    let run = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_roblox"))
            .env_clear()
            .args(["convert", "animation"])
            .arg(&source)
            .arg("--output")
            .arg(&output)
            .output()
            .unwrap()
    };

    let result = run();
    assert!(result.status.success(), "{:?}", result.stderr);
    let response: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(response["result"]["format"], "rbxm-keyframe-sequence");
    assert_eq!(response["result"]["engineVerified"], false);
    let bytes = std::fs::read(&output).unwrap();
    assert_eq!(bytes, encode(&clip()).unwrap());

    let second = run();
    assert!(!second.status.success());
    assert!(serde_json::from_slice::<serde_json::Value>(&second.stderr).is_ok());
    assert_eq!(std::fs::read(&output).unwrap(), bytes);
    assert_eq!(std::fs::read(&source).unwrap(), input);
}

#[test]
fn invalid_animation_never_creates_an_output() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("clip.json");
    let output = directory.path().join("clip.rbxm");
    let mut invalid = clip();
    invalid.priority = "not-an-animation-priority".into();
    std::fs::write(&source, serde_json::to_vec(&invalid).unwrap()).unwrap();

    assert!(roblox_toolchain::animation::convert(&source, &output).is_err());
    assert!(!output.exists());
    assert!(roblox_toolchain::animation::convert(&source, &output.with_extension("glb")).is_err());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn bundle_links_animation_and_rolls_back_invalid_clips() {
    use serde_json::json;

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::write(root.join("clip.json"), serde_json::to_vec(&clip()).unwrap()).unwrap();
    let scene = json!({"kind":"model", "roots":[{
        "id":"animation", "class":"Animation", "name":"Wave",
        "properties":{}, "references":{}, "children":[],
        "assets":{"AnimationContent":{"asset":"wave","file":"animation.rbxm"}}
    }]});
    std::fs::write(root.join("scene.json"), serde_json::to_vec(&scene).unwrap()).unwrap();
    let plan = json!({
        "assets":[{"id":"wave","conversion":{"kind":"animation","source":"clip.json"}}],
        "scenes":[{"id":"scene","source":"scene.json"}]
    });
    let source = root.join("build.json");
    std::fs::write(&source, serde_json::to_vec(&plan).unwrap()).unwrap();

    let output = root.join("bundle");
    let manifest = roblox_toolchain::bundle::build(&source, &output).unwrap();
    assert_eq!(manifest.files.len(), 1);
    let artifact = &manifest.files[0];
    assert_eq!(
        std::fs::read(output.join(&artifact.path)).unwrap(),
        encode(&clip()).unwrap()
    );
    let scene_bytes = std::fs::read(output.join(&manifest.scenes[0].path)).unwrap();
    let dom = rbx_binary::from_reader(scene_bytes.as_slice()).unwrap();
    let animation = dom.get_by_ref(dom.root().children()[0]).unwrap();
    assert_eq!(
        animation.properties.get(&"AnimationContent".into()),
        Some(&Variant::Content(rbx_dom_weak::types::Content::from_uri(
            &artifact.local_uri
        )))
    );

    let second = root.join("second");
    let repeated = roblox_toolchain::bundle::build(&source, &second).unwrap();
    assert_eq!(
        serde_json::to_vec(&manifest).unwrap(),
        serde_json::to_vec(&repeated).unwrap()
    );

    std::fs::write(root.join("clip.json"), b"invalid").unwrap();
    let failed = root.join("failed");
    assert!(roblox_toolchain::bundle::build(&source, &failed).is_err());
    assert!(!failed.exists());
    assert!(output.exists());
}
