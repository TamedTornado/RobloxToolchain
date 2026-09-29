use base64::Engine;
use image::{Rgba, RgbaImage};
use roblox_toolchain::material_gltf::{Config, convert};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn config() -> Config {
    Config {
        material_index: 0,
        name: "Paint".into(),
        local_uri_prefix: "rbxasset://paint/".into(),
        max_width: 2,
        max_height: 2,
        max_decoded_bytes: 4096,
        outputs: Default::default(),
    }
}

fn input(root: &Path) -> Value {
    RgbaImage::from_pixel(2, 1, Rgba([128, 255, 64, 128]))
        .save(root.join("base.png"))
        .unwrap();
    RgbaImage::from_pixel(1, 1, Rgba([255, 128, 64, 255]))
        .save(root.join("packed.png"))
        .unwrap();
    json!({"asset":{"version":"2.0"},"images":[{"uri":"base.png"},{"uri":"packed.png"}],
    "textures":[{"source":0},{"source":1}],
    "materials":[{"alphaMode":"BLEND","pbrMetallicRoughness":{
        "baseColorFactor":[0.5,0.25,1,0.5],"baseColorTexture":{"index":0},
        "metallicFactor":0.5,"roughnessFactor":0.5,"metallicRoughnessTexture":{"index":1}
    }}]})
}

#[test]
fn gltf_material_output_profiles_survive_bundle_linking_and_native_serialization() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let source = input(root);
    fs::write(root.join("source.gltf"), source.to_string()).unwrap();
    let profiles = json!({
        "color":{"format":"ddsRgba8","mipmaps":true,"maxOutputBytes":140,"mipFilter":"colorPremultipliedAlpha"},
        "normal":{"format":"png"},
        "roughness":{"format":"ddsBc4","mipmaps":true,"maxOutputBytes":136},
        "metalness":{"format":"ddsL8","mipmaps":false,"maxOutputBytes":129}
    });
    let config = json!({"materialIndex":0,"name":"Paint","localUriPrefix":"rbxasset://paint/",
        "maxWidth":2,"maxHeight":2,"maxDecodedBytes":4096,"outputs":profiles});
    let parsed: Config = serde_json::from_value(config.clone()).unwrap();
    let standalone = root.join("standalone");
    let material = convert(&root.join("source.gltf"), &standalone, &parsed).unwrap();
    for map in material.maps.values() {
        assert!(map.file.ends_with(".dds"));
    }
    let plan = json!({"assets":[{"id":"paint","conversion":{"kind":"materialGltf","source":"source.gltf","config":config}}],"scenes":[]});
    fs::write(root.join("build.json"), plan.to_string()).unwrap();
    let bundle = root.join("bundle");
    let manifest = roblox_toolchain::bundle::build(&root.join("build.json"), &bundle).unwrap();
    for map in material.maps.values() {
        let file = manifest
            .files
            .iter()
            .find(|file| file.file == map.file)
            .unwrap();
        assert_eq!(
            fs::read(standalone.join(&map.file)).unwrap(),
            fs::read(bundle.join(&file.path)).unwrap()
        );
    }
    let model = manifest
        .files
        .iter()
        .find(|file| file.file == "material.rbxm")
        .unwrap();
    let dom =
        rbx_binary::from_reader(fs::read(bundle.join(&model.path)).unwrap().as_slice()).unwrap();
    let instance = dom.get_by_ref(dom.root().children()[0]).unwrap();
    let color = manifest
        .files
        .iter()
        .find(|file| file.file.ends_with("color.dds"))
        .unwrap();
    assert_eq!(
        instance.properties[&"ColorMapContent".into()],
        rbx_dom_weak::types::Variant::Content(rbx_dom_weak::types::Content::from_uri(
            &color.local_uri
        ))
    );
}

#[test]
fn gltf_factors_bake_in_correct_color_spaces_and_bundle_links_the_result() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let spec = input(root);
    let source = root.join("source.gltf");
    fs::write(&source, spec.to_string()).unwrap();
    let output = root.join("out");
    let result = convert(&source, &output, &config()).unwrap();
    assert_eq!(
        image::open(output.join(&result.maps["color"].file))
            .unwrap()
            .to_rgba8()
            .get_pixel(0, 0)
            .0,
        [92, 137, 64, 64]
    );
    assert_eq!(
        image::open(output.join(&result.maps["roughness"].file))
            .unwrap()
            .to_luma8()
            .get_pixel(0, 0)
            .0,
        [64]
    );
    assert_eq!(
        image::open(output.join(&result.maps["metalness"].file))
            .unwrap()
            .to_luma8()
            .get_pixel(0, 0)
            .0,
        [32]
    );
    let repeated = root.join("again");
    convert(&source, &repeated, &config()).unwrap();
    assert_eq!(
        fs::read(output.join("manifest.json")).unwrap(),
        fs::read(repeated.join("manifest.json")).unwrap()
    );

    let config = json!({"materialIndex":0,"name":"Paint","localUriPrefix":"rbxasset://paint/","maxWidth":2,"maxHeight":2,"maxDecodedBytes":4096});
    fs::write(root.join("config.json"), config.to_string()).unwrap();
    let cli = std::process::Command::new(env!("CARGO_BIN_EXE_roblox"))
        .env_clear()
        .args(["convert", "material-gltf"])
        .arg(&source)
        .arg("--config")
        .arg(root.join("config.json"))
        .arg("--output")
        .arg(root.join("cli"))
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    assert_eq!(
        fs::read(output.join("material.rbxm")).unwrap(),
        fs::read(root.join("cli/material.rbxm")).unwrap()
    );
    let plan = json!({"assets":[{"id":"paint","conversion":{"kind":"materialGltf","source":"source.gltf","config":config}}],"scenes":[]});
    fs::write(root.join("build.json"), plan.to_string()).unwrap();
    let bundle = root.join("bundle");
    let built = roblox_toolchain::bundle::build(&root.join("build.json"), &bundle).unwrap();
    assert_eq!(built.files.len(), 5);
    let native = built
        .files
        .iter()
        .find(|f| f.file == "material.rbxm")
        .unwrap();
    let color = built
        .files
        .iter()
        .find(|f| f.file.ends_with("color.png"))
        .unwrap();
    let dom = rbx_binary::from_reader(fs::File::open(bundle.join(&native.path)).unwrap()).unwrap();
    let appearance = dom.get_by_ref(dom.root().children()[0]).unwrap();
    assert_eq!(
        appearance.properties[&"ColorMapContent".into()],
        rbx_dom_weak::types::Variant::Content(rbx_dom_weak::types::Content::from_uri(
            &color.local_uri
        ))
    );
}

#[test]
fn image_data_uris_glb_views_and_opaque_alpha_agree() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut spec = input(root);
    spec["materials"][0]["alphaMode"] = "OPAQUE".into();
    let mut bytes = fs::read(root.join("base.png")).unwrap();
    spec["images"][0]["uri"] = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    )
    .into();
    let source = root.join("source.gltf");
    fs::write(&source, spec.to_string()).unwrap();
    let out = root.join("uri");
    let result = convert(&source, &out, &config()).unwrap();
    assert_eq!(
        image::open(out.join(&result.maps["color"].file))
            .unwrap()
            .to_rgba8()
            .get_pixel(0, 0)[3],
        255
    );

    spec["images"][0] = json!({"bufferView":0,"mimeType":"image/png"});
    spec["buffers"] = json!([{"byteLength":bytes.len()}]);
    spec["bufferViews"] = json!([{"buffer":0,"byteOffset":0,"byteLength":bytes.len()}]);
    let mut json = spec.to_string().into_bytes();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
    let mut glb = Vec::new();
    for word in [
        0x46546c67u32,
        2,
        (12 + 8 + json.len() + 8 + bytes.len()) as u32,
        json.len() as u32,
        0x4e4f534a,
    ] {
        glb.extend(word.to_le_bytes());
    }
    glb.extend(json);
    glb.extend((bytes.len() as u32).to_le_bytes());
    glb.extend(0x004e4942u32.to_le_bytes());
    glb.extend(bytes);
    let source = root.join("source.glb");
    fs::write(&source, glb).unwrap();
    let target = root.join("glb");
    convert(&source, &target, &config()).unwrap();
    assert_eq!(
        fs::read(out.join("manifest.json")).unwrap(),
        fs::read(target.join("manifest.json")).unwrap()
    );
}

#[test]
fn unsupported_semantics_invalid_inputs_and_nondefault_limits_fail_without_output() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let spec = input(root);
    let source = root.join("source.gltf");
    for (pointer, value) in [
        ("/materials/0/alphaMode", json!("MASK")),
        (
            "/materials/0/pbrMetallicRoughness/baseColorFactor/0",
            json!(2),
        ),
        ("/images/0/uri", json!("https://example.com/image.png")),
        ("/images/0/uri", json!("../outside.png")),
    ] {
        let mut invalid = spec.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        fs::write(&source, invalid.to_string()).unwrap();
        let output = root.join("rejected");
        assert!(convert(&source, &output, &config()).is_err());
        assert!(!output.exists());
    }
    fs::write(&source, spec.to_string()).unwrap();
    let mut small = config();
    small.max_width = 1;
    assert!(convert(&source, &root.join("small"), &small).is_err());
    assert!(!root.join("small").exists());
    let mut extensions = spec.clone();
    extensions["extensionsUsed"] = json!(["KHR_materials_unlit"]);
    fs::write(&source, extensions.to_string()).unwrap();
    assert!(convert(&source, &root.join("extension"), &config()).is_err());
    for field in [
        json!({"doubleSided":true}),
        json!({"emissiveFactor":[1,0,0]}),
        json!({"occlusionTexture":{"index":0}}),
        json!({"normalTexture":{"index":0,"scale":0.5}}),
        json!({"normalTexture":{"index":0,"texCoord":1}}),
    ] {
        let mut invalid = spec.clone();
        invalid["materials"][0]
            .as_object_mut()
            .unwrap()
            .extend(field.as_object().unwrap().clone());
        fs::write(&source, invalid.to_string()).unwrap();
        assert!(convert(&source, &root.join("unsupported"), &config()).is_err());
        assert!(!root.join("unsupported").exists());
    }
    let mut nearest = spec.clone();
    nearest["samplers"] = json!([{"magFilter":9728}]);
    nearest["textures"][0]["sampler"] = 0.into();
    fs::write(&source, nearest.to_string()).unwrap();
    assert!(convert(&source, &root.join("nearest"), &config()).is_err());
}

#[test]
fn normal_maps_remain_linear_without_base_color_factor_multiplication() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut spec = input(root);
    spec["materials"][0]["normalTexture"] = json!({"index":1});
    let source = root.join("source.gltf");
    fs::write(&source, spec.to_string()).unwrap();
    let out = root.join("out");
    let result = convert(&source, &out, &config()).unwrap();
    assert_eq!(result.maps["normal"].color_space, "linear");
    assert_eq!(
        image::open(out.join(&result.maps["normal"].file))
            .unwrap()
            .to_rgb8()
            .get_pixel(0, 0)
            .0,
        [255, 128, 64]
    );
}

#[test]
fn independent_khronos_material_without_textures_generates_constant_maps() {
    let temp = tempfile::tempdir().unwrap();
    let source = Path::new("tests/fixtures/rigged-simple.glb");
    let gltf = gltf::Gltf::open(source).unwrap();
    let pbr = gltf.materials().next().unwrap().pbr_metallic_roughness();
    let result = convert(source, &temp.path().join("out"), &config()).unwrap();
    for (semantic, expected) in [
        ("metalness", pbr.metallic_factor()),
        ("roughness", pbr.roughness_factor()),
    ] {
        let pixel = image::open(temp.path().join("out").join(&result.maps[semantic].file))
            .unwrap()
            .to_luma8();
        assert_eq!(pixel.dimensions(), (1, 1));
        assert!((pixel.get_pixel(0, 0)[0] as f32 / 255.0 - expected).abs() <= 0.5 / 255.0);
    }
}
