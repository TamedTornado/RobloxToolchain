use roblox_toolchain::scene::build;
use serde_json::json;
use std::fs;

fn specification() -> serde_json::Value {
    json!({"kind":"model","scriptCompiler":{"optimizationLevel":1,"debugLevel":1,"typeInfoLevel":0,"coverageLevel":0},"roots":[{
        "id":"model","class":"Model","name":"Kit","properties":{},"references":{"PrimaryPart":"part"},
        "children":[
            {"id":"part","class":"Part","name":"Block","properties":{"Anchored":{"Bool":true}},"references":{},"children":[]},
            {"id":"script","class":"ModuleScript","name":"Logic","properties":{},"references":{},"children":[],"scriptSource":"logic.luau"}
        ]
    }]})
}

#[test]
fn explicit_model_and_part_pivots_survive_without_rebaking_child_transforms() {
    use rbx_dom_weak::types::{CFrame, Matrix3, Variant, Vector3};
    let pivot = CFrame::new(Vector3::new(-3.0, 7.0, 11.0), Matrix3::identity());
    let offset = CFrame::new(
        Vector3::new(2.0, -1.0, 0.5),
        Matrix3::new(
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(-1.0, 0.0, 0.0),
        ),
    );
    let placement = CFrame::new(Vector3::new(100.0, 20.0, -50.0), Matrix3::identity());
    let spec = json!({"kind":"model","roots":[{"id":"outer","class":"Model","name":"Outer","properties":{"WorldPivotData":Variant::OptionalCFrame(Some(pivot))},"references":{},"children":[
        {"id":"inner","class":"Model","name":"Inner","properties":{},"references":{"PrimaryPart":"part"},"children":[
            {"id":"part","class":"MeshPart","name":"Mesh","properties":{"CFrame":Variant::CFrame(placement),"PivotOffset":Variant::CFrame(offset),"Size":Variant::Vector3(Vector3::new(8.0,12.0,4.0))},"references":{},"children":[]}
        ]}
    ]}]});
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("pivots.json");
    fs::write(&source, spec.to_string()).unwrap();
    let output = temp.path().join("pivots.rbxm");
    build(&source, &output).unwrap();
    let dom = rbx_binary::from_reader(fs::File::open(output).unwrap()).unwrap();
    let outer = dom.get_by_ref(dom.root().children()[0]).unwrap();
    let inner = dom.get_by_ref(outer.children()[0]).unwrap();
    let part = dom.get_by_ref(inner.children()[0]).unwrap();
    assert_eq!(
        outer.properties[&"WorldPivotData".into()],
        Variant::OptionalCFrame(Some(pivot))
    );
    assert_eq!(
        part.properties[&"PivotOffset".into()],
        Variant::CFrame(offset)
    );
    assert_eq!(
        part.properties[&"CFrame".into()],
        Variant::CFrame(placement)
    );
    assert_eq!(
        part.properties[&"Size".into()],
        Variant::Vector3(Vector3::new(8.0, 12.0, 4.0))
    );
    assert_eq!(
        inner.properties[&"PrimaryPart".into()],
        Variant::Ref(part.referent())
    );
}

#[test]
fn native_scene_preserves_hierarchy_scripts_references_and_repeatable_bytes() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("scene.json");
    fs::write(&source, serde_json::to_vec(&specification()).unwrap()).unwrap();
    fs::write(
        temporary.path().join("logic.luau"),
        "return {answer = 42}\n",
    )
    .unwrap();
    let first = temporary.path().join("first.rbxm");
    let second = temporary.path().join("second.rbxm");
    let result = build(&source, &first).unwrap();
    assert_eq!(result.instances, 3);
    assert!(!result.published);
    build(&source, &second).unwrap();
    let bytes = fs::read(&first).unwrap();
    assert_eq!(bytes, fs::read(&second).unwrap());
    let dom = rbx_binary::from_reader(bytes.as_slice()).unwrap();
    let model = dom.get_by_ref(dom.root().children()[0]).unwrap();
    assert_eq!(model.name, "Kit");
    assert_eq!(model.children().len(), 2);
    assert_eq!(
        model.properties.get(&"PrimaryPart".into()),
        Some(&rbx_dom_weak::types::Variant::Ref(model.children()[0]))
    );
    let module = dom.get_by_ref(model.children()[1]).unwrap();
    assert_eq!(
        module.properties.get(&"Source".into()),
        Some(&rbx_dom_weak::types::Variant::String(
            "return {answer = 42}\n".into()
        ))
    );
    assert!(build(&source, &first).is_err());
    assert_eq!(bytes, fs::read(&first).unwrap());
}

#[test]
fn malformed_scenes_fail_before_creating_output() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("scene.json");
    fs::write(temporary.path().join("logic.luau"), "return {}").unwrap();
    let out = temporary.path().join("out.rbxm");
    let mut missing = specification();
    missing["roots"][0]["references"]["PrimaryPart"] = "absent".into();
    let mut duplicate = specification();
    duplicate["roots"][0]["children"][1]["id"] = "part".into();
    let mut unknown = specification();
    unknown["roots"][0]["properties"]["NotAProperty"] = json!({"Bool":true});
    for value in [missing, duplicate, unknown] {
        fs::write(&source, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(build(&source, &out).is_err());
        assert!(!out.exists());
    }
}

#[test]
fn scene_rejects_unsaved_properties_conflicting_migrations_and_wrong_types() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("scene.json");
    let output = temporary.path().join("out.rbxm");
    let cases = [
        (
            "Part",
            json!({"AssemblyMass":{"Float32":1.}}),
            json!({}),
            "does not serialize",
        ),
        (
            "Animation",
            json!({
                "AnimationId":{"ContentId":"rbxassetid://123"},
                "AnimationContent":{"Content":{"Uri":"rbxassetid://456"}}
            }),
            json!({}),
            "both serialize to AnimationContent",
        ),
        (
            "Part",
            json!({"Anchored":{"String":"true"}}),
            json!({}),
            "requires Bool",
        ),
        (
            "Part",
            json!({}),
            json!({"Anchored":"node"}),
            "not an instance reference",
        ),
    ];

    for (class, properties, references, expected) in cases {
        let spec = json!({"kind":"model", "roots":[{
            "id":"node", "class":class, "name":"test",
            "properties":properties, "references":references,"children":[]
        }]});
        fs::write(&source, serde_json::to_vec(&spec).unwrap()).unwrap();
        let error = build(&source, &output).err().unwrap().to_string();
        assert!(error.contains(expected), "expected {expected}, got {error}");
        assert!(!output.exists());
    }
}

#[test]
fn place_cli_builds_offline_and_script_paths_cannot_escape() {
    let temporary = tempfile::tempdir().unwrap();
    let source_root = temporary.path().join("source");
    fs::create_dir(&source_root).unwrap();
    let source = source_root.join("scene.json");
    let output = temporary.path().join("place.rbxl");
    fs::write(source_root.join("logic.luau"), "return {}").unwrap();
    let mut spec = specification();
    spec["kind"] = "place".into();
    spec["roots"][0]["class"] = "Workspace".into();
    spec["roots"][0]["references"] = json!({});
    fs::write(&source, serde_json::to_vec(&spec).unwrap()).unwrap();
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_roblox"))
        .env_clear()
        .args(["build", "scene"])
        .arg(&source)
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let response: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(response["result"]["format"], "rbxl");
    let dom = rbx_binary::from_reader(fs::read(&output).unwrap().as_slice()).unwrap();
    assert_eq!(
        dom.get_by_ref(dom.root().children()[0])
            .unwrap()
            .class
            .as_str(),
        "Workspace"
    );

    fs::write(temporary.path().join("outside.luau"), "return 'outside'").unwrap();
    spec["roots"][0]["children"][1]["scriptSource"] = "../outside.luau".into();
    fs::write(&source, serde_json::to_vec(&spec).unwrap()).unwrap();
    let rejected = temporary.path().join("rejected.rbxl");
    assert!(
        build(&source, &rejected)
            .err()
            .unwrap()
            .to_string()
            .contains("source directory")
    );
    assert!(!rejected.exists());
}

#[test]
fn scene_build_ignores_ambient_reflection_database_overrides() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("scene.json");
    fs::write(&source, serde_json::to_vec(&specification()).unwrap()).unwrap();
    fs::write(temporary.path().join("logic.luau"), "return {}").unwrap();
    let baseline = temporary.path().join("baseline.rbxm");
    build(&source, &baseline).unwrap();
    let invalid_database = temporary.path().join("invalid.msgpack");
    fs::write(&invalid_database, b"not a reflection database").unwrap();
    let output = temporary.path().join("actual.rbxm");
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_roblox"))
        .env_clear()
        .env("RBX_DATABASE", invalid_database)
        .args(["build", "scene"])
        .arg(source)
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(fs::read(baseline).unwrap(), fs::read(output).unwrap());
}

#[test]
fn scene_script_compilation_is_mandatory_for_inline_and_file_sources() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("scene.json");
    let output = temporary.path().join("model.rbxm");
    fs::write(temporary.path().join("logic.luau"), "local = invalid").unwrap();
    fs::write(&source, serde_json::to_vec(&specification()).unwrap()).unwrap();
    assert!(
        build(&source, &output)
            .err()
            .unwrap()
            .to_string()
            .contains("failed compilation")
    );
    assert!(!output.exists());
    let mut inline = specification();
    inline["roots"][0]["children"][1]
        .as_object_mut()
        .unwrap()
        .remove("scriptSource");
    inline["roots"][0]["children"][1]["properties"]["Source"] = json!({"String":"local = invalid"});
    fs::write(&source, serde_json::to_vec(&inline).unwrap()).unwrap();
    assert!(build(&source, &output).is_err());
    assert!(!output.exists());
    inline["roots"][0]["children"][1]["properties"]["Source"] = json!({"String":"return 42"});
    inline.as_object_mut().unwrap().remove("scriptCompiler");
    fs::write(&source, serde_json::to_vec(&inline).unwrap()).unwrap();
    assert!(
        build(&source, &output)
            .err()
            .unwrap()
            .to_string()
            .contains("scriptCompiler")
    );
    assert!(!output.exists());
}
