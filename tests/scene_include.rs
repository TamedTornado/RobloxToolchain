use rbx_dom_weak::{InstanceBuilder, WeakDom, types::Variant};
use roblox_toolchain::scene::build;
use serde_json::json;
use std::{fs, path::Path};

const FIXTURE: &str = "tests/fixtures/rojo-include";

fn compiler() -> serde_json::Value {
    json!({"optimizationLevel":1,"debugLevel":1,"typeInfoLevel":0,"coverageLevel":0})
}

fn place(
    include_server: serde_json::Value,
    include_shared: serde_json::Value,
) -> serde_json::Value {
    json!({"kind":"place","scriptCompiler":compiler(),"roots":[
        {"id":"server","class":"ServerScriptService","name":"ServerScriptService","properties":{},"references":{},
         "include":include_server,
         "children":[{"id":"explicit","class":"Folder","name":"Explicit","properties":{},"references":{},"children":[]}]},
        {"id":"shared","class":"ReplicatedStorage","name":"ReplicatedStorage","properties":{},"references":{},
         "include":include_shared,"children":[]}
    ]})
}

fn child<'a>(
    dom: &'a WeakDom,
    parent: &rbx_dom_weak::Instance,
    name: &str,
) -> &'a rbx_dom_weak::Instance {
    let matches: Vec<_> = parent
        .children()
        .iter()
        .map(|r| dom.get_by_ref(*r).unwrap())
        .filter(|i| i.name == name)
        .collect();
    assert_eq!(matches.len(), 1, "expected one child named {name}");
    matches[0]
}

fn source(instance: &rbx_dom_weak::Instance) -> &str {
    match instance.properties.get(&"Source".into()) {
        Some(Variant::String(text)) => text,
        other => panic!("unexpected Source {other:?}"),
    }
}

fn write_scene(root: &Path, spec: &serde_json::Value) -> std::path::PathBuf {
    let path = root.join("scene.json");
    fs::write(&path, serde_json::to_vec(spec).unwrap()).unwrap();
    path
}

#[test]
fn rojo_output_is_included_under_place_services_with_repeatable_bytes() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    fs::copy(Path::new(FIXTURE).join("code.rbxl"), root.join("code.rbxl")).unwrap();
    let spec = place(
        json!({"file":"code.rbxl","path":["ServerScriptService"]}),
        json!({"file":"code.rbxl","path":["ReplicatedStorage"]}),
    );
    let scene = write_scene(root, &spec);
    let first = root.join("first.rbxl");
    let second = root.join("second.rbxl");
    build(&scene, &first).unwrap();
    build(&scene, &second).unwrap();
    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());

    let dom = rbx_binary::from_reader(fs::File::open(&first).unwrap()).unwrap();
    let top = dom.root();
    let server = child(&dom, top, "ServerScriptService");
    child(&dom, server, "Explicit");
    let main = child(&dom, child(&dom, server, "Server"), "Main");
    assert_eq!(main.class.as_str(), "Script");
    assert_eq!(
        source(main),
        fs::read_to_string(Path::new(FIXTURE).join("src/server/Main.server.luau")).unwrap()
    );
    let shared = child(&dom, top, "ReplicatedStorage");
    let config = child(&dom, child(&dom, shared, "Shared"), "Config");
    assert_eq!(config.class.as_str(), "ModuleScript");
    assert_eq!(
        source(config),
        fs::read_to_string(Path::new(FIXTURE).join("src/shared/Config.luau")).unwrap()
    );
}

fn two_folders_with_link(path: &Path, duplicate: bool) {
    let mut dom = WeakDom::new(InstanceBuilder::new("DataModel"));
    let root = dom.root_ref();
    let a = dom.insert(root, InstanceBuilder::new("Folder").with_name("A"));
    let link = dom.insert(a, InstanceBuilder::new("ObjectValue").with_name("Link"));
    let b = dom.insert(
        root,
        InstanceBuilder::new("Folder").with_name(if duplicate { "A" } else { "B" }),
    );
    let target = dom.insert(b, InstanceBuilder::new("Part").with_name("Target"));
    dom.get_by_ref_mut(link)
        .unwrap()
        .properties
        .insert("Value".into(), Variant::Ref(target));
    let refs = dom.root().children().to_vec();
    rbx_binary::to_writer(fs::File::create(path).unwrap(), &dom, &refs).unwrap();
}

#[test]
fn included_references_are_remapped_and_outside_references_fail() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    two_folders_with_link(&root.join("tree.rbxm"), false);
    let spec = |path: serde_json::Value| {
        json!({"kind":"model","roots":[{"id":"host","class":"Folder","name":"Host","properties":{},"references":{},
            "include":{"file":"tree.rbxm","path":path},"children":[]}]})
    };

    let scene = write_scene(root, &spec(json!([])));
    let output = root.join("whole.rbxm");
    build(&scene, &output).unwrap();
    let dom = rbx_binary::from_reader(fs::File::open(&output).unwrap()).unwrap();
    let host = dom.get_by_ref(dom.root().children()[0]).unwrap();
    let link = child(&dom, child(&dom, host, "A"), "Link");
    let target = child(&dom, child(&dom, host, "B"), "Target");
    assert_eq!(
        link.properties[&"Value".into()],
        Variant::Ref(target.referent())
    );

    let scene = write_scene(root, &spec(json!(["A"])));
    let rejected = root.join("partial.rbxm");
    let error = build(&scene, &rejected).err().unwrap().to_string();
    assert!(error.contains("outside the included tree"), "{error}");
    assert!(!rejected.exists());
}

#[test]
fn include_rejects_bad_files_selections_and_unchecked_scripts() {
    let temporary = tempfile::tempdir().unwrap();
    let source_root = temporary.path().join("source");
    fs::create_dir(&source_root).unwrap();
    fs::copy(
        Path::new(FIXTURE).join("code.rbxl"),
        source_root.join("code.rbxl"),
    )
    .unwrap();
    fs::copy(
        Path::new(FIXTURE).join("code.rbxl"),
        temporary.path().join("outside.rbxl"),
    )
    .unwrap();
    fs::write(source_root.join("text.rbxm"), "not a native file").unwrap();
    two_folders_with_link(&source_root.join("duplicate.rbxm"), true);
    let shared = json!({"file":"code.rbxl","path":["ReplicatedStorage"]});
    let cases = [
        (
            place(
                json!({"file":"code.rbxl","path":["Missing"]}),
                shared.clone(),
            ),
            "no instance at path segment Missing",
        ),
        (
            place(json!({"file":"../outside.rbxl","path":[]}), shared.clone()),
            "scene source directory",
        ),
        (
            place(json!({"file":"text.rbxm","path":[]}), shared.clone()),
            "not a binary model or place",
        ),
        (
            place(
                json!({"file":"duplicate.rbxm","path":["A"]}),
                shared.clone(),
            ),
            "ambiguous",
        ),
        (
            place(
                json!({"file":"code.rbxl","path":["ReplicatedStorage","Shared","Config"]}),
                shared.clone(),
            ),
            "selects no instances",
        ),
        (
            place(
                json!({"file":"code.rbxl","path":[],"extra":true}),
                shared.clone(),
            ),
            "unknown field",
        ),
    ];
    for (spec, expected) in cases {
        let scene = write_scene(&source_root, &spec);
        let output = temporary.path().join("rejected.rbxl");
        let error = build(&scene, &output).err().unwrap().to_string();
        assert!(error.contains(expected), "expected {expected}, got {error}");
        assert!(!output.exists());
    }

    let mut unchecked = place(
        json!({"file":"code.rbxl","path":["ServerScriptService"]}),
        shared,
    );
    unchecked.as_object_mut().unwrap().remove("scriptCompiler");
    let scene = write_scene(&source_root, &unchecked);
    let output = temporary.path().join("unchecked.rbxl");
    let error = build(&scene, &output).err().unwrap().to_string();
    assert!(error.contains("scriptCompiler"), "{error}");
    assert!(!output.exists());
}

#[test]
fn included_scripts_pass_the_compilation_gate() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let mut dom = WeakDom::new(InstanceBuilder::new("DataModel"));
    let top = dom.root_ref();
    dom.insert(
        top,
        InstanceBuilder::new("ModuleScript")
            .with_name("Broken")
            .with_property("Source", "local = invalid"),
    );
    let refs = dom.root().children().to_vec();
    rbx_binary::to_writer(
        fs::File::create(root.join("broken.rbxm")).unwrap(),
        &dom,
        &refs,
    )
    .unwrap();
    let spec = json!({"kind":"model","scriptCompiler":compiler(),"roots":[{"id":"host","class":"Folder","name":"Host",
        "properties":{},"references":{},"include":{"file":"broken.rbxm","path":[]},"children":[]}]});
    let scene = write_scene(root, &spec);
    let output = root.join("broken-out.rbxm");
    let error = build(&scene, &output).err().unwrap().to_string();
    assert!(
        error.contains("Broken in include broken.rbxm failed compilation"),
        "{error}"
    );
    assert!(!output.exists());
}
