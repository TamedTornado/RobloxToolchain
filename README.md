# RobloxToolchain — `roblox` CLI

General-purpose, agent-first **offline Roblox conversion and build tooling** in
Rust. Public repository: [TamedTornado/RobloxToolchain](https://github.com/TamedTornado/RobloxToolchain). Project units,
quality choices and resource budgets are configured in JSON, not tied to one game.

Builds require no Studio, Roblox credentials or Roblox servers. Deployment is a
separate, explicit CLI stage. A valid local bundle is not proof of engine playback,
rendering, collision behavior or cloud acceptance.

## Build and use

```sh
cargo build --release --locked
target/release/roblox capabilities
target/release/roblox convert mesh model.glb --config examples/conversion.json --output converted
target/release/roblox build bundle build.json --output game-bundle
target/release/roblox verify-bundle game-bundle
```

Deployment remains separate from offline builds. The credential-free
[`deploy link-scene` command](docs/deployment-linking.md) resolves prebuilt native
scene references to explicit, hash-pinned remote-ID mappings. It does not upload,
publish, or claim those caller-supplied IDs have been verified remotely.

The [deployment CLI](docs/deployment-cli.md) now uploads prebuilt dependencies,
links remote IDs and publishes to an explicitly configured destination:

```sh
roblox deploy publish game-bundle --config deployment.json --state .roblox-deploy
roblox deploy status --state .roblox-deploy
```

Reuse the state directory across rebuilds for content-keyed upload receipts,
safe resumption and exact published-version download verification. Every asset
upload is zero-price-only. Offline builds still require no network or credentials.

The executable is `roblox`. Commands return structured JSON, with JSON errors on
stderr and nonzero failure exits. New output directories must not already exist.
Bundle verification checks declared hashes and local native/TexturePack references;
it is neither authenticity verification nor an engine-acceptance test.

## Conversion and assembly

- [Static meshes](docs/offline-conversion.md): GLB/glTF, FBX and OBJ to native mesh
  files; optional [collision cooking](docs/collision-format.md), with a
  [native engine acceptance fixture](docs/engine-collision-acceptance.md) covering
  box/hull/decomposition, raycasts, mass, center of mass and inertia.
- [Skinned meshes](docs/skinned-mesh-format.md): rigid-bind GLB/glTF and supported
  linear FBX rigs, native bone hierarchy and metric transforms.
- [Animation](docs/animation-format.md): canonical clips, rigid linear glTF clips
  and explicitly sampled FBX motion; optional binding to converted rigs.
- [Textures/materials](docs/texture-conversion.md): PNG/DDS output, PBR mapping,
  semantic mip filters, native SurfaceAppearance and TexturePack descriptors.
- [Audio/video](docs/media-conversion.md): local in-process conversion and
  packaging, with explicit supported codec/container profiles.
- [Terrain](docs/terrain-format.md): configured sparse voxels or heightmaps to
  native SmoothGrid, with optional native lazy physics indexes.
- [Scenes](docs/scene-assembly.md): explicit hierarchy, properties, scripts and
  asset bindings assembled into native models/places, optionally including
  prebuilt native trees such as Rojo output. Automatic scene authoring
  from arbitrary source geometry is not implemented.
- [Luau](docs/luau-compilation.md): local compilation without executing scripts;
  source packaging is distinguished from publishable bytecode.
- [Bundles](docs/offline-bundles.md): deterministic linking, source inventories,
  optional per-asset incremental cache, integrity verification and owned rollback.

Read each profile's restrictions. Unsupported semantics fail rather than being
silently dropped or delegated to cloud conversion. Repeatability is tested for
the pinned toolchain; native codec library upgrades can change output bytes.

## Asset editing and preview retirement

The experimental `roblox-server`, HTTP snapshot protocol and Luau preview plugin
have been removed from this repository. They are not required to load or inspect
build inputs, and no preview geometry is a substitute for native build output.
Removal does not uninstall previously installed files or alter Studio sessions.

`roblox assets --plan build.json` manages entries in the same plan consumed by
`build bundle`: initialize, add, edit, list, inspect, validate and remove. See
[asset editing](docs/plan-asset-editing.md) for conversion JSON, revision checks
and dependency-aware removal. Validation executes a temporary offline build.
The old GLB catalog, `--catalog` option, preview configuration and geometry decoder
are removed; old documents are rejected rather than silently reinterpreted.

Hosted asset upload, ID mapping,
place relinking and publishing are tracked under issue 9; no automatic publication
is performed by any build command.

## Validation

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
```

Tests cover independent licensed source fixtures, native format roundtrips,
configured non-default limits, deterministic bytes, malformed inputs, CLI errors,
reference integrity, incremental builds and failure cleanup. Focused offline
integration suites also run in network-disabled namespaces. No preview server or
plugin packaging is part of these gates.
