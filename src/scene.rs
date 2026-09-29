//! Offline native model/place serialization from explicitly typed scene data.
use crate::Result;
use rbx_dom_weak::{
    InstanceBuilder, WeakDom,
    types::{Content, ContentId, NetAssetRef, Ref, SharedString, Variant, VariantType},
};
use rbx_reflection::{PropertyDescriptor, PropertyKind, PropertySerialization};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Specification {
    pub kind: Kind,
    pub roots: Vec<Node>,
    pub script_compiler: Option<crate::scripts::Config>,
    #[serde(default)]
    pub animation_bindings: Vec<crate::rig_binding::Binding>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Model,
    Place,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Node {
    pub id: String,
    pub class: String,
    pub name: String,
    pub properties: BTreeMap<String, Variant>,
    pub references: BTreeMap<String, String>,
    pub children: Vec<Node>,
    pub script_source: Option<PathBuf>,
    #[serde(default)]
    pub assets: BTreeMap<String, AssetReference>,
    pub material: Option<AssetReference>,
    pub rig: Option<AssetReference>,
    pub include: Option<Include>,
}

/// Children copied from a prebuilt native model or place, such as a Rojo build.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Include {
    /// Binary RBXM/RBXL file under the scene source directory.
    pub file: PathBuf,
    /// Instance names from the file's root to the instance whose children are
    /// copied. Empty selects the file's root instances.
    pub path: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetReference {
    pub asset: String,
    pub file: String,
}

pub struct ResolvedAsset {
    pub uri: String,
    pub path: PathBuf,
    pub animation_rig: Option<crate::animation::Rig>,
}

pub type AssetMap = BTreeMap<AssetReference, ResolvedAsset>;

fn property_descriptor(
    class: &str,
    property: &str,
) -> Result<&'static PropertyDescriptor<'static>> {
    let database = rbx_reflection_database::get_bundled();
    let mut current = Some(class);
    while let Some(name) = current {
        let descriptor = database
            .classes
            .get(name)
            .ok_or_else(|| format!("unknown scene class: {name}"))?;
        if let Some(descriptor) = descriptor.properties.get(property) {
            return Ok(descriptor);
        }
        current = descriptor.superclass;
    }
    Err(format!("unknown property {class}.{property}").into())
}

fn property_type(class: &str, property: &str) -> Result<VariantType> {
    Ok(property_descriptor(class, property)?.data_type.ty())
}

/// Resolve names which actually reach the file, not just names in the API.
fn serialized_names(
    class: &str,
    property: &str,
    visiting: &mut HashSet<String>,
) -> Result<Vec<String>> {
    if !visiting.insert(property.to_owned()) {
        return Err(format!("cyclic property serialization: {class}.{property}").into());
    }
    let descriptor = property_descriptor(class, property)?;
    let result = match &descriptor.kind {
        PropertyKind::Alias { alias_for } => serialized_names(class, alias_for, visiting)?,
        PropertyKind::Canonical { serialization } => match serialization {
            PropertySerialization::Serializes => vec![property.to_owned()],
            // SerializesAs targets are storage descriptors and need not be
            // independently serializable through the public API.
            PropertySerialization::SerializesAs(name) => vec![(*name).to_owned()],
            PropertySerialization::Migrate(migration) => {
                let mut names = Vec::new();
                for target in migration.new_property_names() {
                    names.extend(serialized_names(class, target, visiting)?);
                }
                names
            }
            PropertySerialization::DoesNotSerialize => {
                return Err(format!("property {class}.{property} does not serialize").into());
            }
            _ => return Err("unsupported property serialization rule".into()),
        },
        _ => return Err("unsupported property descriptor kind".into()),
    };
    visiting.remove(property);
    Ok(result)
}

fn validate_properties(node: &Node) -> Result<()> {
    let mut destinations = HashMap::new();
    for property in node
        .properties
        .keys()
        .chain(node.references.keys())
        .chain(node.assets.keys())
    {
        for destination in serialized_names(&node.class, property, &mut HashSet::new())? {
            if let Some(previous) = destinations.insert(destination.clone(), property) {
                return Err(format!(
                    "scene properties {previous} and {property} both serialize to {destination}"
                )
                .into());
            }
        }
    }
    for (property, value) in &node.properties {
        let expected = property_type(&node.class, property)?;
        if expected != value.ty() {
            return Err(format!(
                "property {}.{property} requires {expected:?}, got {:?}",
                node.class,
                value.ty()
            )
            .into());
        }
    }
    for property in node.references.keys() {
        if property_type(&node.class, property)? != VariantType::Ref {
            return Err(format!(
                "property {}.{property} is not an instance reference",
                node.class
            )
            .into());
        }
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildResult {
    pub format: &'static str,
    pub sha256: String,
    pub instances: usize,
    pub bytes: usize,
    pub published: bool,
}

fn insert(
    dom: &mut WeakDom,
    parent: Ref,
    node: &Node,
    root: &Path,
    ids: &mut HashMap<String, Ref>,
    compiler: Option<&crate::scripts::Config>,
    assets: &AssetMap,
) -> Result<()> {
    if node.id.is_empty() || node.class.is_empty() {
        return Err("scene node id and class must be nonempty".into());
    }
    let database = rbx_reflection_database::get_bundled();
    if !database.classes.contains_key(node.class.as_str()) {
        return Err(format!("unknown scene class: {}", node.class).into());
    }
    validate_properties(node)?;
    if ids.contains_key(&node.id) {
        return Err(format!("duplicate scene id: {}", node.id).into());
    }
    if node.class == "DataModel" {
        return Err("DataModel is the implicit root; do not nest it".into());
    }
    let mut builder = InstanceBuilder::new(node.class.as_str()).with_name(&node.name);
    if let Some(rig) = &node.rig {
        if node.class != "MeshPart" || node.children.iter().any(|child| child.class == "Bone") {
            return Err(
                "native rig attachment requires a MeshPart without explicit Bone children".into(),
            );
        }
        let mesh = node
            .assets
            .get("MeshContent")
            .ok_or("rig attachment requires an explicit MeshContent asset binding")?;
        if mesh.asset != rig.asset || rig.file != "rig.rbxm" || mesh.file == rig.file {
            return Err(
                "rig attachment must use the same converted skin asset as MeshContent".into(),
            );
        }
        let asset = assets.get(rig).ok_or("missing native rig asset")?;
        for bone in crate::rig_asset::load(&asset.path)? {
            builder = builder.with_child(bone);
        }
    }
    if let Some(material) = &node.material {
        if node.class != "MeshPart"
            || node
                .children
                .iter()
                .any(|child| child.class == "SurfaceAppearance")
        {
            return Err(
                "material attachment requires a MeshPart without another SurfaceAppearance".into(),
            );
        }
        let asset = assets
            .get(material)
            .ok_or_else(|| format!("missing local material binding: {material:?}"))?;
        let native = rbx_binary::from_reader(fs::File::open(&asset.path)?)?;
        if native.root().children().len() != 1 {
            return Err("material artifact must contain exactly one SurfaceAppearance".into());
        }
        let appearance = native
            .get_by_ref(native.root().children()[0])
            .ok_or("missing material root")?;
        if appearance.class.as_str() != "SurfaceAppearance" || !appearance.children().is_empty() {
            return Err("material artifact must contain a childless SurfaceAppearance".into());
        }
        builder = builder.with_child(
            InstanceBuilder::new("SurfaceAppearance")
                .with_name(&appearance.name)
                .with_properties(
                    appearance
                        .properties
                        .iter()
                        .map(|(name, value)| (*name, value.clone())),
                ),
        );
    }
    for (name, value) in &node.properties {
        if name == "Name"
            || name == "Parent"
            || matches!(value, Variant::Ref(_))
            || node.references.contains_key(name)
            || node.assets.contains_key(name)
        {
            return Err(format!(
                "property {name} conflicts with explicit scene structure/references"
            )
            .into());
        }
        if name == "Source" {
            let Variant::String(source) = value else {
                return Err("script Source must be a string".into());
            };
            validate_script(source, &node.class, &node.id, compiler)?;
        }
        builder = builder.with_property(name.as_str(), value.clone());
    }
    for (property, reference) in &node.assets {
        if node.references.contains_key(property)
            || matches!(property.as_str(), "Name" | "Parent" | "Source")
        {
            return Err(format!("asset binding conflicts with scene property {property}").into());
        }
        let asset = assets.get(reference).ok_or_else(|| {
            format!("missing local asset binding: {reference:?}; use build bundle")
        })?;
        let value = match property_type(&node.class, property)? {
            VariantType::Content => Variant::Content(Content::from_uri(&asset.uri)),
            VariantType::ContentId => Variant::ContentId(ContentId::from(asset.uri.clone())),
            VariantType::SharedString => {
                Variant::SharedString(SharedString::new(fs::read(&asset.path)?))
            }
            VariantType::NetAssetRef => {
                Variant::NetAssetRef(NetAssetRef::new(fs::read(&asset.path)?))
            }
            VariantType::BinaryString => Variant::BinaryString(fs::read(&asset.path)?.into()),
            other => {
                return Err(format!("property {property} cannot bind an asset ({other:?})").into());
            }
        };
        builder = builder.with_property(property.as_str(), value);
    }
    if let Some(source) = &node.script_source {
        if !matches!(
            node.class.as_str(),
            "Script" | "LocalScript" | "ModuleScript"
        ) || node.properties.contains_key("Source")
        {
            return Err(
                "scriptSource requires a script class without an inline Source property".into(),
            );
        }
        let path = contained(root, source).map_err(|e| {
            format!("scriptSource must remain under the scene source directory: {e}")
        })?;
        let source = fs::read_to_string(path)?;
        validate_script(&source, &node.class, &node.id, compiler)?;
        builder = builder.with_property("Source", source);
    }
    let reference = dom.insert(parent, builder);
    ids.insert(node.id.clone(), reference);
    if let Some(include) = &node.include {
        insert_included(dom, reference, include, root, compiler)?;
    }
    for child in &node.children {
        insert(dom, reference, child, root, ids, compiler, assets)?;
    }
    Ok(())
}

fn is_script(class: &str) -> bool {
    matches!(class, "Script" | "LocalScript" | "ModuleScript")
}

fn validate_script(
    source: &str,
    class: &str,
    label: &str,
    compiler: Option<&crate::scripts::Config>,
) -> Result<()> {
    if !is_script(class) {
        return Err("Source requires a supported script class".into());
    }
    let compiler = compiler
        .ok_or("scene containing script source requires explicit scriptCompiler configuration")?;
    crate::scripts::compile(source, compiler)
        .map_err(|e| format!("script {label} failed compilation: {e}"))?;
    Ok(())
}

/// Resolve a scene-relative input file that must stay under the scene directory.
pub(crate) fn contained(root: &Path, relative: &Path) -> Result<PathBuf> {
    let path = root.join(relative).canonicalize()?;
    if relative.is_absolute() || !path.starts_with(root) || !path.is_file() {
        return Err(format!(
            "{} is not a regular file under the scene source directory",
            relative.display()
        )
        .into());
    }
    Ok(path)
}

fn insert_included(
    dom: &mut WeakDom,
    parent: Ref,
    include: &Include,
    root: &Path,
    compiler: Option<&crate::scripts::Config>,
) -> Result<()> {
    let label = include.file.display().to_string();
    let path = contained(root, &include.file).map_err(|e| format!("include {label}: {e}"))?;
    let native = rbx_binary::from_reader(fs::File::open(&path)?)
        .map_err(|e| format!("include {label} is not a binary model or place: {e}"))?;
    let mut selected = native.root_ref();
    for name in &include.path {
        let parent = native
            .get_by_ref(selected)
            .ok_or("include selection vanished")?;
        let matches: Vec<Ref> = parent
            .children()
            .iter()
            .copied()
            .filter(|child| native.get_by_ref(*child).is_some_and(|i| i.name == *name))
            .collect();
        selected = match matches.as_slice() {
            [only] => *only,
            [] => {
                return Err(
                    format!("include {label} has no instance at path segment {name}").into(),
                );
            }
            _ => {
                return Err(format!("include {label} path segment {name} is ambiguous").into());
            }
        };
    }
    let sources = native
        .get_by_ref(selected)
        .ok_or("include selection vanished")?
        .children()
        .to_vec();
    if sources.is_empty() {
        return Err(format!("include {label} selects no instances").into());
    }
    let mut copied = HashMap::new();
    for source in sources {
        copy_included(&native, source, dom, parent, &label, compiler, &mut copied)?;
    }
    // Instance references are remapped after every copied instance exists.
    for (&old, &new) in &copied {
        let instance = native.get_by_ref(old).ok_or("included instance vanished")?;
        for (name, value) in &instance.properties {
            let Variant::Ref(target) = value else {
                continue;
            };
            let mapped = if target.is_none() {
                Ref::none()
            } else {
                *copied.get(target).ok_or_else(|| {
                    format!(
                        "include {label}: {}.{name} references an instance outside the included tree",
                        instance.name
                    )
                })?
            };
            dom.get_by_ref_mut(new)
                .ok_or("copied instance vanished")?
                .properties
                .insert(*name, Variant::Ref(mapped));
        }
    }
    Ok(())
}

fn copy_included(
    native: &WeakDom,
    source: Ref,
    dom: &mut WeakDom,
    parent: Ref,
    label: &str,
    compiler: Option<&crate::scripts::Config>,
    copied: &mut HashMap<Ref, Ref>,
) -> Result<()> {
    let instance = native
        .get_by_ref(source)
        .ok_or("included instance vanished")?;
    let class = instance.class.as_str();
    if class == "DataModel" {
        return Err(format!("include {label} cannot copy a DataModel").into());
    }
    if !rbx_reflection_database::get_bundled()
        .classes
        .contains_key(class)
    {
        return Err(format!("include {label} contains unknown class {class}").into());
    }
    if is_script(class) {
        match instance.properties.get(&"Source".into()) {
            Some(Variant::String(text)) => validate_script(
                text,
                class,
                &format!("{} in include {label}", instance.name),
                compiler,
            )?,
            None => {}
            Some(other) => {
                return Err(format!(
                    "include {label}: {} Source has unexpected type {:?}",
                    instance.name,
                    other.ty()
                )
                .into());
            }
        }
    }
    let builder = InstanceBuilder::new(class)
        .with_name(&instance.name)
        .with_properties(
            instance
                .properties
                .iter()
                .filter(|(_, value)| !matches!(value, Variant::Ref(_)))
                .map(|(name, value)| (*name, value.clone())),
        );
    let reference = dom.insert(parent, builder);
    copied.insert(source, reference);
    for &child in instance.children() {
        copy_included(native, child, dom, reference, label, compiler, copied)?;
    }
    Ok(())
}

fn link(dom: &mut WeakDom, node: &Node, ids: &HashMap<String, Ref>) -> Result<()> {
    let reference = ids[&node.id];
    for (property, target) in &node.references {
        if property == "Name" || property == "Parent" {
            return Err("references cannot override Name or Parent".into());
        }
        let target = *ids
            .get(target)
            .ok_or_else(|| format!("missing scene reference: {target}"))?;
        dom.get_by_ref_mut(reference)
            .ok_or("scene instance vanished")?
            .properties
            .insert(property.as_str().into(), Variant::Ref(target));
    }
    for child in &node.children {
        link(dom, child, ids)?;
    }
    Ok(())
}

pub fn build(source: &Path, output: &Path) -> Result<BuildResult> {
    build_with_assets(source, output, &AssetMap::new())
}

pub fn build_with_assets(source: &Path, output: &Path, assets: &AssetMap) -> Result<BuildResult> {
    let source = source.canonicalize()?;
    let root = source.parent().ok_or("scene source directory missing")?;
    let specification: Specification = serde_json::from_slice(&fs::read(&source)?)?;
    if specification.roots.is_empty() {
        return Err("scene needs at least one root instance".into());
    }
    let format = match specification.kind {
        Kind::Model => "rbxm",
        Kind::Place => "rbxl",
    };
    if output.extension().and_then(|v| v.to_str()) != Some(format) {
        return Err(format!("output extension must be .{format} for this scene kind").into());
    }
    let mut dom = WeakDom::new(InstanceBuilder::new("DataModel"));
    let mut ids = HashMap::new();
    let parent = dom.root_ref();
    for node in &specification.roots {
        insert(
            &mut dom,
            parent,
            node,
            root,
            &mut ids,
            specification.script_compiler.as_ref(),
            assets,
        )?;
    }
    for node in &specification.roots {
        link(&mut dom, node, &ids)?;
    }
    let mut pending: Vec<_> = specification.roots.iter().collect();
    while let Some(node) = pending.pop() {
        for reference in node.assets.values() {
            if assets
                .get(reference)
                .is_some_and(|asset| asset.animation_rig.is_some())
                && !specification
                    .animation_bindings
                    .iter()
                    .any(|binding| binding.animation == *reference)
            {
                return Err(format!(
                    "source animation asset {reference:?} requires an animationBindings target rig"
                )
                .into());
            }
        }
        pending.extend(&node.children);
    }
    for binding in &specification.animation_bindings {
        crate::rig_binding::validate(binding, &dom, &ids, assets)?;
    }
    let mut bytes = Vec::new();
    // Cargo enables the library's always-bundled feature, including its constructor.
    // Neither RBX_DATABASE nor a user's local reflection cache affects this build.
    rbx_binary::Serializer::new()
        .reflection_database(rbx_reflection_database::get_bundled())
        .serialize(&mut bytes, &dom, dom.root().children())?;
    let result = BuildResult {
        format,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        instances: dom
            .descendants()
            .filter(|node| node.referent() != dom.root_ref())
            .count(),
        bytes: bytes.len(),
        published: false,
    };
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist_noclobber(output)?;
    Ok(result)
}
