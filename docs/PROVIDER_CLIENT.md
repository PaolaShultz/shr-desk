# GP02 read-only provider client

Implemented headless DS03 client for accepted GP-2026-10-04.1 / C-AUDIO:1 GP02
snapshot **bodies**. `provider::Client` is separate from `model::Simulator`;
no Simulator enum is serialized and no writer grant or mutation is constructed.
The provider alone owns mixer/arbitration algorithms. GP02 reports target metadata,
not rendered audio. Every actual/acquisition frame/age remains null, observation
validity unavailable, rendered application/release disabled, durability volatile.
Future GP03 capabilities require separate acceptance and decoder work.

Explicit reproducible file mode (no default connection, endpoints or devices):

```sh
shr-desk --provider 11111111-1111-4111-8111-111111111111 9 \
  --snapshot-file tests/fixtures/gp02/v1/e03-final-snapshot.json \
  --select input-01 --render artifacts/provider.svg
```

Each file is one bounded message; multiple files may supply up to16 pages in any
order, completed within2s of first local receipt. The command accepts at most16
files, reads at most65537 bytes per file and rejects incomplete final sets.
Snapshot bodies have no envelope/version key in the exact accepted GP02 format;
`--provider` explicitly selects this C-AUDIO:1 body schema. Unknown fields,
duplicate JSON keys, invalid UTF8, floats, depth>12 and bytes>65536 refuse.
Mutable request/reply envelope handling, including preview tokens, is DS04 work;
request/response corpus bytes are retained with provenance, never replayed here.

The client pins the expected show and epoch before collection, validates inventory,
capacities, per-target ranges/steps/types, owner/hold, modes and automation bounds,
then atomically publishes all40 unique parameters. Mixed, duplicate or expired
pages discard the collection. Old sequence/revision cannot roll back trusted
state, including after same-epoch reconnect. Selection follows string identity
across inventory/parameter reorder. Invalid input preserves the last display as
stale. Disconnect/reconnect requires a complete newer snapshot; explicit new epoch
allows reset sequence. Local receipt freshness (250ms) describes metadata receipt,
never measured audio freshness: GP02 observations remain unavailable.

Text and SVG presentation use the same trusted nullable view. Actual, target,
proposal, owner and hold are separate; missing values say unavailable. No fixture
meters, processing curves, recorder health or zero-valued observations appear in
provider mode. `simulate` and the original gallery remain explicit synthetic modes.
`Console::reconnect_provider` discards pending local simulator intents, cancels
drafts and rearms release/pickup through shared context-loss behavior without
converting provider values into synthetic channel state. Native/provider input
integration and physical binding remain later work.

Exact copied provider data are in `tests/fixtures/gp02/v1`, including root's
ACCEPTED.json, PROVENANCE.json, schema document and original manifests. All accepted
SHA256 hashes were verified at receipt. Adversarial tests mutate in-memory copies,
never the accepted corpus. Normal tests open no devices, displays or endpoints.

## DS04 local rendered client (implemented and offline-validated)

The independent `audio` module decodes the accepted `GP03-rendered:1` extension.
The nested GP02 authority still contains unavailable `actual` values. Rendered
linear amplitude is separately typed as nanogain, never converted to an invented
integer dB reading. The extension declares `offline-unprotected` and null meters;
this is real offline coefficient rendering, with no PA or physical acceptance.

An explicit operator endpoint and bounded script select the local path:

```sh
# Endpoint must already exist under a canonical, owned0700 directory, socket0600.
# Replace show/epoch with the trusted provider launch configuration.
shr-desk --audio-local /absolute/private/session/audio.sock \
  11111111-1111-4111-8111-111111111111 9 desk-session-unique foh \
  --script /absolute/private/operator.txt
```

Example script (integer milli-dB, not decimal dB):

```text
status
grant
input-release
set input-01 fader -3000
snapshot
status
export /absolute/private/status.json
release
```

Omit `--script` for read-only status. `--script -` buffers stdin before connecting,
so waiting for human input never blocks renewal while a writer lease is held.
Scripts are bounded to64KiB,256commands and30seconds; stdin must reach EOF within10seconds before connection. This is a batch operator path, with no indefinite interactive writer claim. `wait MS` accepts0..2000ms and services snapshot/500ms renewal while waiting. `grant` is explicit; reading
or selecting never recalls a mix. Use fresh writer IDs on reconnect. Successful
queue admission is pending, not applied. Retry100/250/500ms uses the same immutable
ID/envelope, and grant/renew deadline starts at first send. A disconnect discards
local intents and requires a new identity, fresh snapshot/grant and input release.

`set INPUT pan INTEGER`, `set INPUT mute true|false`, and `send INPUT monitor-1|monitor-2 MDB`
use explicit stable inventory IDs. Monitor writers use `monitor1` or `monitor2`
and may only alter that monitor's send. `mode manual|assist` preserves engine
holds; AUTO requires explicit bounds through `json set_mode {"mode":"auto","bounds":[...]}`.
The `json KIND BODY` script command remains restricted to the reviewed C-AUDIO
commands and client validation; it is not a raw arbitrary endpoint command.
Engine-bound `preview INPUT PARAM` and `preview-send INPUT MONITOR` display provider destinations/token/240frame ramp and stage a `confirm` action. `cancel` sends engine cancel for a live release preview. Mode commands also stage `confirm`; repeating confirm after consumption refuses. Confirmation pins the reviewed revision through request construction. Sequence/frame progress alone preserves a valid preview; revision/context changes invalidate it. Strict preview DTO comes from the coordinator accepted provider schema.

Normal GP02 file mode and the default simulator are preserved. The local mode
opens one Unix socket; no TCP/device output, service installation or native GPU.
Actual accepted GP03/GP06 service execution passed with82normal tests, fmt, Clippy and release; final exact manifest/evidence remain subject to coordinator review. Physical/remote-production acceptance remains separate.


Accepted provider launch for a bounded synthetic demonstration (trusted operator
paths, fresh epoch for every authority restart):

```sh
# Create an owned private directory; retain identity files for a deliberate restart.
mkdir -m700 /absolute/private/directory
/home/shome/p/gigpies-module-planning-0006/user/providers/0008/gp03-local/bin/gigpies-headless \
  --directory /absolute/private/directory \
  --show 11111111-1111-4111-8111-111111111111 --epoch 10 --ticks 2000
# From another shell, use the --audio-local command above with EPOCH10.
# Gracefully stop/join your own child; never remove a live/unknown endpoint.
```

Actual task0008 reproduction uses private pass3/P4-A real-integration.py and
real-refusals.py. The first runs the exact accepted executable, console script
commands and a bounded owned Unix packet-loss proxy; the second checks raw bad
version/lease refusal and unchanged rendered mix. Binary SHA256 is
ad8a4a74ca426ee3d3594929b00fca5daa2603d5c7ddfb7aef542d5782fa9b0a.
No generated executable or private integration logs belong in Git.
