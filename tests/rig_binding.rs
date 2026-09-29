use rbx_dom_weak::types::{CFrame, Matrix3, Variant, Vector3};
use roblox_toolchain::{
    animation::{Rig, RigBinding},
    animation_gltf, bundle,
};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn matrix(c: [f32; 12]) -> glam::Mat4 {
    glam::Mat4::from_cols_array(&[
        c[0], c[3], c[6], 0., c[1], c[4], c[7], 0., c[2], c[5], c[8], 0., c[9], c[10], c[11], 1.,
    ])
}

fn native(m: glam::Mat4) -> CFrame {
    CFrame::new(
        Vector3::new(m.w_axis.x, m.w_axis.y, m.w_axis.z),
        Matrix3::new(
            Vector3::new(m.x_axis.x, m.y_axis.x, m.z_axis.x),
            Vector3::new(m.x_axis.y, m.y_axis.y, m.z_axis.y),
            Vector3::new(m.x_axis.z, m.y_axis.z, m.z_axis.z),
        ),
    )
}

fn bone(rig: &Rig, joint: &RigBinding, fold_parent: bool) -> Value {
    let rest = matrix(joint.rest_cframe);
    let rest = if joint.parent_node.is_none() && fold_parent {
        matrix(rig.root_parent_cframe) * rest
    } else {
        rest
    };
    let children: Vec<_> = rig
        .joints
        .iter()
        .filter(|candidate| candidate.parent_node == Some(joint.node_index))
        .map(|child| bone(rig, child, fold_parent))
        .collect();
    json!({"id":format!("joint-{}",joint.node_index),"class":"Bone","name":joint.name,"properties":{"CFrame":Variant::CFrame(native(rest))},"references":{},"children":children})
}

fn inputs(root: &Path, fold_parent: bool) -> (std::path::PathBuf, Value) {
    let source = Path::new("tests/fixtures/rigged-simple.glb");
    let config = animation_gltf::Config {
        animation_index: 0,
        root_node: 3,
        metres_per_stud: 0.5,
        rigid_tolerance: 0.00001,
        name: "Motion".into(),
        looped: true,
        priority: "Action".into(),
    };
    let (_, rig) = animation_gltf::read(source, &config).unwrap();
    let joint = rig.joints.iter().find(|j| j.parent_node.is_none()).unwrap();
    let mut scene = json!({"kind":"model","animationBindings":[{"animation":{"asset":"motion","file":"animation.rbxm"},"root":format!("joint-{}",joint.node_index),"rigidTolerance":0.00001}],
        "roots":[{"id":"mesh","class":"MeshPart","name":"Mesh","properties":{},"references":{},"children":[bone(&rig,joint,fold_parent)]}]});
    scene["roots"].as_array_mut().unwrap().push(json!({"id":"animation","class":"Animation","name":"Motion","properties":{},"references":{},"children":[],"assets":{"AnimationContent":{"asset":"motion","file":"animation.rbxm"}}}));
    fs::copy(source, root.join("source.glb")).unwrap();
    let plan = json!({"assets":[{"id":"motion","conversion":{"kind":"animationGltf","source":"source.glb","config":config}}],"scenes":[{"id":"rig","source":"scene.json"}]});
    let path = root.join("build.json");
    fs::write(&path, plan.to_string()).unwrap();
    fs::write(root.join("scene.json"), scene.to_string()).unwrap();
    (path, scene)
}

#[test]
fn animation_binding_checks_the_actual_scene_rest_space_including_root_context() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let (source, _) = inputs(root, true);
    let output = root.join("valid");
    let result = bundle::build(&source, &output).unwrap();
    assert_eq!(result.scenes.len(), 1);
    let repeated = root.join("repeat");
    bundle::build(&source, &repeated).unwrap();
    assert_eq!(
        fs::read(output.join(&result.scenes[0].path)).unwrap(),
        fs::read(repeated.join(&result.scenes[0].path)).unwrap()
    );
    inputs(root, false);
    let rejected = root.join("rejected");
    let error = bundle::build(&source, &rejected).err().unwrap().to_string();
    assert!(error.contains("rest-space mismatch"), "{error}");
    assert!(!rejected.exists());
}

#[test]
fn animation_binding_rejects_wrong_names_counts_parents_assets_and_tolerance() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let (source, scene) = inputs(root, true);
    for (pointer, value) in [
        ("/roots/0/children/0/name", json!("Wrong")),
        ("/roots/0/children/0/children", json!([])),
        ("/roots/0/class", json!("Bone")),
        ("/animationBindings/0/rigidTolerance", json!(0)),
        ("/animationBindings/0/animation/file", json!("missing.rbxm")),
        ("/animationBindings/0/root", json!("missing-root")),
        ("/animationBindings", json!([])),
    ] {
        let mut invalid = scene.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        fs::write(root.join("scene.json"), invalid.to_string()).unwrap();
        let out = root.join("rejected");
        assert!(bundle::build(&source, &out).is_err(), "{pointer}");
        assert!(!out.exists());
    }
}

#[test]
fn fbx_rig_metadata_reaches_scene_validation_and_wrong_reparenting_fails() {
    use roblox_toolchain::animation_fbx;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let source = Path::new("tests/fixtures/maya-wiggle.fbx");
    let parsed = ufbx::load_memory(
        &fs::read(source).unwrap(),
        ufbx::LoadOpts {
            load_external_files: false,
            ..Default::default()
        },
    )
    .unwrap();
    let root_node = parsed
        .nodes
        .iter()
        .find(|n| n.bone.is_some() && n.parent.as_ref().is_none_or(|p| p.bone.is_none()))
        .unwrap();
    let config = animation_fbx::Config {
        animation_index: 0,
        root_node: root_node.element.typed_id as usize,
        metres_per_stud: 0.28,
        rigid_tolerance: 0.0001,
        resample_rate: 30.,
        minimum_sample_rate: 60.,
        max_keyframe_segments: 50000,
        max_output_frames: 10000,
        name: "Wiggle".into(),
        looped: true,
        priority: "Action".into(),
    };
    let converted =
        animation_fbx::convert(source, &root.join("reference.rbxm"), config.clone()).unwrap();
    let rig = converted.rig;
    let joint = rig.joints.iter().find(|j| j.parent_node.is_none()).unwrap();
    let mut scene = json!({"kind":"model","animationBindings":[{"animation":{"asset":"motion","file":"animation.rbxm"},"root":format!("joint-{}",joint.node_index),"rigidTolerance":0.0001}],
        "roots":[{"id":"mesh","class":"MeshPart","name":"Mesh","properties":{},"references":{},"children":[bone(&rig,joint,true)]}]});
    fs::copy(source, root.join("source.fbx")).unwrap();
    let plan = json!({"assets":[{"id":"motion","conversion":{"kind":"animationFbx","source":"source.fbx","config":config}}],"scenes":[{"id":"rig","source":"scene.json"}]});
    fs::write(root.join("build.json"), plan.to_string()).unwrap();
    fs::write(root.join("scene.json"), scene.to_string()).unwrap();
    bundle::build(&root.join("build.json"), &root.join("valid")).unwrap();
    let grandchild = scene["roots"][0]["children"][0]["children"][0]["children"]
        .as_array_mut()
        .unwrap()
        .remove(0);
    scene["roots"][0]["children"][0]["children"]
        .as_array_mut()
        .unwrap()
        .push(grandchild);
    fs::write(root.join("scene.json"), scene.to_string()).unwrap();
    let out = root.join("rejected");
    let error = bundle::build(&root.join("build.json"), &out)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("hierarchy mismatch"), "{error}");
    assert!(!out.exists());

    let mesh_node = parsed
        .nodes
        .iter()
        .find(|node| {
            node.mesh
                .as_ref()
                .is_some_and(|mesh| !mesh.skin_deformers.is_empty())
        })
        .unwrap()
        .element
        .typed_id as usize;
    let skin_config = roblox_toolchain::skin_import::Config {
        mesh_node,
        metres_per_stud: 0.28,
        cull_distance_metres: 30.,
        rigid_tolerance: 0.0001,
    };
    let skin =
        roblox_toolchain::skin_import::convert(source, &root.join("skin-reference"), &skin_config)
            .unwrap();
    let mut bound_plan = plan.clone();
    bound_plan["assets"][0]["conversion"]["bindTo"] = "skin".into();
    bound_plan["assets"].as_array_mut().unwrap().push(json!({"id":"skin","conversion":{"kind":"skin","source":"source.fbx","config":skin_config}}));
    let bound_scene = json!({"kind":"model","animationBindings":[{"animation":{"asset":"motion","file":"animation.rbxm"},"root":"mesh","bone":joint.name,"rigidTolerance":0.0001}],
        "roots":[{"id":"mesh","class":"MeshPart","name":"Mesh","properties":{},"references":{},"children":[],"assets":{"MeshContent":{"asset":"skin","file":skin.meshes[0].file}},"rig":{"asset":"skin","file":"rig.rbxm"}}]});
    fs::write(root.join("build.json"), bound_plan.to_string()).unwrap();
    fs::write(root.join("scene.json"), bound_scene.to_string()).unwrap();
    bundle::build(&root.join("build.json"), &root.join("bound-fbx")).unwrap();
}
