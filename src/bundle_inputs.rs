//! Source dependency closure for incremental builds, separate from runtime artifacts.
use crate::{
    Result,
    bundle::{Conversion, Plan},
    scene,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Inputs {
    pub format: String,
    pub version: u32,
    pub plan_sha256: String,
    pub assets: BTreeMap<String, BTreeMap<String, String>>,
    pub scenes: BTreeMap<String, BTreeMap<String, String>>,
}

fn local(root: &Path, source: &Path) -> Result<PathBuf> {
    let path = root.join(source).canonicalize()?;
    if source.is_absolute() || !path.starts_with(root) || !path.is_file() {
        return Err("build input must be a contained regular file".into());
    }
    Ok(path)
}

fn record(root: &Path, path: &Path, files: &mut BTreeMap<String, String>) -> Result<()> {
    let relative = path
        .strip_prefix(root)?
        .to_str()
        .ok_or("non-UTF8 build input path")?;
    files.insert(
        relative.to_owned(),
        format!("{:x}", Sha256::digest(fs::read(path)?)),
    );
    Ok(())
}

fn uri(
    root: &Path,
    parent: &Path,
    value: &str,
    files: &mut BTreeMap<String, String>,
) -> Result<()> {
    if !value.starts_with("data:") {
        record(root, &crate::convert::local_uri_path(parent, value)?, files)?;
    }
    Ok(())
}

enum Materials {
    None,
    Used,
    Index(usize),
}

fn gltf_inputs(
    root: &Path,
    source: &Path,
    buffers: bool,
    materials: Materials,
    files: &mut BTreeMap<String, String>,
) -> Result<()> {
    let document = gltf::Gltf::from_slice(&fs::read(source)?)?;
    let parent = source.parent().ok_or("missing input parent")?;
    if buffers {
        for buffer in document.buffers() {
            if let gltf::buffer::Source::Uri(value) = buffer.source() {
                uri(root, parent, value, files)?;
            }
        }
    }
    let selected: Vec<_> = match materials {
        Materials::None => return Ok(()),
        Materials::Index(index) => vec![
            document
                .materials()
                .nth(index)
                .ok_or("material index outside source")?,
        ],
        Materials::Used => document
            .meshes()
            .flat_map(|mesh| mesh.primitives().map(|p| p.material()).collect::<Vec<_>>())
            .collect(),
    };
    for material in selected {
        let pbr = material.pbr_metallic_roughness();
        let textures = [
            pbr.base_color_texture().map(|t| t.texture()),
            pbr.metallic_roughness_texture().map(|t| t.texture()),
            material.normal_texture().map(|t| t.texture()),
        ];
        for texture in textures.into_iter().flatten() {
            match texture.source().source() {
                gltf::image::Source::Uri { uri: value, .. } => uri(root, parent, value, files)?,
                gltf::image::Source::View { .. } => {
                    // The material decoder uses load_gltf_buffers, which reads
                    // every declared buffer before slicing the image view.
                    for buffer in document.buffers() {
                        if let gltf::buffer::Source::Uri(value) = buffer.source() {
                            uri(root, parent, value, files)?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn scripts(
    root: &Path,
    parent: &Path,
    nodes: &[scene::Node],
    files: &mut BTreeMap<String, String>,
) -> Result<()> {
    for node in nodes {
        if let Some(source) = &node.script_source {
            record(root, &local(parent, source)?, files)?;
        }
        if let Some(include) = &node.include {
            record(root, &local(parent, &include.file)?, files)?;
        }
        scripts(root, parent, &node.children, files)?;
    }
    Ok(())
}

pub(crate) fn collect(source: &Path, plan: &Plan) -> Result<Inputs> {
    let root = source.parent().ok_or("missing build source parent")?;
    let mut result = Inputs {
        format: "roblox-build-inputs".into(),
        version: 1,
        plan_sha256: format!("{:x}", Sha256::digest(fs::read(source)?)),
        assets: BTreeMap::new(),
        scenes: BTreeMap::new(),
    };
    for asset in &plan.assets {
        let input = match &asset.conversion {
            Conversion::Terrain { source }
            | Conversion::MediaSource { source, .. }
            | Conversion::Media { source }
            | Conversion::Video { source, .. }
            | Conversion::MaterialGltf { source, .. }
            | Conversion::Material { source }
            | Conversion::AnimationFbx { source, .. }
            | Conversion::Skin { source, .. }
            | Conversion::AnimationGltf { source, .. }
            | Conversion::Animation { source }
            | Conversion::Mesh { source, .. }
            | Conversion::Texture { source, .. }
            | Conversion::Audio { source, .. } => local(root, source)?,
        };
        let parent = input.parent().ok_or("missing asset source parent")?;
        let mut files = BTreeMap::new();
        record(root, &input, &mut files)?;
        match &asset.conversion {
            Conversion::Terrain { .. } => {
                let spec: crate::terrain::Specification =
                    serde_json::from_slice(&fs::read(&input)?)?;
                if let crate::terrain::Specification::Heightmap { source, .. } = spec {
                    record(
                        root,
                        &crate::terrain::image_source(&input, &source)?,
                        &mut files,
                    )?;
                }
            }
            Conversion::Material { .. } => {
                let spec: crate::material::Specification =
                    serde_json::from_slice(&fs::read(&input)?)?;
                for map in spec.maps {
                    record(root, &local(parent, &map.source)?, &mut files)?;
                }
            }
            Conversion::Media { .. } => {
                let spec: crate::media_mux::Specification =
                    serde_json::from_slice(&fs::read(&input)?)?;
                for path in [&spec.video, &spec.audio] {
                    record(root, &local(parent, path)?, &mut files)?;
                }
            }
            Conversion::MaterialGltf { config, .. } => gltf_inputs(
                root,
                &input,
                false,
                Materials::Index(config.material_index),
                &mut files,
            )?,
            Conversion::AnimationGltf { .. } => {
                gltf_inputs(root, &input, true, Materials::None, &mut files)?
            }
            Conversion::Mesh { config, .. }
                if is_gltf(&input) || fs::read(&input)?.starts_with(b"glTF") =>
            {
                gltf_inputs(
                    root,
                    &input,
                    true,
                    if config.materials.is_some() {
                        Materials::Used
                    } else {
                        Materials::None
                    },
                    &mut files,
                )?
            }
            Conversion::Skin { .. } if is_gltf(&input) => {
                gltf_inputs(root, &input, true, Materials::None, &mut files)?
            }
            // These adapters do not load external FBX/OBJ files or media sidecars.
            Conversion::MediaSource { .. }
            | Conversion::Video { .. }
            | Conversion::AnimationFbx { .. }
            | Conversion::Animation { .. }
            | Conversion::Texture { .. }
            | Conversion::Audio { .. }
            | Conversion::Mesh { .. }
            | Conversion::Skin { .. } => {}
        }
        result.assets.insert(asset.id.clone(), files);
    }
    for scene in &plan.scenes {
        let input = local(root, &scene.source)?;
        let spec: scene::Specification = serde_json::from_slice(&fs::read(&input)?)?;
        let mut files = BTreeMap::new();
        record(root, &input, &mut files)?;
        scripts(
            root,
            input.parent().ok_or("missing scene parent")?,
            &spec.roots,
            &mut files,
        )?;
        result.scenes.insert(scene.id.clone(), files);
    }
    Ok(result)
}

fn is_gltf(source: &Path) -> bool {
    source
        .extension()
        .and_then(|v| v.to_str())
        .is_some_and(|v| v.eq_ignore_ascii_case("gltf") || v.eq_ignore_ascii_case("glb"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn write(root: &Path, name: &str, value: Value) -> PathBuf {
        let path = root.join(name);
        fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        path
    }

    #[test]
    fn gltf_tracks_buffers_and_selected_images_including_percent_encoded_paths() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::write(root.join("geometry.bin"), [0; 4]).unwrap();
        fs::write(root.join("other.bin"), [1; 4]).unwrap();
        fs::write(root.join("color map.png"), b"image input").unwrap();
        let source = write(
            root,
            "input.gltf",
            json!({
                "asset":{"version":"2.0"},
                "buffers":[{"uri":"geometry.bin","byteLength":4},{"uri":"other.bin","byteLength":4}],
                "bufferViews":[{"buffer":0,"byteLength":4}],
                "images":[{"uri":"color%20map.png"},{"bufferView":0,"mimeType":"image/png"},{"uri":"unused-missing.png"}],
                "textures":[{"source":0},{"source":1}],
                "materials":[{"pbrMetallicRoughness":{"baseColorTexture":{"index":0}},"normalTexture":{"index":1}}]
            }),
        );
        let mut files = BTreeMap::new();
        gltf_inputs(root, &source, false, Materials::Index(0), &mut files).unwrap();
        assert_eq!(
            files.keys().cloned().collect::<Vec<_>>(),
            ["color map.png", "geometry.bin", "other.bin"]
        );
        let before = files.clone();
        fs::write(root.join("color map.png"), b"changed image").unwrap();
        gltf_inputs(root, &source, false, Materials::Index(0), &mut files).unwrap();
        assert_ne!(files["color map.png"], before["color map.png"]);
        assert_eq!(files["geometry.bin"], before["geometry.bin"]);

        files.clear();
        gltf_inputs(root, &source, true, Materials::None, &mut files).unwrap();
        assert_eq!(
            files.keys().cloned().collect::<Vec<_>>(),
            ["geometry.bin", "other.bin"]
        );
        fs::remove_file(root.join("geometry.bin")).unwrap();
        assert!(gltf_inputs(root, &source, true, Materials::None, &mut files).is_err());
    }

    #[test]
    fn records_material_media_and_nested_script_dependencies_without_unrelated_files() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        for name in [
            "color.png",
            "video.webm",
            "audio.ogg",
            "code.luau",
            "tree.rbxl",
            "unrelated",
        ] {
            fs::write(root.join(name), name).unwrap();
        }
        write(
            root,
            "material.json",
            json!({"name":"test","alphaMode":"Overlay","color":[1,1,1],"localUriPrefix":"rbxasset://test/",
            "maps":[{"source":"color.png","config":{"operation":"color","maxWidth":2,"maxHeight":2,"maxDecodedBytes":4096}}]}),
        );
        write(
            root,
            "media.json",
            json!({"video":"video.webm","audio":"audio.ogg","audioOffsetMillis":0,"maxPackets":100}),
        );
        write(
            root,
            "scene.json",
            json!({"kind":"model","roots":[{
                "id":"root","name":"root","class":"Folder","properties":{},"references":{},
                "include":{"file":"tree.rbxl","path":[]},"children":[{
                    "id":"code","name":"code","class":"ModuleScript","properties":{},"references":{},"children":[],"scriptSource":"code.luau"
                }]
            }]}),
        );
        let value = json!({"assets":[
            {"id":"material","conversion":{"kind":"material","source":"material.json"}},
            {"id":"media","conversion":{"kind":"media","source":"media.json"}}
        ],"scenes":[{"id":"scene","source":"scene.json"}]});
        let source = write(root, "build.json", value.clone());
        let plan: Plan = serde_json::from_value(value).unwrap();
        let before = collect(&source, &plan).unwrap();
        assert_eq!(before.assets["material"].len(), 2);
        assert_eq!(before.assets["media"].len(), 3);
        assert_eq!(before.scenes["scene"].len(), 3);
        fs::write(root.join("unrelated"), b"unrelated change").unwrap();
        assert_eq!(before, collect(&source, &plan).unwrap());
        fs::write(root.join("code.luau"), b"return 42").unwrap();
        let after = collect(&source, &plan).unwrap();
        assert_eq!(before.assets, after.assets);
        assert_ne!(before.scenes, after.scenes);
        fs::write(root.join("tree.rbxl"), b"rebuilt tree").unwrap();
        assert_ne!(after.scenes, collect(&source, &plan).unwrap().scenes);
        fs::remove_file(root.join("audio.ogg")).unwrap();
        assert!(collect(&source, &plan).is_err());
    }

    #[test]
    fn uri_dependencies_reject_network_and_directory_escapes() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("source");
        fs::create_dir(&root).unwrap();
        fs::write(temporary.path().join("outside.bin"), b"outside").unwrap();
        for value in [
            "https://example.com/a.bin",
            "../outside.bin",
            "%2e%2e/outside.bin",
        ] {
            assert!(uri(&root, &root, value, &mut BTreeMap::new()).is_err());
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(temporary.path().join("outside.bin"), root.join("link.bin"))
                .unwrap();
            assert!(uri(&root, &root, "link.bin", &mut BTreeMap::new()).is_err());
        }
    }
}
