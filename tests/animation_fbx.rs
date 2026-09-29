use glam::Mat4;
use rbx_dom_weak::{
    WeakDom,
    types::{CFrame, Ref, Variant},
};
use roblox_toolchain::animation_fbx::{self, Config};
use std::{collections::BTreeMap, fs, path::Path};

fn source() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/maya-wiggle.fbx")
}

fn scene() -> ufbx::SceneRoot {
    ufbx::load_memory(
        &fs::read(source()).unwrap(),
        ufbx::LoadOpts {
            file_format: ufbx::FileFormat::Fbx,
            target_axes: ufbx::CoordinateAxes::right_handed_y_up(),
            target_unit_meters: 1.,
            space_conversion: ufbx::SpaceConversion::ModifyGeometry,
            load_external_files: false,
            ..Default::default()
        },
    )
    .unwrap()
}

fn config(scene: &ufbx::Scene) -> Config {
    let root = scene
        .nodes
        .iter()
        .find(|node| {
            node.bone.is_some()
                && node
                    .parent
                    .as_ref()
                    .is_none_or(|parent| parent.bone.is_none())
        })
        .unwrap();
    Config {
        animation_index: 0,
        root_node: root.element.typed_id as usize,
        metres_per_stud: 0.28,
        rigid_tolerance: 0.0001,
        resample_rate: 30.,
        minimum_sample_rate: 60.,
        max_keyframe_segments: 50000,
        max_output_frames: 10000,
        name: "Wiggle".into(),
        looped: true,
        priority: "Action".into(),
    }
}

fn transform(value: [f32; 12]) -> Mat4 {
    Mat4::from_cols_array(&[
        value[0], value[3], value[6], 0., value[1], value[4], value[7], 0., value[2], value[5],
        value[8], 0., value[9], value[10], value[11], 1.,
    ])
}

fn native(value: &CFrame) -> Mat4 {
    let r = value.orientation;
    let p = value.position;
    transform([
        r.x.x, r.x.y, r.x.z, r.y.x, r.y.y, r.y.z, r.z.x, r.z.y, r.z.z, p.x, p.y, p.z,
    ])
}

fn compare_pose(
    dom: &WeakDom,
    reference: Ref,
    rig: &BTreeMap<&str, (usize, Mat4)>,
    scene: &ufbx::Scene,
    time: f64,
    scale: f32,
    parents: (Mat4, Mat4),
) {
    let (native_parent, source_parent) = parents;
    let pose = dom.get_by_ref(reference).unwrap();
    let (node, rest) = rig[pose.name.as_str()];
    let Variant::CFrame(offset) = &pose.properties[&"CFrame".into()] else {
        panic!("missing offset");
    };
    let actual = rest * native(offset);
    let evaluated = ufbx::evaluate_transform(&scene.anim_stacks[0].anim, &scene.nodes[node], time);
    let expected = ufbx::transform_to_matrix(&evaluated);
    let expected = Mat4::from_cols_array(&[
        expected.m00 as f32,
        expected.m10 as f32,
        expected.m20 as f32,
        0.,
        expected.m01 as f32,
        expected.m11 as f32,
        expected.m21 as f32,
        0.,
        expected.m02 as f32,
        expected.m12 as f32,
        expected.m22 as f32,
        0.,
        expected.m03 as f32 / scale,
        expected.m13 as f32 / scale,
        expected.m23 as f32 / scale,
        1.,
    ]);
    assert!(
        actual.abs_diff_eq(expected, 0.002),
        "pose {} at {time}: native {actual:?}, source {expected:?}",
        pose.name
    );
    let native_world = native_parent * actual;
    let source_world = source_parent * expected;
    assert!(native_world.abs_diff_eq(source_world, 0.002));
    for child in pose.children() {
        compare_pose(
            dom,
            *child,
            rig,
            scene,
            time,
            scale,
            (native_world, source_world),
        );
    }
}

#[test]
fn fbx_clip_native_poses_match_source_evaluation_and_are_deterministic() {
    let scene = scene();
    let policy = config(&scene);
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("animation.rbxm");
    let manifest = animation_fbx::convert(&source(), &output, policy.clone()).unwrap();
    assert!(manifest.sampled_approximation);
    assert!(
        manifest
            .unchanged_properties
            .iter()
            .any(|property| property.ends_with(".Visibility"))
    );
    assert!(manifest.artifact.keyframes > 2);
    let dom = rbx_binary::from_reader(fs::read(&output).unwrap().as_slice()).unwrap();
    let sequence = dom.get_by_ref(dom.root().children()[0]).unwrap();
    let mut parent = Mat4::IDENTITY;
    if let Some(source_parent) = &scene.nodes[policy.root_node].parent {
        let m = &source_parent.node_to_world;
        parent = Mat4::from_cols_array(&[
            m.m00 as f32,
            m.m10 as f32,
            m.m20 as f32,
            0.,
            m.m01 as f32,
            m.m11 as f32,
            m.m21 as f32,
            0.,
            m.m02 as f32,
            m.m12 as f32,
            m.m22 as f32,
            0.,
            m.m03 as f32 / policy.metres_per_stud,
            m.m13 as f32 / policy.metres_per_stud,
            m.m23 as f32 / policy.metres_per_stud,
            1.,
        ]);
    }
    let native_parent = transform(manifest.rig.root_parent_cframe);
    assert!(native_parent.abs_diff_eq(parent, policy.rigid_tolerance));
    assert_eq!(manifest.rig_sha256, manifest.rig.sha256().unwrap());
    let first = dom.get_by_ref(sequence.children()[0]).unwrap();
    let last = dom
        .get_by_ref(*sequence.children().last().unwrap())
        .unwrap();
    assert_eq!(first.properties[&"Time".into()], Variant::Float32(0.));
    assert_eq!(
        last.properties[&"Time".into()],
        Variant::Float32((scene.anim_stacks[0].time_end - manifest.source_time_begin) as f32)
    );
    let rig = manifest
        .rig
        .joints
        .iter()
        .map(|bone| {
            (
                bone.name.as_str(),
                (bone.node_index, transform(bone.rest_cframe)),
            )
        })
        .collect();
    for frame in sequence.children() {
        let frame = dom.get_by_ref(*frame).unwrap();
        let Variant::Float32(time) = frame.properties[&"Time".into()] else {
            panic!("missing time");
        };
        compare_pose(
            &dom,
            frame.children()[0],
            &rig,
            &scene,
            f64::from(time) + manifest.source_time_begin,
            policy.metres_per_stud,
            (native_parent, parent),
        );
    }
    let repeated = directory.path().join("repeated.rbxm");
    let second = animation_fbx::convert(&source(), &repeated, policy).unwrap();
    assert_eq!(fs::read(output).unwrap(), fs::read(repeated).unwrap());
    assert_eq!(manifest.rig_sha256, second.rig_sha256);
}

#[test]
fn explicit_fbx_sampling_budgets_are_enforced_without_partial_output() {
    let scene = scene();
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("bad.rbxm");
    let mut policy = config(&scene);
    policy.max_output_frames = 1;
    assert!(
        animation_fbx::convert(&source(), &output, policy)
            .err()
            .unwrap()
            .to_string()
            .contains("maxOutputFrames")
    );
    assert!(!output.exists());
    let mut policy = config(&scene);
    policy.resample_rate = 0.;
    assert!(animation_fbx::convert(&source(), &output, policy).is_err());
    assert!(!output.exists());
}

#[test]
fn fbx_cli_bundle_and_sampling_rate_are_data_driven() {
    let scene = scene();
    let policy = config(&scene);
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::copy(source(), root.join("source.fbx")).unwrap();
    fs::write(
        root.join("config.json"),
        serde_json::to_vec(&policy).unwrap(),
    )
    .unwrap();
    let output = root.join("cli.rbxm");
    let run = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_roblox"))
            .env_clear()
            .args(["convert", "animation-fbx"])
            .arg(root.join("source.fbx"))
            .arg("--config")
            .arg(root.join("config.json"))
            .arg("--output")
            .arg(&output)
            .output()
            .unwrap()
    };
    let result = run();
    assert!(result.status.success(), "{:?}", result.stderr);
    let response: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    let bytes = fs::read(&output).unwrap();
    assert!(!run().status.success());
    assert_eq!(bytes, fs::read(&output).unwrap());
    let plan = serde_json::json!({"assets":[{"id":"motion","conversion":{"kind":"animationFbx","source":"source.fbx","config":policy}}],"scenes":[]});
    fs::write(root.join("build.json"), serde_json::to_vec(&plan).unwrap()).unwrap();
    let bundle = root.join("bundle");
    let manifest = roblox_toolchain::bundle::build(&root.join("build.json"), &bundle).unwrap();
    assert_eq!(
        bytes,
        fs::read(bundle.join(&manifest.files[0].path)).unwrap()
    );
    let mut higher = policy;
    higher.resample_rate = 120.;
    let sampled = animation_fbx::convert(&source(), &root.join("higher.rbxm"), higher).unwrap();
    assert!(
        sampled.artifact.keyframes as u64
            > response["result"]["artifact"]["keyframes"]
                .as_u64()
                .unwrap()
    );
}
