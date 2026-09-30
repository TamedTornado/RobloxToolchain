# Native deployment CLI — issue 9

Deployment consumes a verified, immutable offline bundle. It never executes a
conversion recipe or sends source GLB/FBX files to Roblox's importer. Build/CI
checks remain credential-free and network-free; only explicit `deploy` cloud
commands contact Roblox.

Build and install the executable with `cargo build --release --locked`, then
copy `target/release/roblox` to a directory on PATH (for example `~/.local/bin`).

```sh
roblox verify-bundle game-bundle
roblox deploy upload game-bundle --config deployment.json --state .roblox-deploy
roblox deploy publish game-bundle --config deployment.json --state .roblox-deploy
roblox deploy status --state .roblox-deploy
```

`upload` uploads dependencies and writes a linked place, but does not publish.
`publish` performs/resumes those same steps and then publishes the place. Running
only `publish` is sufficient for the ordinary build/deploy loop. The state path
must be outside the source bundle, with an existing parent directory. Existing
unrelated nonempty directories are rejected, not repurposed.

## Configuration

Copy `examples/deployment.json` and replace the example IDs and artifact names
with the actual destination and entries from the bundle manifest. Choose exactly
one creator `userId` or `groupId`. The configured scene must be an RBXL place.
The key file path is relative to the configuration file unless absolute; shell
tilde expansion is not performed. On Unix it must be a regular owner-only file
(for example `chmod 600`). Keys never appear in command arguments or receipts.

The API key needs asset read/write for the selected creator, place publishing for
the selected universe, asset-permissions write, and asset delivery access for
exact-version download verification. An explicit destination is required; no
experience is created or guessed.

Before publishing, the CLI grants the destination universe **Use** permission on
every uploaded asset (the asset-permissions API, in batches; idempotent, so it
runs on every publish). Without it a live game server and its clients were refused
the creator's own freshly uploaded meshes, images and sounds (`Could not fetch`
from `assetdelivery.roblox.com/v1/asset`) even though Studio, signed in as the
creator, loaded them. Found on 2026-09-30, the first live session of the
Sweatshop experience; granting use fixed it without republishing. No other
account permissions are changed.

Each upload selects an exact manifest `asset`/`file` and explicitly declares its
type, display name and description. Supported deployment types are Mesh (native
v2/v4.01), Image (PNG), Audio (Ogg), Animation (native KeyframeSequence RBXM), Model
(native RBXM), and TexturePack (the toolchain's local descriptor profile). DDS and
video uploading are excluded. Physics/terrain data embedded in the scene do not
need independent uploads. Material RBXM used only for local attachment need not
be uploaded separately; select the content dependencies actually referenced by
the final scene. Extra selected assets are uploaded intentionally, such as a
separate model-library export.

The CLI discovers dependencies from typed native content properties and pack
descriptors, checks every required selection and cycles before making writes,
and uploads in dependency order regardless of configuration order. Pack linking
changes local map references into numeric asset IDs; native model/place linking
changes only typed content references. It never rewrites script text or embedded
collision/terrain bytes. Already-external references remain external.

All timeouts, poll intervals, read-attempt counts and response/upload byte limits
are required positive JSON policy. The example values are examples, not hidden
runtime defaults. Roblox's own upload quotas and limits still apply.

## Receipts, iteration and failures

- `receipts.json` is an atomically replaced, synced journal. An OS file lock
  prevents simultaneous writers using the same state directory.
- Receipts bind the API origin, creator and destination. Reusing them for another
  authority fails. Use a separate state directory for a different destination.
- Upload reuse is keyed by the exact request metadata and linked payload hash.
  Reuse the state directory across rebuilds: unchanged payloads reuse their IDs;
  changed payloads receive new IDs. Cached assets are rechecked for creator,
  type, active state and moderation approval before reuse.
- Upload operation completion and moderation approval are separate waits. Their
  timeouts leave resumable receipts. Repeating the same command resumes the
  operation instead of blindly uploading again.
- Every create request specifies `expectedPrice: 0`. There is no paid fallback.
- Native place publication records its returned version and downloads that
  exact version to compare SHA-256 with the linked bytes. A failed verification
  is a failure, not a successful publish result. The version receipt is retained
  so verification can be retried without another publication.
- An identical previously verified publication is reused. This attests to that
  specific version, not that nobody subsequently published another version using
  a different tool. Changed linked content publishes a new version.
- HTTP and operation errors are reported with their concrete response details.
  Transport errors preserve their cause chain while redacting the API key.
  Reads retry according to policy. Writes are never blindly retried after an
  uncertain network/server failure. A definite 409/429 or other non-408 client
  rejection leaves the write eligible for a later explicit rerun.

For a definitively failed operation, explicitly schedule a retry after addressing
its cause, then run `upload` or `publish` again:

```sh
roblox deploy retry-upload game-bundle --config deployment.json --state .roblox-deploy --receipt RECEIPT_HASH
```

For an ambiguous write, inspect `deploy status` and identify its actual server
operation/version. Do not guess IDs or delete the journal to conceal uncertainty.

```sh
roblox deploy reconcile-operation game-bundle --config deployment.json --state .roblox-deploy --receipt RECEIPT_HASH --operation-id OPERATION_ID
roblox deploy reconcile-publication game-bundle --config deployment.json --state .roblox-deploy --sha256 LINKED_PLACE_HASH --version VERSION
```

Operation reconciliation explicitly associates the operator-selected operation
with the pending upload and checks its resulting creator/type/moderation. Roblox
transcoding means it cannot prove original uploaded bytes for every asset type;
the output labels this operator association. Publication reconciliation requires
an exact byte match. Neither command resubmits a write.

## Readiness and security boundaries

Approved assets can still require Roblox's on-demand compressed material
representation generation. The CLI reports `runtimeRepresentationsVerified:false`
and `engineVerified:false`: moderation and exact place-byte verification are not
pixel/physics tests. The separate published PBR runtime fixture establishes the
supported rendering path; do not turn a timeout into a false engine pass.

The production API origin is fixed to Roblox's HTTPS authority; an explicit
loopback HTTP origin is allowed for local integration tests. Redirects are not
followed. Signed asset download URLs are accepted only from Roblox CDN hosts
(or the same loopback test origin), and the API key is never attached to CDN
requests. Credentials and public-network calls are absent from CI tests.
