use rbx_dom_weak::types::{CFrame, Matrix3, Variant, Vector3};
use roblox_toolchain::{bundle, scene, skin_import};
use serde_json::json;
use std::{fs, path::Path};

fn cframe(c: [f32; 12]) -> CFrame {
    CFrame::new(
        Vector3::new(c[9], c[10], c[11]),
        Matrix3::new(
            Vector3::new(c[0], c[1], c[2]),
            Vector3::new(c[3], c[4], c[5]),
            Vector3::new(c[6], c[7], c[8]),
        ),
    )
}

#[test]
fn gltf_and_fbx_skin_rigs_become_matching_native_bones_in_scenes() {
    for filename in ["rigged-simple.glb", "maya-transformed-skin.fbx"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let source = Path::new("tests/fixtures").join(filename);
        let mesh_node = if filename.ends_with("glb") {
            2
        } else {
            let parsed = ufbx::load_memory(
                &fs::read(&source).unwrap(),
                ufbx::LoadOpts {
                    load_external_files: false,
                    ..Default::default()
                },
            )
            .unwrap();
            parsed
                .nodes
                .iter()
                .find(|n| {
                    n.mesh
                        .as_ref()
                        .is_some_and(|m| !m.skin_deformers.is_empty())
                })
                .unwrap()
                .element
                .typed_id as usize
        };
        let config = skin_import::Config {
            mesh_node,
            metres_per_stud: 0.5,
            cull_distance_metres: 20.,
            rigid_tolerance: 0.0001,
        };
        let converted = skin_import::convert(&source, &root.join("native"), &config).unwrap();
        let bytes = fs::read(root.join("native").join(&converted.rig_file)).unwrap();
        use sha2::{Digest, Sha256};
        assert_eq!(
            converted.rig_sha256,
            format!("{:x}", Sha256::digest(&bytes))
        );
        let dom = rbx_binary::from_reader(bytes.as_slice()).unwrap();
        for binding in &converted.rig {
            let bone = dom.descendants().find(|n| n.name == binding.name).unwrap();
            assert_eq!(bone.class.as_str(), "Bone");
            let Variant::CFrame(actual) = bone.properties[&"CFrame".into()] else {
                panic!("missing Bone.CFrame");
            };
            let expected = cframe(binding.local_bind_cframe);
            for (actual, expected) in [
                (actual.position, expected.position),
                (actual.orientation.x, expected.orientation.x),
                (actual.orientation.y, expected.orientation.y),
                (actual.orientation.z, expected.orientation.z),
            ] {
                for (actual, expected) in [
                    (actual.x, expected.x),
                    (actual.y, expected.y),
                    (actual.z, expected.z),
                ] {
                    // Native cardinal-rotation encoding removes source roundoff.
                    assert!((actual - expected).abs() <= config.rigid_tolerance);
                }
            }
            if let Some(index) = binding.parent {
                assert_eq!(
                    dom.get_by_ref(bone.parent()).unwrap().name,
                    converted.rig[usize::from(index)].name
                );
            } else {
                assert_eq!(bone.parent(), dom.root_ref());
            }
        }
        fs::copy(&source, root.join(filename)).unwrap();
        let plan = json!({"assets":[{"id":"skin","conversion":{"kind":"skin","source":filename,"config":config}}],"scenes":[{"id":"character","source":"scene.json"}]});
        let spec = json!({"kind":"model","roots":[{"id":"mesh","class":"MeshPart","name":"Character","properties":{},"references":{},"children":[],"assets":{"MeshContent":{"asset":"skin","file":converted.meshes[0].file}},"rig":{"asset":"skin","file":"rig.rbxm"}}]});
        fs::write(root.join("build.json"), plan.to_string()).unwrap();
        fs::write(root.join("scene.json"), spec.to_string()).unwrap();
        let out = root.join("bundle");
        let manifest = bundle::build(&root.join("build.json"), &out).unwrap();
        let assets: scene::AssetMap = manifest
            .files
            .iter()
            .map(|file| {
                (
                    scene::AssetReference {
                        asset: file.asset.clone(),
                        file: file.file.clone(),
                    },
                    scene::ResolvedAsset {
                        uri: file.local_uri.clone(),
                        path: out.join(&file.path),
                        animation_rig: None,
                    },
                )
            })
            .collect();
        let result = scene::build_with_assets(
            &root.join("scene.json"),
            &root.join("character.rbxm"),
            &assets,
        )
        .unwrap();
        assert_eq!(result.instances, converted.rig.len() + 1);
        let assembled =
            rbx_binary::from_reader(fs::File::open(root.join("character.rbxm")).unwrap()).unwrap();
        assert_eq!(
            assembled
                .descendants()
                .filter(|node| node.class.as_str() == "Bone")
                .count(),
            converted.rig.len()
        );
        assert_eq!(
            fs::read(root.join("character.rbxm")).unwrap(),
            fs::read(out.join(&manifest.scenes[0].path)).unwrap()
        );
        let mut wrong = spec.clone();
        wrong["roots"][0]["rig"]["asset"] = "other-skin".into();
        fs::write(root.join("scene.json"), wrong.to_string()).unwrap();
        assert!(
            scene::build_with_assets(
                &root.join("scene.json"),
                &root.join("rejected.rbxm"),
                &assets
            )
            .is_err()
        );
        assert!(!root.join("rejected.rbxm").exists());
    }
}

#[test]
fn real_source_rest_and_skin_bind_mismatch_is_rejected_not_silently_retargeted() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::copy("tests/fixtures/rigged-simple.glb", root.join("source.glb")).unwrap();
    let mut plan = json!({"assets":[
        {"id":"skin","conversion":{"kind":"skin","source":"source.glb","config":{"meshNode":2,"metresPerStud":0.5,"cullDistanceMetres":20,"rigidTolerance":0.0001}}},
        {"id":"motion","conversion":{"kind":"animationGltf","source":"source.glb","config":{"animationIndex":0,"rootNode":3,"metresPerStud":0.5,"rigidTolerance":0.0001,"name":"Motion","looped":true,"priority":"Action"}}}
    ],"scenes":[{"id":"character","source":"scene.json"}]});
    let spec = json!({"kind":"model","animationBindings":[{"animation":{"asset":"motion","file":"animation.rbxm"},"root":"mesh","bone":"Bone","rigidTolerance":0.0001}],"roots":[
        {"id":"mesh","class":"MeshPart","name":"Character","properties":{},"references":{},"children":[],"assets":{"MeshContent":{"asset":"skin","file":"node-2-primitive-0.mesh"}},"rig":{"asset":"skin","file":"rig.rbxm"}},
        {"id":"animation","class":"Animation","name":"Motion","properties":{},"references":{},"children":[],"assets":{"AnimationContent":{"asset":"motion","file":"animation.rbxm"}}}
    ]});
    fs::write(root.join("build.json"), plan.to_string()).unwrap();
    fs::write(root.join("scene.json"), spec.to_string()).unwrap();
    let error = bundle::build(&root.join("build.json"), &root.join("out"))
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("rest-space mismatch"), "{error}");
    assert!(!root.join("out").exists());
    plan["assets"][1]["conversion"]["bindTo"] = "skin".into();
    // Dependency data, not array order, must schedule the skin before the clip.
    plan["assets"].as_array_mut().unwrap().reverse();
    fs::write(root.join("build.json"), plan.to_string()).unwrap();
    let built = bundle::build(&root.join("build.json"), &root.join("out")).unwrap();
    assert_eq!(built.scenes.len(), 1);
    verify_rebased_motion(root, &built);
    for (pointer, value) in [
        ("/assets/0/conversion/bindTo", json!("missing")),
        ("/assets/0/conversion/bindTo", json!("motion")),
        ("/assets/1/conversion/config/metresPerStud", json!(0.25)),
    ] {
        let mut invalid = plan.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        fs::write(root.join("build.json"), invalid.to_string()).unwrap();
        let rejected = root.join("bad-dependency");
        assert!(bundle::build(&root.join("build.json"), &rejected).is_err());
        assert!(!rejected.exists());
    }
}

fn matrix(c: CFrame) -> glam::Mat4 {
    let r = c.orientation;
    glam::Mat4::from_cols_array(&[
        r.x.x,
        r.y.x,
        r.z.x,
        0.,
        r.x.y,
        r.y.y,
        r.z.y,
        0.,
        r.x.z,
        r.y.z,
        r.z.z,
        0.,
        c.position.x,
        c.position.y,
        c.position.z,
        1.,
    ])
}

fn verify_rebased_motion(root: &Path, built: &bundle::Manifest) {
    use roblox_toolchain::animation_gltf;
    let source = root.join("source.glb");
    let config = animation_gltf::Config {
        animation_index: 0,
        root_node: 3,
        metres_per_stud: 0.5,
        rigid_tolerance: 0.0001,
        name: "Motion".into(),
        looped: true,
        priority: "Action".into(),
    };
    let (clip, rig) = animation_gltf::read(&source, &config).unwrap();
    let native_scene = rbx_binary::from_reader(
        fs::File::open(root.join("out").join(&built.scenes[0].path)).unwrap(),
    )
    .unwrap();
    let bones: std::collections::HashMap<_, _> = native_scene
        .descendants()
        .filter(|node| node.class.as_str() == "Bone")
        .map(|bone| {
            let Variant::CFrame(rest) = bone.properties[&"CFrame".into()] else {
                panic!("bone rest missing");
            };
            (bone.name.clone(), matrix(rest))
        })
        .collect();
    let artifact = built
        .files
        .iter()
        .find(|file| file.asset == "motion" && file.file == "animation.rbxm")
        .unwrap();
    let native =
        rbx_binary::from_reader(fs::File::open(root.join("out").join(&artifact.path)).unwrap())
            .unwrap();
    let sequence = native.get_by_ref(native.root().children()[0]).unwrap();
    assert_eq!(sequence.children().len(), clip.frames.len());
    for (frame, reference) in clip.frames.iter().zip(sequence.children()) {
        let keyframe = native.get_by_ref(*reference).unwrap();
        let mut pending: Vec<_> = frame
            .poses
            .iter()
            .zip(keyframe.children())
            .map(|(pose, reference)| {
                (
                    pose,
                    *reference,
                    matrix(cframe(rig.root_parent_cframe)),
                    glam::Mat4::IDENTITY,
                )
            })
            .collect();
        while let Some((pose, reference, source_parent, target_parent)) = pending.pop() {
            let native_pose = native.get_by_ref(reference).unwrap();
            assert_eq!(pose.name, native_pose.name);
            let rest = rig
                .joints
                .iter()
                .find(|joint| joint.name == pose.name)
                .unwrap();
            let source_world =
                source_parent * matrix(cframe(rest.rest_cframe)) * matrix(cframe(pose.cframe));
            let Variant::CFrame(value) = native_pose.properties[&"CFrame".into()] else {
                panic!("pose missing");
            };
            let target_world = target_parent * bones[&pose.name] * matrix(value);
            assert!(
                source_world.abs_diff_eq(target_world, 0.0001),
                "changed motion at {} time {}",
                pose.name,
                frame.time
            );
            assert_eq!(pose.children.len(), native_pose.children().len());
            pending.extend(
                pose.children
                    .iter()
                    .zip(native_pose.children())
                    .map(|(child, reference)| (child, *reference, source_world, target_world)),
            );
        }
    }
}
