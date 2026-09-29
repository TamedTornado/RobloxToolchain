# Terrain output inventory — issues 7 and 10

Terrain is not a triangle-mesh asset. A native Terrain instance can carry
`SmoothGrid` voxel bytes and `PhysicsGrid` bytes, a typed `MaterialColors` palette,
and water appearance properties. TerrainRegion also exposes SmoothGrid according
to the independent [format investigation](https://github.com/RobloxAPI/spec/issues/7).
That investigation does not provide a complete modern encoder specification;
the legacy ClusterGridV3 format must not be silently substituted.

Current local scene assembly preserves existing payloads as typed properties.
A pinned, independently authored Rojo terrain fixture is decoded, rebuilt under
Workspace in a native place, and decoded again. The test checks exact SmoothGrid
and PhysicsGrid bytes, material colors and water properties, plus deterministic
repeat output. See the fixture attribution for its MPL-2.0 source and the one-time
XML-to-binary normalization. CI does not run Rojo or Studio for this test.

That container test alone proves property preservation, not voxel interpretation
or engine acceptance. The version-one wire codec below now additionally decodes
and reproduces the independent payload byte-for-byte. No mesh converter output
is labeled terrain as a workaround.

## SmoothGrid version-one wire codec

`terrain_grid::{decode, encode}` operates on explicit raw cells and chunk
coordinates. It requires positive externally supplied `maxChunks` and `maxCells`
budgets on both read and write. These are resource policy, not invented terrain
dimensions. Unsupported versions, exponents, truncation, runs crossing chunk
boundaries, duplicate coordinates and unrepresentable cell channels fail.

The independent Rojo payload is 14,987 bytes: version 1, chunk exponent 5, sixteen
chunks of 32³ cells (524,288 cells total). Its chunk coordinates range from -1 to
1 in X/Z and -1 to 0 in Y. The first chunk is (-1,-1,-1), containing 32,570 cells
with raw material 0 and 198 with raw material 2. Decoding and canonical RLE
encoding reproduces **every original byte**, not merely a self-authored fixture.
This does not establish the named material mapping or engine behavior.

Wire structure established from the fixture and the installed reader:

- Byte 0 is the serialization version, byte 1 the chunk edge exponent. Edge
  length is `1 << exponent`; the native version-one reader rejects exponents >8.
- Chunks continue to the end of the payload without a chunk-count header.
- Each chunk starts with twelve coordinate-delta bytes. For shifts 24,16,8,0,
  the stream supplies X,Y,Z bytes in that order. Add each reconstructed delta to
  the previous signed 32-bit coordinate with wrapping arithmetic; initial
  coordinates are zero. These are not three contiguous big-endian integers.
- Cells are X-fastest, then Z, then Y. RLE records fill exactly `edge³` cells.
- A record header's low six bits hold a raw material slot. Bit 6 adds an explicit
  occupancy byte; otherwise the encoded occupancy defaults to 255.
- Bit 7 adds a run-count byte, with length `byte + 1` (maximum 256). Without bit
  7, length is one. The special bit-7/count-zero record consumes one additional
  auxiliary channel byte and still represents one cell, not an extended run.
- The native reader forces material-zero occupancy to zero. The auxiliary byte
  survives only for material slots >1 with occupancy !=255. The encoder rejects
  inputs that this normalization would discard instead of silently losing data.

The wire codec intentionally exposes raw material, occupancy and auxiliary bytes.
The sparse voxel adapter below supplies the separately established public material
mapping and normalized occupancy conversion. Mixed solid/liquid auxiliary-channel
authoring and newer fragment formats remain unfinished. PhysicsGrid supports the
explicit native lazy-index profile described below, not precomputed contact geometry.

## Metric sparse voxel adapter

`terrain_voxels::build` converts named sparse voxels into the wire grid. Each voxel
has an integer X/Y/Z cell offset, project material alias and normalized occupancy.
JSON configuration supplies `metresPerStud`, `voxelSizeMetres`, `originMetres`,
`alignmentToleranceMetres`, `chunkExponent`, resource `limits`, and a `materials`
map from project aliases to native names. No LayerOne dimensions or names are
embedded. Output chunks are sorted, making bytes independent of source ordering.

Native voxel resolution is four studs, as documented by Roblox's
[terrain overview](https://create.roblox.com/docs/parts/terrain), not an arbitrary
generator limit. Declared metric voxel size must match that resolution within
the explicit tolerance. The metric origin must be grid aligned; coordinates use
Euclidean division/remainders so cells at negative positions enter the right
chunks. No implicit resampling or origin snapping outside tolerance occurs.

The installed WriteVoxels implementation at `0x1444e06f0` scans the 23-element
public-enum table at `0x1484f1b80`; the matched index is the native material slot.
Cross-checking the values against the independent pinned reflection database and
the [official Material enum](https://create.roblox.com/docs/reference/engine/enums/Material)
establishes these slots in order:

`Air, Water, Grass, Slate, Concrete, Brick, Sand, WoodPlanks, Rock, Glacier, Snow,
Sandstone, Mud, Basalt, Ground, CrackedLava, Asphalt, Cobblestone, Ice, LeafyGrass,
Salt, Limestone, Pavement`.

The native method falls back to slot 2 for other enums; this converter deliberately
rejects unsupported materials instead of silently turning them into Grass. Empty
aliases, unknown aliases, duplicate cell positions and coordinate overflow fail.

At `0x1444e0b33`–`0x1444e0b98`, positive occupancy becomes single precision,
multiplies by 256, subtracts 0.5, truncates and clamps to [0,255]. Constants are
read from `0x1484f23f0` (256) and `0x1484b4450` (0.5). This is **not** rounding
`occupancy*255`: 0.5 encodes as 127. Zero occupancy produces an empty cell; a tiny
positive occupancy can produce raw occupancy zero with a non-Air material.
Inputs must be finite in [0,1]; Air with nonzero occupancy is rejected. This
profile authors one material per voxel, including water-only cells, not mixed
solid/liquid cells or edits merged into existing terrain.

Tests map every table entry independently, check quantization boundaries, metric
origins, negative chunk boundaries, native X/Z/Y order, deterministic source-order
independence and configured resource rejection. Reconstructing the independent
Rojo fixture as sparse Grass/Rock voxels and re-encoding reproduces all original
bytes. This is stronger than a self-generated roundtrip, but still not live
rendering/physics acceptance. The optional PhysicsGrid index has a separate contract.

## Heightmaps and the conversion boundary

`roblox convert terrain terrain.json --output NEW_DIRECTORY` emits
`terrain.smoothgrid` and a manifest. The input is tagged `kind: "voxels"` with
`config` and `voxels` as described above; [the example](../examples/terrain.json)
is self-contained. `kind: "heightmap"` instead takes a contained relative `source`
image path and a heightmap `config`:

- `terrain`: the same complete voxel configuration, including metric origin,
  chunking, aliases and resource budgets.
- `material`: a configured non-Air alias for the columns.
- `pixelSizeMetres`: one native voxel per pixel; resampling is not implicit.
- `floorMetres`, `heightMinMetres`, `heightMaxMetres`: vertical offsets relative
  to the configured origin. Floor must be grid-aligned; range must be finite,
  ordered and at or above the floor. Empty columns emit no occupied cells.
- `rowDirection`: `positive` or `negative` Z. Columns advance in positive X;
  pixel (0,0) starts at the configured X/Z origin. Negative rows are not silently
  rebased to a different origin.
- `maxWidth`, `maxHeight`, `maxDecodedBytes`: explicit image decode policy in
  addition to the terrain chunk/cell budgets.

The supported image profile is scalar grayscale PNG with 8- or 16-bit samples.
Values linearly interpolate the metric height range; no sRGB/luminance transform
is applied. RGB, alpha-bearing images and unapplied orientation fail. The adapter
fills columns from the floor to the sampled surface, quantizing only the final
partial cell through the established native occupancy rule. It is a local
heightfield algorithm, not a reproduction claim about Studio's terrain importer.

Bundle assets accept `{"id":"land","conversion":{"kind":"terrain","source":"terrain.json"}}`.
A Terrain scene node binds the payload through
`"assets":{"SmoothGrid":{"asset":"land","file":"terrain.smoothgrid"}}`.
The bundle source inventory includes referenced heightmap bytes, so image changes
invalidate the terrain conversion cache. Integration tests compare embedded native
Terrain bytes with standalone conversion, check cache hits/invalidation, and reject
missing inputs. Referenced image paths cannot escape the heightmap document
directory; conversion never overwrites an existing output directory.

To generate a native lazy spatial index, add `"physics":{"maxEntries":256}`
at the terrain document's top level (budget shown is an example, not a default).
This emits `terrain.physicsgrid`; bind it to the Terrain node's `PhysicsGrid`
property using the same asset-reference syntax. The manifest reports its file/hash,
`physicsGenerated: true` and `physicsKind: "native-lazy-spatial-index"`. Without
the option it emits no physics file and reports `physicsGenerated: false`.
`engineVerified` remains false in both cases. **Neither mode is yet a proven
playable terrain artifact.** No unrelated fixture data is copied into generated
terrain, and no fully precomputed collision-geometry claim is made.

### Installed-reader evidence

Read-only static inspection of `version-c792f79abddd41bd/RobloxStudioBeta.exe`,
SHA-256 `a0f2e5dfeaacc86a8329f6e41b8082940a64837dca707899a6a7350a0c9a49bf`:

- The deserialize entry at `0x143eacc80` reads the initial version. Version 1
  follows the path at `0x143eaceb5`; version 2 dispatches to `0x143eaec40`, and
  higher versions take another path. This is SmoothGrid, not ClusterGridV3.
- `0x143eaced9` reads the exponent and `0x143eacef2` applies its bound.
  `0x143eacf60`–`0x143eacfe2` accumulates interleaved coordinate deltas.
- `0x143ec9640` fills a chunk using run records from `0x143ec9510`. The latter
  implements flags 0x40/0x80, the count-zero auxiliary byte, material mask 0x3f
  and normalization. `0x143ec9773` rejects run overflow.
- The copy loops at `0x143ec9850`–`0x143ec98b6`, together with the dimension
  constructor at `0x143e76b40`, establish X/Z/Y linearization.
- A separate fragment reader at `0x143ead530` checks `VXFG` and fragment versions
  1–2, with a channel-count bound of 31. No fragment encoder is claimed here.

These observations establish a meaningful retained input format and explain the
independent fixture exactly. They are **not** execution of the proprietary reader,
live engine acceptance or proof that PhysicsGrid can be omitted. No executable
bytes, proprietary assets, decompiled source or leaked source are committed.

## PhysicsGrid version-two wire structure

`terrain_physics::{decode, encode}` now preserves the independent fixture's
PhysicsGrid as structured coordinate groups rather than an unexplained blob.
It requires an explicit positive `maxEntries` budget and rejects unknown versions,
unsupported exponents, truncation, excess counts and trailing bytes. The raw codec
preserves supplied state; the separately requested `lazy_index` generator derives
candidate regions from a new SmoothGrid as described below.

The payload starts with version byte 2 and an exponent byte (native reader bound
0–8). It then contains exactly three groups. Each group begins with a big-endian
u32 entry count, followed by that many twelve-byte coordinate deltas using the
same byte-interleaved X/Y/Z representation as SmoothGrid. The previous coordinate
resets to zero at each group boundary. Order and duplicates must be preserved;
the fixture itself contains repeated coordinates. The codec intentionally leaves
the groups unnamed rather than guessing solid/liquid semantics.

The fixture's 2,342 bytes contain exponent 3 and group counts [194,0,0]. There are
136 unique coordinates. A regression compares these coordinates with the fixture's
non-Air voxel positions expanded by one cell in every direction and bucketed into
8-cell regions: the sets match exactly. This is evidence of a spatial acceleration
relationship, **not proof that this recipe generates correct physics state for
arbitrary terrain**. Two groups are empty in this fixture; tests using authored
data additionally check their delta resets and encoding, not native semantics.

Installed executable trace (same hash as above):

- PhysicsGrid reflection registration at `0x1402b0300` binds getter thunk
  `0x140eda240` and setter thunk `0x140edb6f0`.
- RTTI identifies MegaClusterInstance's subobject offset 0x1e8 and vtable
  `0x148bd3c90`. Setter dispatch at vtable offset 0x28 reaches `0x1444d2570`.
  This method skips empty input and sends nonempty payloads to `0x1409e1fc0`.
- The latter checks version 2 and the exponent bound, then calls group readers
  `0x1409ddbf0`, `0x1409dd920` and `0x1409dd650`, in that order. All three read a
  count and interleaved coordinate deltas; they install different internal marker
  states. No triangle positions, triangle indices or mesh cooking occur in this
  wire decode path.
- Getter thunk reaches `0x1444d7870`, then serializer `0x1409e5520` on a separate
  physics structure. That serializer writes header bytes 2 and 3 at
  `0x1409e568f`/`0x1409e56d1`, then calls the same coordinate-list writer three
  times. Alternative accepted header exponents are preserved by the raw codec,
  not interpreted as an established alternative physics resolution. The
  SmoothGrid setter follows a different voxel-grid path.

### Native lazy spatial index

Further inspection establishes a supported way to populate the first group without
guessing the other groups' precise classifications:

- Group-zero decode writes internal marker 0xfe. The serializer at `0x1409e4103`
  onward writes marker-0xfe regions directly back to the first group.
- A separate region lookup path at `0x1409e63f0` dispatches marker 0xfe (and 0xff)
  to `0x1409e6740`. That invokes `0x1409e6120` to reconstruct cached masks from
  the terrain's voxel-grid object. It is not a stored triangle mesh.
- Reconstruction multiplies the region coordinates by eight, adds an eight-cell
  extent, and expands all bounds by one via `0x143e78180` before reading voxels.
  The mask builder `0x1409de140` distinguishes solid/water/material occupancy;
  this tool does not replace that on-demand engine operation with guessed masks.

The generator indexes every eight-cell region whose one-cell border can contain a
non-Air source cell. It writes sorted unique coordinates in group zero and empty
remaining groups. Air contributes no candidates; both water and solid cells do.
Explicit budgets bound entries, and coordinate/halo overflow fails. The independent
fixture's 136 unique first-group coordinates match generated candidates exactly;
its redundant entries are not duplicated by new generation.

Both wire codecs reproduce the supplied fixture exactly. Generated lazy-index
bytes also pass native place assembly alongside newly converted voxels/heightmaps.
This is a native lazy-index profile, **not fully precomputed collision geometry**:
the traced reader can perform normal on-demand mask work. Static evidence does not
prove live collision/raycast behavior, nor does it prove that omitting PhysicsGrid
entirely is equivalent. `engineVerified` remains false; offline-format acceptance
is recorded separately in the [acceptance audit](scene-terrain-acceptance.md).

[Issue 10](https://github.com/TamedTornado/RobloxToolchain/issues/10) covers the
offline terrain conversion contract audited in that report.
Rust voxel/heightmap encoding and CLI/bundle integration are implemented for the
documented version-one profile, with metric dimensions, aliases and resource
policy supplied as validated external configuration.
It is the separate terrain implementation issue requested by issue 7, not a
claim that terrain generation is finished.
