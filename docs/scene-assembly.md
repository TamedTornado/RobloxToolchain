# Offline native scene assembly — issue 7

The stated offline assembly requirements are satisfied; see the
[requirement-by-requirement acceptance audit](scene-terrain-acceptance.md).
Engine execution and automatic authoring of scene placement are separate claims.

`roblox build scene scene.json --output model.rbxm` constructs a native model
without Studio. Use `kind: "place"` and `.rbxl` output for a place artifact.
The root DataModel is implicit. [Example input](../examples/scene.json).

Each node has a stable input id, class, name, typed property map, reference map
and children. Property values use `rbx_types::Variant` JSON serialization, not
string guessing or inferred numeric types. Classes/properties are checked against
the bundled reflection database. References resolve by input id after all nodes
exist; unknown targets and duplicate ids fail. Name/Parent cannot be overridden
by arbitrary property values. Output creation is atomic and does not overwrite.

Admission checks the reflection serialization rule as well as the API name.
Runtime-only properties are rejected rather than silently dropped; aliases and
migrations cannot write the same serialized destination twice. Explicit values
must match the property's declared type, and instance references require a Ref
property. Regressions cover runtime-only mass, competing old/new animation
content properties, wrong value types and references assigned to booleans.

`scriptSource` may name a UTF-8 source file under the scene document directory for
Script, LocalScript or ModuleScript. Absolute paths and symlink/directory escapes
fail. A scene containing file or inline script source requires a `scriptCompiler`
object with the four levels shown in [compiler configuration](../examples/compiler.json).
Each source is compiled locally as a validation gate, then the original text is
packed into the native Source property. Source is never executed. This is syntax
and compilation validation, not whole-project type checking or runtime testing.

## Including prebuilt native trees

A node may copy the children of an instance in a prebuilt binary RBXM/RBXL, such
as the output of a separate `rojo build`. This keeps Luau project layout, live
sync and luau-lsp sourcemaps in Rojo while this toolchain still converts assets,
assembles the place and prepares deployment. The toolchain never runs Rojo; the
project's own build script runs `rojo build` first.

```json
{"id":"server","class":"ServerScriptService","name":"ServerScriptService",
 "properties":{},"references":{},"children":[],
 "include":{"file":"build/code.rbxl","path":["ServerScriptService"]}}
```

`file` follows the same containment rule as `scriptSource`. `path` names instances
from the file's root to the instance whose children are copied; each segment
must match exactly one child, and an empty path copies the file's root instances.
An empty selection, a DataModel, or a class unknown to the bundled reflection
database fails. Included instances are placed under the node, before its explicit
children, with their native properties unchanged. Instance references inside the
copied tree are remapped; a reference to anything outside it fails rather than
being dropped. Included scripts go through the same mandatory compilation gate,
so a scene with an include that contains script Source needs `scriptCompiler`.
Included instances have no scene ids, so explicit scene references cannot target
them. Bundle source inventories record the included file for change detection
and cache invalidation.

The fixture under `tests/fixtures/rojo-include` is real Rojo 7.7.0 output. Tests
cover service-level inclusion with repeatable bytes, reference remapping, and
rejection of escapes, non-native files, missing/ambiguous/empty selections,
unknown fields, uncompiled and invalid scripts. This is native serialization
evidence, not a live Studio acceptance test of the included scripts.

## Serialization

This uses the MIT rbx-dom/rbx_binary libraries, not a Rojo subprocess. Repeated
builds are tested for identical bytes, followed by native deserialization checks
on hierarchy, properties, scripts and references. The schema is general, with no
LayerOne names or conventions.

Explicit pivot regression covers Model.WorldPivotData, MeshPart.PivotOffset,
MeshPart.CFrame/Size and a nested model's PrimaryPart reference. Nonidentity
rotation, offset and world placement survive native serialization unchanged;
the writer does not apply the parent model pivot a second time to the child.

An independently authored native terrain fixture additionally verifies exact
voxel/physics bytes, material palette and water properties through place assembly.
See [terrain inventory](terrain-format.md) and its separate generation issue.

## Animation target compatibility

Scenes linking an `animationGltf` or `animationFbx` asset must declare an
`animationBindings` entry with `animation` (the usual asset/file reference),
`root` (scene id of the root Bone), and positive `rigidTolerance` below one.
For imported native rigs, `root` can identify the containing MeshPart and `bone`
selects its uniquely named direct root Bone. No generated scene ids are invented.
The builder receives the exact rig metadata from the conversion result; it does
not infer an association from filenames, reread a stale external manifest, or
trust a caller-provided matching hash alone.

The target root must be directly under its MeshPart. Every bone name, parent
relationship and local rest CFrame must match the source rig; the source's
`rootParentCframe` is folded into the root rest exactly once. Duplicate names,
missing/extra bones, non-rigid matrices, missing metadata and omitted required
bindings fail before scene serialization. Bone ancestry outside the selected
root cannot quietly add another transform. Non-Bone intermediary hierarchies
are rejected. The tolerance is explicit numerical admission, not a hidden limit.

This is rest-space validation, not retargeting or animation
execution. A skin's inverse-bind rest may differ from the source animation's
rest; that mismatch must be resolved deliberately. Canonical authored animation
JSON has no imported source rig metadata and cannot satisfy this source-rig
binding check; it remains usable as a separately authored artifact, without an
imported-rig compatibility claim.

Integration tests use the independent Khronos glTF and Maya/ufbx FBX fixtures.
They verify successful native scene builds and reject omitted root context,
wrong names/counts, reparented bones with unchanged names/count, invalid tolerance,
missing references and omitted declarations. Failed bundles remove their own
partial outputs. These checks do not establish current engine animation playback.

## Outstanding integration and reproducibility work

Bundle assembly supports explicit mesh/collision/texture bindings and native
material attachment; see [offline bundles](offline-bundles.md). It does not yet
automatically turn a geometry conversion manifest into properly sized/pivoted
MeshParts. Native Bone creation/attachment is implemented through the `rig` asset
reference. Explicit `bindTo` conversion can rebase source-rest animation onto the
skin bind pose while preserving motion; see [animation rebasing](animation-format.md#explicit-skin-bind-pose-rebasing).
The independent Khronos fixture demonstrates that these rests can differ: the
unbound pairing still fails while the explicitly rebased pairing passes without
weakening scene validation. Terrain payload generation is now
tracked separately in issue 10; preservation of supplied native terrain data is
tested. Plain instance serialization does not prove the remaining integration
work complete. No remote asset ids are generated here
and no game is published.

Serialization uses the pinned bundled reflection database. The dependency's
`debug_always_use_bundled` Cargo feature bypasses local lookup in both debug and
release builds, despite its name. A subprocess regression supplies a deliberately
invalid `RBX_DATABASE` file and proves identical output rather than relying on a
clean environment. This addresses the reflection lookup boundary, not all of
issue 8's remaining build/link/script requirements.
