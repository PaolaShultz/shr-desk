# SHR Desk: GigPies implementation

Current software scope **2026-10-04 / task0009**: DS-01..04 are implemented,
including the DS-02 native frontend and actual GP03/GP09 software integration.
DS-05 implements read-only accepted GP05 module health; final validation is recorded
in [status](STATUS.md). Physical controller/window/LED behavior remains unverified.
The original planning baseline is GP-2026-10-04.1. [Central inventory](https://github.com/PaolaShultz/gigpies/blob/main/docs/MODULE_IMPLEMENTATION_MAP.md) ·
[Agreed contracts](https://github.com/PaolaShultz/gigpies/blob/main/docs/MODULE_CONTRACTS.md). Existing product roadmaps remain authoritative for
unrelated work; this plan owns only the GigPies integration increments below.

## Objective and boundary

Own audio surface actions, navigation, presentation and controller translation. Never mix/process/record audio or arbitrate automation here. Preserve 1920×1080 scene/font model and independent audio role; full keyboard/manual operation precedes automation. The optional native renderer shares the same confirmed provider/action scene.

## Source and evidence reviewed

Repository: `/home/shome/p/shr-desk`. Inspected HEAD: `UNBORN: no commit exists`.
All initial source is untracked; preserve it. Exact pre-plan source manifest SHA-256: `3d6be3b9d32f26a0c3da36b4117132523b3f4612b181a0b2571bde43b8620258`.

Owning documents: README.md; docs/STATUS.md, BLUEPRINT.md, CONTROL_CONTRACT.md, CONTROLLER.md, SCREENS.md, DEVELOPMENT.md.

Source inspected: `src/model.rs::{Desk,Simulator,Edit,Snapshot}`, `src/midi.rs::{Decoder,Pickup}`, `src/main.rs`, `src/render.rs`; `tests/contracts.rs`, `tests/cli.rs`.

`Desk` and `Simulator` implement one pending command, 64-response cache,
36 synthetic channels, selection, holds and immediate fader simulation. MIDI has
pickup/relative/LED byte primitives; render exports three SVG drafts. Seventeen
normal tests previously passed. There is no socket, lease implementation, engine
ramp, native GPU backend, device worker or real measurement graph.

These are source inspection and previously recorded results, not fresh builds or
physical acceptance. The planning session runs documentation checks only.

## Milestones and tasks

Implemented software includes the action/focus workflow, optional native rendering,
real read-only attach and reviewed GP03 commands, GP09 role leases and read-only
GP05 provider health. Physical acceptance and writable module extensions remain
separate. The table below preserves the original launch dependencies.

Task states are execution dependencies: READY has no missing software provider;
WAITING names its precise prerequisite; DEFERRED has an activation condition.
Source delivery and build reservation are additional launch prerequisites on a
peer. Every row has one owner, the repository named in its Owner column. A later
task starts only after the previous artifact is reviewed, never merely delivered.

| Task / priority / state | Owner | Work area, inputs and required artifact | Output and measurable acceptance |
|---|---|---|---|
| DS-01 / P0 / READY | SHR Desk | `src/main.rs`, `model.rs`, `midi.rs`, new `src/actions.rs`, tests/contracts.rs and CLI tests. Inputs existing D2 blueprint and C-ROLE presentation policy; no provider required. | One semantic action table for select/bank/page/edit/confirm/cancel/back; detached typed draft, explicit hold/release/mode confirmation. Page/selection/disconnect invalidates draft and pickup. Keyboard and injected MIDI produce same commands. Unsupported controls remain unavailable; simulator label remains. |
| DS-02 / P1 / WAITING | SHR Desk | DS-01 accepted action model; C-ROLE:1 E02 for abstract assignment events. `render.rs`, new native frontend and worker seams. | Implement winit/wgpu prototype over existing primitives/font, resize/font/device-loss recovery and bounded input/LED queues. Keep headless backend. Actual device binding waits GP-09 artifact and GP-H2; graphics dependencies need resource review. Headless model work can proceed without display. |
| DS-03 / P1 / WAITING | SHR Desk | GP-01 E01/E02 + GP-02 accepted C-AUDIO:1/E03 corpus; `model.rs` adapter seam, new protocol/adapter modules. | Split client from Simulator; bounded read-only snapshot decoder, actual/proposed/target/freshness display. Validate wrong show/version, malformed snapshots and reconnect. Decoder work can use the GP-02 corpus; real read-only authority integration additionally waits GP-03, and remote endpoint waits GP-06. |
| DS-04 / P1 / OFFLINE LOCAL VALIDATED | SHR Desk | GP-03 authoritative E03 tests and DS-03. Live remote use additionally GP-06. | Scoped fader/pan/mute, send/hold/release through real engine adapter; applied/frame/ramp status, uncertain ACK recovery and no replay. Never send simulator enums as wire API. |
| DS-05 / P2 / WAITING | SHR Desk | GP-05 accepted C-REC:1/C-PA:1/C-FX:1 descriptors + DS-03; mutable subfeatures need respective provider versions. | Read-only recorder/PA/FX pages first, then only advertised commands; written frames distinct from packet count; unsupported fixed-v1 edits disabled. |
| DS-H1 / P2 / DEFERRED | SHR Desk | GP-H2/H3 explicit hardware/reservation and DS-02..DS-04. | Actual audio-role controller/HDMI/LED/restart and response budgets. No inferred performance from SVG. |

## Validation and failure behavior

Focused `CARGO_INCREMENTAL=0 cargo +1.97.1 test --locked --test contracts -j 1`, then full `cargo +1.97.1 test --locked --all-targets -j 1` under the same environment for shared interaction/model changes. `cargo +1.97.1 fmt --check`; Clippy `--locked --all-targets -j 1 -- -D warnings`. No historical tests currently; optional `gallery artifacts/screens` only after visible layout changes, retain one small review set.

Test held key/focus loss, duplicate press/release, stale draft, pickup after selection, busy pending command, reject vs actual and lost ACK. Engine owns holds/ramps/protection. DS-01 may use existing Simulator but cannot mark DS-03/04 integrated. Replacement gate: same provider fixture hashes plus real GP-03 authority integration, simulator explicitly selected only for demo.

Historical research, auditions, exhaustive matrices, long soaks, full-show renders
and physical/combined-load checks are intentionally outside the normal software
milestones unless their protected behavior changes. Retain their owning documented
on-demand commands; no private media download or test hardware side effect.
Independent builds retain lockfiles and existing repository editions; this plan
does not upgrade dependencies/editions or replace existing intra-repository workspace
paths. The ban is on new sibling-repository path dependencies.

## Resources, review and recovery of work

Peer lane P4-A, rpi4 microSD/4 GiB class, isolated checkout `/home/shome/p/gigpies-module-planning-0006/shr-desk`. First task uses current dependency-free core; estimate <512 MiB compiler RSS and <256 MiB target growth, provisional ceilings not measured peaks. One shared build slot, jobs=1; no native GPU build in first wave. Pi5 original remains reference-only during peer ownership.

Independent fallback: DS-01 remaining action-table/recovery cases and documentation without compiling; DS-02 headless event-design only after DS-01 review. No unbounded render or research assignment.
Before builds check free space and target size; below 20 GiB free or above 5 GiB
output is a review, not permission to delete another task's cache. No reduced
coverage/debug information to make a budget appear to pass.

Handoff: exact changed files, commit plus patch hashes or bounded source manifest
if uncommitted, contract IDs/versions and provider-fixture hashes, commands/results,
intentional skipped classes, remaining limits and next task/owner. Stage only named
owned changes if a later implementation session commits; no public push is implied.
Receiving owner reviews independently and writes an immutable private-ledger
acknowledgement. Interrupted work stays visible with last completed acceptance
criterion; never reset/stash/clean another session or replay an uncertain mutation.


## Implementation launch prompt

Host/cwd assignments and source preparation are in GigPies PARALLEL_WORK_PLAN.md.
This is a prompt for a later user-started session; no implementation worker has
been started by the planning pass.

```text
Work only in the current shr-desk checkout. Read AGENTS.md (if present), the
owning docs and docs/GIGPIES_IMPLEMENTATION.md, then the referenced GP-2026-10-04.1 contracts.
Implement only DS-01; keep progress and evidence in this plan. Check hostname,
HEAD/source manifest, live Git state, active ownership and current contract hashes
before edits; preserve other sessions and unrelated work. If this is a delivered
snapshot, verify its handoff manifest and separate receiving acknowledgement first.
Reserve this host's one build slot as described in GigPies PARALLEL_WORK_PLAN.md;
use Rust 1.97.1, Cargo.lock, CARGO_INCREMENTAL=0 and cargo -j 1 in normal target/.
Run focused checks during work and the required normal suite for changed behavior.
No sibling writes, sibling path dependencies or unilateral contract changes.
No audio/MIDI/DMX/playback/device/display/service changes or shared load tests.
When blocked report exact provider/task/version mismatch and continue only the
independent fallback DS-01 remaining action-table/recovery cases and documentation without compiling; DS-02 headless event-design only after DS-01 review within this repository; do not fake acceptance.
Stop after the scoped task and reviewable handoff, before later milestones,
physical operations, publication or deployment. Do not stage unrelated files or
claim mock, planned or incomplete behavior is a finished engine.
```

## Progress

- 2026-10-04: source and owner documents inspected; plan written. Implementation
  tasks remain in the states above. Physical evidence retains its original limits.

### DS-01 first-wave handoff — 2026-10-04

Implemented for offline review in the accepted unborn P4-A checkout; acceptance
by the coordinator remains pending. `model::actions` owns the semantic table,
detached typed drafts and keyboard/injected-MIDI workflow. Exposed under `model`
via a path module to preserve the unassigned `src/lib.rs` export file.
Selection, three available pages, twelve-channel banks, scalar edits, explicit
hold/release/mode previews, confirm/cancel/back and pad menu use shared actions.
Release shows current/target/delta and explicitly simulated immediate application.
Drafts bind selection/page/epoch/revision/context generation. Context loss discards
drafts and pickup; focus/reconnect requires pad release, held keyboard keys require
key-up, repeats and text-focused shortcuts cannot execute edits. Confirmed state
still changes only after a correlated Simulator response/snapshot.

Normal validation: nine focused action regressions, thirteen existing contracts,
five CLI regressions (27 total), formatting, warning-denied Clippy and locked
release build. All Cargo build/test/Clippy commands use Rust 1.97.1, jobs=1,
CARGO_INCREMENTAL=0 and the host-local nonblocking build lock. Evidence, exact
baseline/changed/fixture hashes and logs are in private P4-A `handoff.md`.
No contract, dependency, lockfile, renderer or sibling source was changed.

Limits: this is a pure offline line-shell interaction model, no native window,
real key capture, device binding, role registry, engine adapter or engine ramp.
The existing SVG pad footer remains the base-page draft; live context-specific
pad guidance is exposed by simulator PREVIEW text and the semantic action table.
Only fixture pad bank/profile is decoded. Unavailable processing/provider pages
remain unavailable. Historical/exhaustive/media rendering, long benchmarks,
hardware/load, audio/MIDI/DMX/playback/network/device/service classes were skipped.
DS-02 and later tasks remain blocked on review/provider gates; no integration or
hardware acceptance is claimed. Next owner: coordinator reviews DS-01; P4-B owns
the next rpi4 build turn after the explicit release marker.

### Task 0008 DS01 correction and DS02 headless pass — 2026-10-04

Task0008 sustained software authorization supersedes the historical DS01-only
launch stopping rule above. Coordinator accepted corrective checkpoint R2 before
DS02 implementation. Protected drafts refuse unrelated protected/menu/mode-choice
actions; Enter preserves an unchosen mode picker. Shorthand key presses synthesize
keyup even after refusal. Keyboard brackets adjust pan; actual keyboard/MIDI pan
parity is tested. Direct Desk disconnect/reconnect fences every pad until a fresh
release and resets pickup through generation synchronization.

Implemented DS02 **headless portion**: `console::Console` pumps bounded generation-
tagged keyboard, typed text and complete synthetic MIDI events into shared actions.
Overflow drops queued events, cancels drafts and fences all supported keyboard/pad
presses until release; absolute controls rearm pickup. Explicit synthetic audio-role
assignment/revocation increments generation and refuses old envelopes. No event
applies a returned command; correlated provider responses remain Desk's boundary.
A separate single-slot LED mailbox coalesces confirmed/pending desired feedback;
it has no lighting queue, output worker or physical controller binding. The renderer
trait and SVG-backed headless implementation support resize, zero-size suspension,
focus loss and device loss/restoration without losing confirmed engine state.
Root `lib.rs` exports actions/console normally; `model::actions` remains a compatibility
re-export for existing callers.

Validation: focused actions/CLI/console tests followed by complete normal all-targets
suite, formatting, warning-denied Clippy and locked release build; final results and
exact hashes in private task0008 P4-A handoff. All build-producing checks held the
host build lock, jobs=1 and CARGO_INCREMENTAL=0 on Rust1.97.1. Tests are synthetic and
headless. Native DS02 winit/wgpu/device/HDMI and performance acceptance remain pending.
No native dependencies, hardware, audio/MIDI/DMX output, TCP, host service/font changes,
provider copies or DS03 decoder were introduced. DS03/DS04 wait exact reviewed provider
corpus and their owning integration gates. Coordinator reviews this DS02 source next;
P4-B receives the next build turn after this pass's release marker.

### Task0008 pass2 corrective checkpoint

DS02 source correction: lifecycle fences retain queued resize/focus/device recovery
while invalidating actionable key/MIDI/text intents. Text-focus transitions use
shared context loss to invalidate drafts and rearm pickup; new editor text remains
available. `assign(bool)` is only a synthetic headless role switch, with no C-ROLE
registry/native acceptance or physical bindings; generation exhaustion revokes
assignment permanently. Regressions cover same-batch recovery, stale release,
pickup reset, new text editing and generation exhaustion. Validation: nine focused corrective tests and final50 normal tests passed; root
accepted corrective R1. DS03 subsequently received exact GP02 acceptance; no native/hardware acceptance claimed.

### Task0008 pass2 DS03 read-only implementation

Root accepted DS02 corrective R1; nine focused lib/console tests passed. Root
delivered accepted exact GP02 at13:47:37Z; every accepted byte hash verified and
retained under tests/fixtures/gp02/v1 with provenance. [Provider client](PROVIDER_CLIENT.md)
owns the implemented standalone snapshot-body seam, strict decoder, complete
bounded page collector, stable-ID selection, read-only CLI and nullable SVG/text
presentation. Simulator remains explicit and separate. Snapshot receipt freshness
is separate from unavailable GP02 audio observations; no actual/frame fabricated.
Pinned serde1.0.229 and serde_json1.0.151, lockfile retained. Root accepted DS03
snapshot-only source; final exact manifest remains reviewable. Offline validation:
50 normal tests (lib1/actions12/CLI6/console8/contracts13/provider10), fmt,
warning-denied Clippy and release pass. Exact release CLI decoded final GP02
snapshot and exported nullable provider SVG. Host build lock/jobs1/incremental0;
target grew58->263MiB,36GiB free. Initial Clippy style issues repaired; full suite
rerun after adding the bounds/proposal regression. Final source/provenance/test
logs and intentionally skipped hardware/native/long classes retained privately. DS04 waits GP03;
no request/reply writes, DSP copies, devices, sockets or native GPU compile.

### Task0008 pass3 DS04 software client — source checkpoint in progress

Accepted pass2 68file manifest and GP03 DATA/INTERFACE hashes verified on rpi4.
DS04 now has a separate strict GP03 rendered snapshot/reply decoder, typed linear
nanogain current/target/hold values and no fabricated dB or meters. The session
requires fresh complete compatible state, engine-issued scope lease, monotonic
first-send deadlines, one immutable pending request, exact-ticket correlation and
same-ID retries. Epoch reset discards old comparison state; reconnect drops local
intent and requires new writer/fresh grant/input release. Monitor scopes validate
independently. Preview DTO/destinations are engine-provided and strictly validated;
release/mode confirmation pins reviewed context and preserves engine-held mix.

An explicit private Unix batch operator path handles interleaved snapshots, pending
and final replies, bounded framing/connect/write/read/script deadlines, UID and
0700parent/0600socket checks. Default simulator and standalone GP02 reader remain.
No DSP/provider algorithm copy or sibling path dependency. Source validation is
ongoing; real accepted service execution and coordinator final acceptance remain
pending. No hardware/native-GPU/role/controller binding or safety acceptance.

R2 offline validation:79normal tests, fmt, warning-denied Clippy and release pass.
New provider-produced preview/renew amendment verified by its accepted provenance
SHA256; prepared replay test awaits next build turn. No real service yet.


### Task0008 pass3 final DS04 outcome

DS04 local headless software portion is implemented and offline-validated.
Root accepted R3 all89source hashes after transactional typed-reply validation,
actual renewal scope/preview corpus, exact applied ticket correlation and known
terminal refusal correction.82 normal tests, fmt, Clippy-Dwarnings and release
pass. Accepted actual GP03/GP06 executable ad8a4a74... ran explicit temporary Unix
integration: fader/pan/monitor/mute, ASSIST proposal, engine-bound preview/release,
lost pending+commit ACK with sameID2 retry and one revision increment, preserved
mix on fresh readonly reconnect, badshow/epoch/version/lease refusal. Own child
stopped/joined and temporary owned endpoints/identity files removed. No hardware,
PA/signal-meter/scheduler/native-role/GPU acceptance. Batch script input supplies
explicit absolute values; physical MIDI pickup/gesture binding stays deferred.
Remote production GP06 auth remains outside this local sameUID boundary.

Reproduction and exact evidence paths are in private pass3/P4-A handoff.md;
provider binaries stay ignored outside Git. Source remains unborn/uncommitted;
all previous implementation/planning changes preserved. Next owner root verifies
the final source manifest/evidence and coordinates source handback, without user
intervention between software milestones.


### Task0009 continuation

The native/provider software continuation supersedes historical first-task stop
cards and local-only publication statements. Workers own source changes and
headless checks; root reviews contracts/artifacts and performs source publication.
The real frontend reuses DS04's accepted provider session/transport, the existing
semantic keyboard actions, scene primitives and bundled font. Current work and
limits: [native frontend](NATIVE_FRONTEND.md) and [status](STATUS.md). GP09 role
binding is consumed only after exact root acceptance; no competing role registry.


### Task0009 pass2 correction and DS05 read-only source — historical checkpoint

The accepted GP05 read-only contract/data now enables DS05 metadata consumption.
DS02 actual GP03/GP09 software controls/review/recovery and CPU offscreen rendering
passed focused checks after correcting snapshot catchup/expiry/reconnect races.
DS05 adds a strict typed reader and independent bounded health worker, truthful
module summaries and explicit `--modules-status` CLI. No recorder/FX/PA writes or
provider algorithms are copied. Current source and validation limits are in
[status](STATUS.md). At that checkpoint, actual GP05 executable integration and
final matching native checks/releases required the next bounded continuation. Source remains
uncommitted; root owns review, handback and publication.


### Task0009 pass3 final software outcome

DS-01..04 software and the read-only DS-05 increment are implemented and
offline-validated. Root independently accepted the runtime source and the actual
GP05 test delta, and visually accepted the real Health frame. The observer receives
accepted owner-library metadata independently while GP03 grant, presented mute
review and confirmed revision change complete. Broader actual GP03/GP09 controls,
engine review, role loss and reclaim/no-replay checks pass.

Final default97/native99 normal tests and17 focused native/actual-provider tests
pass, including four exact1920x1080 CPU lavapipe frames. Both feature sets pass
warning-denied Clippy; formatting and default/native locked release builds pass.
All Cargo commands used the parent-held nonblocking helper, Rust1.97.1, jobs1,
no incremental compilation and normal profiles/target. Private exact manifests,
commands, counts and separate executable hashes are retained in the handoff.
Root owns source handback, final joint demonstration, commits and publication.
No hardware/controller/window/LED verification, physical protection or writable
REC/PA/FX control completion is claimed. Existing source and old evidence remain.

### Task0012 GP07 consumer extension

The processing increment extends the existing real provider frontend and shared
GP03 authority session; it does not add DSP or another simulator. Accepted producer
bytes and provenance live in `tests/fixtures/gp07/v1`; the explicit probe and full
Channel workflow are documented in [Native frontend](NATIVE_FRONTEND.md).
Desk owns operator actions/readback/presentation, while the GigPies host owns
actual output samples, raw REC/analysis preservation and downstream FX/PA order.
The bounded compiled external driver supplies the real keyboard and injected
controller actions to that host. Current validation/limits are in [Status](STATUS.md).

### Task0013 four-band consumer increment

The task0012 path now consumes explicit GP07-processing:2: four stable bell bands
with independent frequency/gain/Q/bypass and unchanged compressor controls.
Version 1 fixtures remain intact; only new version 2 bytes are accepted. The same
frontend authority/review path supplies the 17-edit integration driver, including
actual final effective-frame evidence and individual field actions. F8 provides
an explicit GP03-only recovery for old providers without silent downgrade/replay.
See [Native frontend](NATIVE_FRONTEND.md) for controls and [Status](STATUS.md) for
validation; the coordinator independently owns signal acceptance and publication.

### Task0015 Brain operator consumer

Desk owns the explicit GP15 controls, shared authority/session integration,
keyboard and injected controller lifecycle, strict readback and complete reviews.
GigPies owns duplex transport, endpoint configuration/application, bridge telemetry,
DSP, source-frame expiry and all sample assertions. Desk has no PCM or sibling
path dependency. Root provides the final producer corpus and independently owns
acceptance and publication; a worker compile or synthetic adversary is not that
gate. See Native frontend for the current interaction path and private task0015
handoff for exact tested revisions and outstanding gates.
