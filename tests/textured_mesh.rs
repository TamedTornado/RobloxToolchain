use base64::Engine;
use roblox_toolchain::convert::{Config, convert};
use serde_json::json;
use std::{fs, path::Path};

fn assert_deployment_keeps_explicit_maps(
    bundle: &Path,
    manifest: &roblox_toolchain::bundle::Manifest,
) {
    use rbx_dom_weak::types::{Content, Variant};
    use roblox_toolchain::{bundle_verify, deployment};

    let verification = bundle_verify::verify(bundle).unwrap();
    let mapping = deployment::Mapping {
        bundle_manifest_sha256: verification.manifest_sha256,
        bindings: manifest
            .files
            .iter()
            .enumerate()
            .map(|(index, file)| deployment::Binding {
                asset: file.asset.clone(),
                file: file.file.clone(),
                source_artifact_sha256: file.sha256.clone(),
                remote_id: (1000 + index).to_string(),
            })
            .collect(),
    };
    let output = bundle.parent().unwrap().join("deployed.rbxm");
    let result = deployment::link_scene(bundle, "box-model", &mapping, &output).unwrap();
    assert!(!result.engine_verified);
    assert!(!result.published);

    let linked = rbx_binary::from_reader(fs::File::open(output).unwrap()).unwrap();
    let part = linked.get_by_ref(linked.root().children()[0]).unwrap();
    let surface = linked.get_by_ref(part.children()[0]).unwrap();
    for (suffix, property, is_content) in [
        ("color.png", "ColorMapContent", true),
        ("texturepack.xml", "TexturePack", false),
    ] {
        let binding = mapping
            .bindings
            .iter()
            .find(|binding| binding.file.ends_with(suffix))
            .unwrap();
        let uri = format!("rbxassetid://{}", binding.remote_id);
        let expected = if is_content {
            Variant::Content(Content::from_uri(uri))
        } else {
            Variant::ContentId(uri.into())
        };
        assert_eq!(surface.properties[&property.into()], expected);
    }

    // The original live probe mistakenly supplied only TexturePack. Deployment
    // must retain the individual map: the edit-mode renderer needs it as well.
    let mut pack_only_mapping = mapping;
    pack_only_mapping
        .bindings
        .retain(|binding| !binding.file.ends_with("color.png"));
    let missing_output = bundle.parent().unwrap().join("pack-only.rbxm");
    let failure = deployment::link_scene(bundle, "box-model", &pack_only_mapping, &missing_output)
        .err()
        .unwrap();
    assert!(failure.to_string().contains("missing remote ID binding"));
    assert!(!missing_output.exists());
}

fn config() -> Config {
    serde_json::from_value(json!({"metresPerStud":0.28,"materials":{
        "localUriPrefix":"rbxasset://box/","maxWidth":256,"maxHeight":256,"maxDecodedBytes":1048576
    }}))
    .unwrap()
}

#[test]
fn independent_textured_glb_preserves_uvs_and_material_dependencies_through_bundle() {
    let temp = tempfile::tempdir().unwrap();
    let original_source = Path::new("tests/fixtures/box-textured.glb");
    // The upstream fixture requests nearest-texel mip filtering. Verify rejection,
    // then explicitly change only that sampler to the supported linear profile.
    let error = convert(original_source, &temp.path().join("unsupported"), &config())
        .err()
        .unwrap();
    assert!(error.to_string().contains("sampler"));
    assert!(!temp.path().join("unsupported").exists());
    let original_gltf = gltf::Gltf::open(original_source).unwrap();
    let mut document = serde_json::to_value(original_gltf.document.as_json()).unwrap();
    document["samplers"][0]["minFilter"] = 9987.into();
    document["buffers"][0]["uri"] = format!(
        "data:application/octet-stream;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(original_gltf.blob.as_ref().unwrap())
    )
    .into();
    let source = temp.path().join("source.gltf");
    fs::write(&source, document.to_string()).unwrap();
    let mut dds_policy = config();
    dds_policy.materials.as_mut().unwrap().outputs.color =
        roblox_toolchain::texture::Output::DdsRgba8 {
            mipmaps: true,
            max_output_bytes: 1048576,
            mip_filter: roblox_toolchain::texture_rgba::Filter::ColorStraightAlpha,
        };
    let dds_output = temp.path().join("dds");
    let dds = convert(&source, &dds_output, &dds_policy).unwrap();
    let dds_material = &dds.materials[0];
    let map = &dds_material.artifact.maps["color"];
    assert!(map.file.ends_with("color.dds"));
    let bytes = fs::read(dds_output.join(&dds_material.directory).join(&map.file)).unwrap();
    assert_eq!(&bytes[..4], b"DDS ");
    let output = temp.path().join("out");
    let result = convert(&source, &output, &config()).unwrap();
    assert_eq!(result.materials.len(), 1);
    let entry = &result.meshes[0];
    assert_eq!(entry.material.as_deref(), Some("material-0/material.rbxm"));
    let native = rbx_mesh::read_mesh_versioned(std::io::Cursor::new(
        fs::read(output.join(&entry.file)).unwrap(),
    ))
    .unwrap();
    let rbx_mesh::mesh::Mesh::V2(native) = native else {
        panic!("expected v2");
    };
    let rbx_mesh::mesh::Vertices2::Full(vertices) = native.vertices else {
        panic!("expected attributes");
    };
    let gltf = original_gltf;
    let blob = gltf.blob.as_ref().unwrap();
    let mesh = gltf.nodes().nth(entry.node_index).unwrap().mesh().unwrap();
    let primitive = mesh.primitives().nth(entry.primitive_index).unwrap();
    let reader = primitive.reader(|_| Some(blob.as_slice()));
    let uvs: Vec<_> = reader.read_tex_coords(0).unwrap().into_f32().collect();
    assert_eq!(vertices.len(), uvs.len());
    for (vertex, uv) in vertices.iter().zip(uvs) {
        assert_eq!(vertex.tex, uv);
    }
    let source_image = primitive
        .material()
        .pbr_metallic_roughness()
        .base_color_texture()
        .unwrap()
        .texture()
        .source();
    let gltf::image::Source::View { view, .. } = source_image.source() else {
        panic!("expected embedded image");
    };
    let original = image::load_from_memory(&blob[view.offset()..view.offset() + view.length()])
        .unwrap()
        .to_rgba8();
    let material = &result.materials[0];
    let converted = image::open(
        output
            .join(&material.directory)
            .join(&material.artifact.maps["color"].file),
    )
    .unwrap()
    .to_rgba8();
    assert_eq!(original, converted);

    let plan = json!({"assets":[{"id":"box","conversion":{"kind":"mesh","source":"source.gltf","config":{
        "metresPerStud":0.28,"materials":{"localUriPrefix":"rbxasset://standalone/","maxWidth":256,"maxHeight":256,"maxDecodedBytes":1048576}
    }}}],"scenes":[{"id":"box-model","source":"scene.json"}]});
    let scene = json!({"kind":"model","roots":[{"id":"box","class":"MeshPart","name":"Box","properties":{},"references":{},"children":[],
        "assets":{"MeshContent":{"asset":"box","file":entry.file}},"material":{"asset":"box","file":entry.material}
    }]});
    fs::write(temp.path().join("scene.json"), scene.to_string()).unwrap();
    let plan_path = temp.path().join("build.json");
    fs::write(&plan_path, plan.to_string()).unwrap();
    let bundle = temp.path().join("bundle");
    let manifest = roblox_toolchain::bundle::build(&plan_path, &bundle).unwrap();
    let appearance = manifest
        .files
        .iter()
        .find(|f| f.file.ends_with("material.rbxm"))
        .unwrap();
    let color = manifest
        .files
        .iter()
        .find(|f| f.file.ends_with("color.png"))
        .unwrap();
    let dom =
        rbx_binary::from_reader(fs::File::open(bundle.join(&appearance.path)).unwrap()).unwrap();
    let material = dom.get_by_ref(dom.root().children()[0]).unwrap();
    assert_eq!(
        material.properties[&"ColorMapContent".into()],
        rbx_dom_weak::types::Variant::Content(rbx_dom_weak::types::Content::from_uri(
            &color.local_uri
        ))
    );
    assert!(bundle.join(&color.path).is_file());
    let scene =
        rbx_binary::from_reader(fs::File::open(bundle.join(&manifest.scenes[0].path)).unwrap())
            .unwrap();
    let part = scene.get_by_ref(scene.root().children()[0]).unwrap();
    let surface = scene.get_by_ref(part.children()[0]).unwrap();
    assert_eq!(surface.properties, material.properties);
    assert_deployment_keeps_explicit_maps(&bundle, &manifest);
    let repeat = temp.path().join("repeat");
    roblox_toolchain::bundle::build(&plan_path, &repeat).unwrap();
    assert_eq!(
        fs::read(bundle.join("manifest.json")).unwrap(),
        fs::read(repeat.join("manifest.json")).unwrap()
    );

    let mut limited = config();
    limited.materials.as_mut().unwrap().max_width = 1;
    let error = convert(&source, &temp.path().join("limited"), &limited)
        .err()
        .unwrap();
    assert!(!error.to_string().contains("sampler"));
    assert!(!temp.path().join("limited").exists());

    let mut no_uv = document.clone();
    no_uv["meshes"][0]["primitives"][0]["attributes"]
        .as_object_mut()
        .unwrap()
        .remove("TEXCOORD_0");
    fs::write(&source, no_uv.to_string()).unwrap();
    let error = convert(&source, &temp.path().join("no-uv"), &config())
        .err()
        .unwrap();
    assert!(error.to_string().contains("UV0"));
    assert!(!temp.path().join("no-uv").exists());

    document["materials"][0]["normalTexture"] = json!({"index":0});
    fs::write(&source, document.to_string()).unwrap();
    let error = convert(&source, &temp.path().join("no-tangents"), &config())
        .err()
        .unwrap();
    assert!(error.to_string().contains("source tangents"));
    assert!(!temp.path().join("no-tangents").exists());
}

#[test]
fn textured_mesh_requires_explicit_material_policy_and_rolls_back_failures() {
    let temp = tempfile::tempdir().unwrap();
    let source = Path::new("tests/fixtures/box-textured.glb");
    let mut policy = config();
    policy.materials = None;
    let out = temp.path().join("out");
    assert!(convert(source, &out, &policy).is_err());
    assert!(!out.exists());
    let mut policy = config();
    policy.materials.as_mut().unwrap().max_width = 1;
    assert!(convert(source, &out, &policy).is_err());
    assert!(!out.exists());
    assert!(convert(Path::new("tests/fixtures/doorway.fbx"), &out, &config()).is_err());
    assert!(!out.exists());
}
