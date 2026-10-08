# Status and next steps

## Task0018 D consumer implementation — validation in progress

Dynamic GP07 now explicitly selects4; legacy2 and historical v3 bytes remain
separate. GP18 sends has a capability-driven overview/channel editor, separate
level/tap drafts and complete reviews, explicit scope reattachment/new grants,
paired source-progress freshness and reconnect/no-replay fences. The optional
GP18 master EQ consumer provides narrow EQ-only PA reviews without rearm; the
existing muted full-configuration editor remains separate. Master EQ now displays
8/31-band settings and owner-golden-checked static response curves. Current
fixture-driven previews use actual frontend scenes and explicit offline labels.

This checkpoint reports implementation, not final acceptance. Focused, full normal,
actual frontend/provider PCM, native CPU rendering and publication gates are
recorded with exact final revisions and failed trials in the private D handoff.
Coordinator owns architectural acceptance, independent sample/diff review,
integration/synchronization and requested TV visual review. No physical devices,
listening, clock-lock or whole-host hard-real-time qualification is claimed.


## Historical Package A master EQ setup checkpoint — 2026-10-07

Implemented and owner-reviewed a focused native/frontend editor over the existing
PA graph: eight parametric and 31 graphic bands, linked or independent main L/R
edits, human-readable fields and unique program-bus selection. Admission refuses
duplicate/unknown EQ JSON members, invalid inactive values and mismatched producer
rate/block/port dimensions. Stereo edits validate atomically, and submission proves
that routing, monitor inputs, crossover/protection and all non-EQ settings equal
the original readback. Opening requires observed output quiescence; Apply also
requires a live PA grant and fresh matching show/epoch/revision/generation plus
unchanged configuration and bus map. Existing complete review and separate rearm
remain authoritative. This is muted setup, not hot EQ adjustment. See
[Master EQ](MASTER_EQ.md) for exact owner revisions, admission and limitations.

Package A offline validation: **256 default / 258 native tests passed**, with
12 opt-ins skipped per suite, including the explicit CPU-lavapipe native check.
Both warning-denied Clippy configurations, formatting, 9 Python publication tests,
local documentation links and whitespace checks pass. Both locked release builds
and final complete-index/history guards are recorded in the worker handoff.
The added regressions cover owner-shaped EQ admission, preservation, atomicity,
authority/context loss, complete review presentation and no reconnect replay/rearm.
The final source is isolated on `task/0018-master-eq`; acceptance/integration and
source publication remain coordinator-owned. Actual-provider EQ transactions were still a later gate at this Package A
checkpoint; the current D result is stated above. Physical mappings, listening
and clock-lock evidence remain separate gates.
Historical/exhaustive/long, actual-provider opt-ins and hardware trials were excluded;
no physical output was activated.

The initial simulator gallery is historical and does not model channel DSP.
Current channel four-band EQ/compression exists in production. Task0018 D adds
independent selected GP18 taps, sends overview/editors and optional live master EQ;
its current validation boundary is stated at the top of this document.

Checkpoint: 2026-10-05. Task0009/0011 source was published and synchronized.
Task0012 processing source was published and synchronized. Task0013 four-band
processing software acceptance is recorded below; source publication is coordinator-owned.
No hardware verification or binary release.

## Freshness and detached-draft review fixes — 2026-10-06

Frontend publication retains raw, Brain, device and structural observation age,
so a stalled worker cannot restart the 250 ms presentation freshness window.
Focus/context loss retains detached processing, device and routing content and
unfinished field text while revoking queued/held actions and reviews. Applying
retained work requires fresh validation and a new review; replaced device identity
requires explicitly reopening its editor. Reconnect never submits retained content.
See [native frontend](NATIVE_FRONTEND.md) for recovery controls and limits.


Local offline validation: **243 default / 245 native tests passed**,
with 12/12 opt-ins intentionally skipped. Both warning-denied Clippy
configurations, formatting and 9 Python tests passed. Reproduce with
`CARGO_INCREMENTAL=0 cargo test --locked -j1 --all-targets`, then the same command
with `--features native`, under the shared build lock. Historical, physical,
external-artifact and explicit rendering campaigns were not run. No publication
or physical/native-window activation was performed.

## Historical initial simulator baseline

The following baseline and early scope retain their original evidence. Current
software includes authenticated remote control, dynamic processing and reviewed
PA/output configuration; see the Task0014 and Task0015 checkpoints below and
[native frontend](NATIVE_FRONTEND.md).

- Independently buildable Rust 1.97.1 / edition 2024 package and Cargo.lock.
- Blueprint, module ownership map, reference-console screen study, complete
  planned page map, MiniLab mapping and proposed live control boundary.
- 36 synthetic channel identities, twelve-strip banking, selection-only keys,
  confirmed versus pending/rejected command state and bounded command/cache state.
- Auto/Assist/Manual simulation, persistent manual fader holds, explicit release,
  stale/conflicting/repeated command refusal and fresh-snapshot reconnect.
- Pure MIDI complete-message decoder, key/pad channel qualification, pad press
  edges, ignored releases/pressure, profile ambiguity checks, absolute pickup,
  three explicit relative decoders and bounded mkII LED packet construction.
- A line-oriented simulator with injectable MIDI and automix proposals. Mix
  absolute controls K01–K14 work; other pages refuse rotary edits. P6/P7 preview
  their intent, then require the explicit CLI release/mode command.
- Three state-driven 1920×1080 SVG screen drafts (Mix, Channel, Analysis), actual
  Terminus 12×24 glyph paths, two rows of eight control legends, and a local HTML
  review gallery. PNGs retained only as local review evidence.

The graph data and meter values are illustrative fixtures. No real FFT, audio
measurement, live meter, mixer output or renderer timing is claimed for these
historical simulator drafts. They do not represent current connected channel EQ,
compression, sends or PA/master EQ capabilities. Recorder controls remain separate.

## Historical Task0008/0009 software scope and remaining work

Task0008 accepted the GP02 reader, GP03 codec/session and actual local provider
batch integration (82 normal tests). Task0009 adds a real-provider frontend and
optional native winit/wgpu entrypoint; current validation is recorded below.
The standalone simulator retains its deliberately synthetic authority and is
separate from the provider frontend. Native role consumption uses the exact accepted GP09 artifact; actual consumer
validation has passed. Read-only real module health is implemented through accepted GP05 artifacts.

Physical controller/LED/HDMI acceptance, GPU timing/readability, real analysis,
writable recorder/PA/FX workflows, remote production authentication, learned controller
profiles and wider scene/routing workflows remain separate gates. No software
fixture proves audio or protection acceptance.

## Historical initial validation

Initial production classes: **17 tests** (13 state/MIDI/recovery/layout contracts
and 4 end-to-end CLI regressions). Formatting, warning-denied Clippy and the locked
release build pass. All three full-HD pages were generated and visually inspected;
fixture glyph coverage and viewport bounds are in the normal suite. Reproducible
commands are in [development](DEVELOPMENT.md).

Intentionally skipped: live audio/MIDI/DMX, playback, network/load/disconnect trials,
native GPU/HDMI performance and physical controller tests. No historical/exhaustive
tests exist in this initial package. GigPies and existing sibling production suites
were not rerun: their code did not change; GigPies changes are documentation only.
Local documentation links/whitespace and the preserved draft identity were checked.
The retained build output is about 41 MiB and the three-screen review set about
0.6 MiB. The temporary reference downloads and research environment (about 175 MiB
on disk) were removed after checking for active references. Existing large sibling
build directories were reviewed but left untouched; about 31 GiB remained free.
No user recordings, source archives, audio host settings, service or font settings
were changed. No code was installed as a live application.

## Repository evidence

The shell hostname was **rpi5**, despite the task's Pi 4 description. Strict alias
`gigpies-pi4` confirmed Pi 4 was reachable and its GigPies tree clean; no peer worker,
shared load, device operation or remote write was launched. Runtime hardware roles
are still undecided. The existing GigPies docs had uncommitted work before this
task; it was preserved, extended in place and not staged or committed.

| Owner inspected | Local HEAD at review |
|---|---|
| GigPies | `eea5267` plus pre-existing architecture/console documentation changes |
| SHR PA | `7683f9b` |
| SHR FX | `cd943c6` |
| SHR REC | `c0416d2` |
| SHR DAW | `d93447d` |
| SHR LUX | `ba4ccd9` |
| SHR Synth | `818c949` |
| SHR Sampler | `3aaabd5` |
| SHR Drums | `f4a9a6a` |
| SHR Tone Over 9000 | `3fec963` |

Those existing siblings remained clean and read-only. The archived Brain plan in
this repository matches the pre-edit GigPies document; historical links inside
the archive retain their original GigPies docs context.

## Historical initial next steps

These initial launch steps predate task0008/task0009. Current software progress
and remaining physical gates are recorded above and in the task0009 entries.

1. D1: implement one native window/GPU backend over the existing scene primitives,
   with the same bundled bitmap font. Add proper graph axes, resizing, larger text,
   retained selection and device-loss recovery. Measure rendering on the chosen
   display; do not equate SVG export time with GPU latency.
2. D2: replace CLI confirmation guidance with a complete action/menu/focus model;
   implement all keyboard/controller paths and per-gesture pickup/context rules.
3. Agree GigPies's real capability/control schema, beginning with read-only state.
   Keep the existing GPC1 test scalar and stereo hardware evidence clearly separate.
4. Schedule an explicitly scoped MiniLab/HDMI session after the input worker exists.
   Verify model/firmware/memory/messages, two banks, clicks, relative modes and LED
   ownership; no preset writes or simultaneous SHR LUX pad output.

See [the blueprint](BLUEPRINT.md) for D3–D6 integration gates and [screens](SCREENS.md)
for required versus useful/deferred capabilities.

## GigPies integration planning — 2026-10-04

[Owning GigPies plan](GIGPIES_IMPLEMENTATION.md) records scoped tasks, contract dependencies,
validation and launch instructions. This is planned work; existing implementation
and hardware status above are unchanged.

## Task0008 read-only software client

DS02 headless corrections are source-accepted and focused offline-tested. DS03
now implements a separate strict GP02 snapshot-body client and explicit file CLI
with trusted nullable text/SVG presentation; [details](PROVIDER_CLIENT.md). Full
normal validation (50 tests), fmt, warning-denied Clippy and release pass; root
accepted snapshot-only source, final exact manifest review pending. This is accepted provider metadata,
not rendered audio: actual values/frames unavailable and release disabled. GP03
DSP and DS04 mutable authority integration, native GPU and hardware remain deferred.

## Task0008 pass3 DS04 offline integration

Implemented separate strict GP03 codec/session and explicit local Unix batch
operator path. Root and independent reviewer accepted all89 R3 source hashes.
Final normal82tests, fmt, warning-denied Clippy and locked release pass. Actual
accepted provider executable SHA256
`ad8a4a74ca426ee3d3594929b00fca5daa2603d5c7ddfb7aef542d5782fa9b0a`
ran under owned temporary0700 directories/0600 sockets; all own children joined
and disposable endpoints removed.

Actual offline integration exercised snapshot/grant, fader/pan/shared mute,
independent monitor send, ASSIST proposal, engine preview/commit, fresh rendered
snapshots and writer release. Dropped pending+final ACK recovered via two identical
set envelopes/requestID2 with one revision increment. Actual completion retained
the same48frame-aligned effective frame/ticket and240frame ramp. New read-only
connections observed preserved mix without a grant or intent replay. Wrong show,
epoch, raw version and lease refused, preserving mix. This is coefficient/authority
evidence; no signal meter, physical audio/PA protection or scheduler acceptance.

GP02 standalone snapshot reader and default simulator remain separate. Native
GPU, physical input/LED/HDMI and remote production authentication are deferred.
Real CLI is a bounded batch operator path, not a native controller binding.
Final manifest/handoff are private task0008 pass3/P4-A evidence; no source commit,
publication, sibling dependency or provider algorithm copy.

## Task0009 pass1 native frontend — historical implementation checkpoint

Implemented source: optional native winit0.30.12/wgpu0.20.1/pollster0.3.0 backend,
CPU bitmap raster using bundled Terminus and existing scene primitives, explicit
real-provider headless/offscreen paths, asynchronous accepted GP03 client and
bounded separate input/provider/desired-LED queues. No Simulator supplies native
state. Selection/pages use the shared semantic action table; controls request
real grants/set operations, protected changes require displayed reviewed
confirmation, and release displays engine destinations/240-frame ramp. Stale,
unavailable and uncertain states disable edits. Explicit reconnect uses a fresh
writer, drops local intent and attaches read-only. Focus/overflow/device loss
fence queued input; resize/zero-size suspend presentation without stopping provider.

Native feature compilation and the pre-role91-test suite passed. Final native
normal-suite validation executed CPU Vulkan shader/upload/readback with exact
1920x1080 RGBA match; the actual provider workflow reached grants, fader, reviewed
mute/mode, explicit producer proposal and engine release preview, but its final
confirmation/reconnect test remains under correction. GP09 consumer and full final
suite/Clippy/release validation remain pending; no window or hardware verification
is claimed by this implementation checkpoint. GP09 source/data and executable have been root-accepted and hash-verified;
consumer validation is in progress. DS05 real module health awaits its providers. Root owns publication guards and final source pushes.

## Task0009 pass2 native and read-only health — historical checkpoint

Implemented DS02 corrections: generation synchronization after a blocked receive
preserves explicit reconnect; bounded raw telemetry coalescing avoids duplicate
queries and artificial per-frame delay; saturated64-frame/250ms batches refuse
freshness. Old rejected observations do not renew receipt. Engine-preview expiry
invalidates release review, and a valid new review is published before unrelated
renewal work. Protected review retains exact destinations/context/ramp while
keeping the internal preview token out of the operator view.

Offline focused validation before DS05:23 tests passed, including actual accepted
GP03/GP09 controls, complete review/commit, lost child deadline and new-generation
reclaim without input replay, broker stalled-reply fencing, and CPU lavapipe exact
1920x1080 RGBA checks for real-provider review and confirmed scenes. Root inspected
and accepted those real-provider layouts. This is software-only evidence; no
physical endpoint, actual operator window or hardware verification occurred.

DS05 read-only source now consumes the exact accepted GP05-modules:1 data. It uses
an independent4Hz worker/connection/latest slot with one250ms end-to-end query
budget, so module I/O cannot consume GP03 control replies or run in input/render
callbacks. Main/Health distinguish the raw mixer from the optional postmixer graph,
show actual recorder state/outcome and accepted/written frames with durable UNKNOWN,
fixed wet-only FX/status and main-only logical PA sample limiting with physical
protection unverified. Explicit `--modules-status` requests no writer/grant or edit.
Four focused default tests pass: strict fields/identity/counters/float locations,
native u64 precision, actual private read-only CLI framing and malformed refusal,
and stalled/drip/backlog deadline plus joined-worker bounds.

The final default normal suite passes97 tests (3 explicit actual-provider tests
ignored). Default warning-denied Clippy passes. Matching native normal validation, native
Clippy and new release builds remain to be completed after these source changes. Actual GP05 process integration
waits for its independently accepted executable/activation manifest; an explicit
ignored regression is prepared. Native release compilation is not a software
completion claim. Root owns final source review, handback, commits and publication.

## Task0009 pass3 actual module-health validation

Verified all117 frozen pass2 source hashes before continuing. Root independently
accepted the final DS05 deadline/domain/footer corrections. The accepted GP05
release, activation manifest and all REC/FX/PA library/header/manifest hashes were
verified before explicit synthetic private-process checks. Five focused native
module tests pass, including the real frontend health worker alongside a GP03
grant, presented protected mute review and one confirmed revision change.
The actual1920x1080 Health scene passes exact CPU lavapipe RGBA readback and
was visually inspected with truthful unavailable controls and physical UNVERIFIED.
No actual operator window or physical endpoint was opened.

Final software validation passes: **97 default normal tests**, **99 native normal
tests** (each leaves three explicit provider opt-ins ignored), and **17 focused
native tests with actual GP03/GP09/GP05 opt-ins enabled**. The latter includes
role-loss/reclaim without replay, actual controls and engine-bound release review,
module-health coexistence and four exact1920x1080 CPU Vulkan frames. Formatting,
Clippy with warnings denied for both feature sets, and both locked release builds
pass. Private default/native executables are retained separately with exact hashes.
Final metadata-only documentation updates do not change the tested runtime source.
Root owns final source handback, joint integration and publication; no worker
source commit/push or binary distribution occurred. Physical devices/windows,
HDMI/MIDI/LED behavior, audio/acoustic safety, remote production control and
shared-load/timing acceptance remain unverified. Historical/media/exhaustive/long
and hardware classes were intentionally skipped.

## Task0012 GP07 processing — software acceptance, 2026-10-05

Implemented on the real frontend: strict GP07 processing readback and complete
Channel EQ/compressor editing, explicit Apply/displayed review/confirm/Cancel,
independent bypasses, settled/target/draft/readiness/GR presentation, and shared
keyboard/injected-controller actions. GP07 shares GP03's connection, writer, lease,
request IDs and pending correlation. Explicit probing preserves legacy GP03 defaults.
Selection, revision, role and reconnect fences discard local intent without replay.

All eight accepted producer fixtures were hash-verified before consumption;
provenance is in `tests/fixtures/gp07/v1/ACCEPTED.json`. Final normal validation
passes **111 default tests** on Pi4 and **113 native-feature tests** on Pi5 (five
explicit integrations ignored in each suite). Formatting and warnings-denied
Clippy pass for both feature sets. Paired-refresh regressions cover an aged
observation refresh and refusal to rebase a queued review or confirm a changed
revision. Both default and native release builds and all nine publication-guard
tests pass. The actual legacy GP03 frontend/GP09 role integration also passes
against the published provider, preserving opt-in compatibility.

The actual release-provider workflow passed in 3.34 seconds, including positive GR,
two channel edits through keyboard/injected semantic actions, complete displayed
reviews and reconnect without replay. The paired-refresh native workflow also
passed in 3.21 seconds with exact CPU lavapipe readback of the actual Channel scene.
The actual 1920×1080 layout was independently inspected and accepted. The final
native workflow passed in 3.32 seconds against the hash-verified published GigPies
provider at `874cff2dede9ae19bc0b19dc4431d412e490e6b4`, including CPU lavapipe
readback. Final driver/source identities are retained in the private handoff.

The coordinator and independent Astra-low reviewer accepted the joint real
operator/provider/sample chain on 2026-10-05. Driver SHA256
`3b597e9eaa7906558ce8b2a8b9fbe650eebf11a16dd67c78b6d47f47d073f24d`
ran against the actual host: 1,693 FOH blocks matched eight isolated slot0 references
exactly; channel boundaries were 5088/19632; monitors remained bit-exact; actual FX
wet+dry followed by PA matched; raw analysis retained 3/55/111 windows; eight PCM24
stems of 80,976 frames covered the complete driver interval without gaps, drops or
faults. Revision 2/settings persisted after reconnect and lease expiry. The final
rebuilt driver and published provider passed the coordinator's exact-artifact rerun
in 8.64 seconds, including positive GR and stable post-exit revision 2. These are
synthetic software signal/authority observations, not physical or listening acceptance.

Runtime fixes preserve raw GP03 telemetry coalescing while awaiting GP07 replies;
obsolete observations had otherwise consumed the 250 ms readback budget. A missed
read-only observation can recover through later polls without replaying a mutation.
Review and confirmation obtain fresh paired observations, retaining the original
revision/generation through every refresh and renewal. Strict decoder and bounded
driver review findings were repaired; exact producer fixtures remain unchanged.
Final validation and independent review are complete. Source publication is
coordinator-owned; no public binary release, window, physical endpoint or host
service change accompanies this software acceptance.

## Task0013 four independent parametric bands — software acceptance

The consumer now requires GP07-processing:2 with all 24 atomic fields, four
independent bell frequency/gain/Q/bypass controls, global EQ bypass and unchanged
compressor editing/readback/GR. Stable band identities may cross in frequency.
Channel displays settled, target and local draft separately; E, field navigation,
numeric entry, Apply, complete displayed review, Confirm and Cancel use the
existing semantic action path. Original paired freshness, context/revision pinning,
writer/lease/request history and no-replay recovery remain in place.

The v2 producer corpus is hash-pinned; v1 bytes remain unchanged and are refused
by the v2 consumer. F8 explicitly reconnects GP03-only after a legacy provider's
probe disconnect, without claiming the disconnect proves unsupported version.
The real driver records at least 17 complete reviewed transactions and actual
final effective frames. Final normal suites pass **116 default / 118 native** tests,
including strict v1 refusal, all-band domains, original freshness/revision fences,
complete review geometry and controller-queue recovery.

Actual supplied release-provider validation passes 17 reviewed edits, positive GR,
second-channel isolation, context cancellation and reconnect without replay. The
native run completes in 30.3 seconds. Actual Channel and complete review scenes
match CPU Vulkan readback at 1920x1080, 960x540, 540x960 and 3840x2160; zero size
suspends presentation. Both full-HD scenes were visually inspected. This exposed
and fixed a 2048-pixel requested resolution limit and scaled texel selection from
interpolated UVs. Direct framebuffer mapping now gives deterministic aspect-fit
readback. Unsent controller field gestures now match keyboard release behavior,
so they cannot fill the provider queue and delay paired observations.

The unchanged legacy release provider produces the observed disconnect on a v2
probe; explicit F8 GP03 reconnect then passes grant/review/confirm without replay.
Nine Python publication tests, both warnings-denied Clippy feature sets, formatting,
default/native locked release builds and the complete-index publication guard pass.
The final default real-provider driver completes all 17 edits in 14.4 seconds.
Normal target output is about 3.7 GiB with 26 GiB free; debug/cache evidence remains.

The coordinator independently accepted the same final driver in the actual joint
host: 17 edits, 88 individual field actions and 4,621 rendered blocks; eight raw
PCM24 stems each preserve 221,520 frames exactly. Independent direct-form-I EQ
references cover 143,664 samples with maximum error 1.943e-16; monitors remain
bit-exact, actual owner FX/PA comparisons pass, and reconnect causes no replay.
Independent source review and publication remain coordinator-owned. These are software-only
checks; no physical display, controller, audio, listening or combined-load claim.

## Task0014 dynamic integration — implementation, acceptance pending

The existing real provider/session/frontend now has explicit successor version
selection, dynamic input/coefficient/processing inventory, exact additional monitor
scope serialization, stable-ID selection and complete partial-bank navigation.
F7 presents paginated physical patch and common-clock/resource metadata; unknown
physical lock/mapping stays unknown. F9 edits detached PA owner JSON (including enums, route objects/arrays, null and
program bus maps), full explicit document imports, or advertised output routes; Apply/review/confirm and separate mute/rearm
use the same authority session with PA/route-specific grants. The actual owner remains responsible for topology/DSP admission; every replacement
uses complete paged review and explicit confirmation.

The authenticated QUIC connection and bounded immutable-document assembler are
implemented in source. No peer grant, freshness or pending/retry authority is moved
out of the existing Session. Old accepted corpora remain unchanged. Actual supplied
16/17/32/48 final producer profiles and actual structural snapshots are retained
with SHA256 provenance in tests/fixtures/gp14/v1 and pass normal decoder tests.
The continuation corrects cached structural reply correlation and remote outer-
envelope reassembly, including the complete-document handoff to the shared Operator.
Regression coverage uses actual producer pages, preserves exact frame/whole-document
hashes, and checks duplicate replies without freshness renewal. Reviewed PA imports
refuse symlinks/FIFOs and preserve JSON case across batched native input events.
The refreshed final reference corpus is retained separately in `gp14/final`; it
preserves MAIN L/R versus LINE 3–10 labels and unassigned physical outputs.

Before the final paired structural-refresh correction, the complete suites pass
**135 default tests** and **137 native tests**, each with eight explicit integrations
ignored. Native validation selects the inspected CPU lavapipe ICD without display
variables. A first parallel run hit the unchanged 250 ms module-health deadline;
the focused rerun and complete serial runs pass without relaxing that assertion.

Three reserved provider runs exposed real integration failures: remote outer-page
handling, the structural `snapshot` reply state, and an unrequested raw observation
needed by structural freshness. They are retained failures, not acceptance. The
consumer now requests raw and structural observations under one absolute 250 ms
budget, requires both accepted observations and matching revision/topology, and
retains operation errors separately from later health-poll status. Successful finals
without readback require a fresh query; map changes require explicit reconnect.
Actual complete snapshot/pending/final/refusal corpora cover 16/32/48 profiles.

The latest correction still requires complete matching suite reruns, both Clippy
feature sets and both release builds. Actual weighted-PA/physical-route/high-channel
operator and independent sample acceptance also remain pending with the coordinator.
The current local continuation is a partial handoff, not task0014 completion.

| Requirement at the preceding partial checkpoint | Consumer location | Then-remaining acceptance |
|---|---|---|
| Dynamic strips/banking and stable identity | frontend/provider/audio | Actual high-channel driver (final profiles decoded) |
| Four-band16/17/32/33/48 edits | processing/audio/frontend | Actual operator/sample chain and native review |
| Additional monitor scopes | scopes/provider/audio | Actual independent grants/sends |
| Authenticated remote, same authority | remote/local_audio | Exact integrated mutual-TLS/revoke/reconnect runs |
| Editable patch and PA controls | topology/structure/frontend | Actual owner PA readback, mute/prepare/patch/rearm |
| Common-clock health/resources | topology/frontend | Final coherent producer timeline and recovery |
| Transport segmentation | pages | Normal adversaries plus real large producer replies |

The coordinator owns final producer schema/fixture freeze, independent acceptance
and publication. This checkpoint does not mark task0014 complete. Physical devices,
USB-slot/socket mapping, ADAT lock, hardware throughput, windows and physical GPU
acceptance remain separate and unverified. Current commands/results and exact local
handoff source identities are recorded by the bounded worker before handback.

### Task0014 third continuation: deadline diagnosis

The fourth reserved actual-provider run also failed; it is not acceptance. A finite
three-pass diagnostic using the exact captured 48-input producer pages measured
297–307 ms for unoptimized assembly/envelope/raw/structural decoding on Pi4, before
network overhead. The same optimized release pipeline measured 34–40 ms. The
250 ms freshness and paired-refresh deadline remain unchanged; this does not
qualify hardware throughput or network latency.

The consumer now checks the deadline before admitting each decoded observation and
again before successful completion. A failed pair invalidates its structural receipt;
a later raw-only poll cannot make that failed structural observation fresh. Last
explicit operation results survive health-update coalescing, and writer availability
uses an actually confirmed scoped lease with conservative expiry. Review-ready is
separate from applied. The current runtime passes **138 default** and **140 native**
normal tests, each with nine opt-ins ignored (eight actual integrations and the
finite decode diagnostic). Python publication tests pass nine cases. Native runs
use only the inspected CPU lavapipe backend with display variables unset.

Reserved run5 demonstrated 16 complete pairs at 58.8–60.4 ms with actual configured
raw documents around 131 KB, then exposed an acceptance-driver readback race after
mute. The driver now requires the exact correlated final and matching intended
readback at its confirmed revision. Rearm checks observed source-frame advancement
past the ramp; release/reconnect check absent write authority. No timeout changed.

Reserved run6 passed the actual 48-input structural operator path in 3.59 s:
explicit baseline rearm, quiescence, complete weighted PA/gain import, a changed
physical output source, fresh-map reconnect and explicit rearm. Confirmed revisions
advanced 1–5, with full readback and effective-frame evidence. The high-channel
operator path passed in 1.69 s, editing all 24 fields on inputs 16/17/32/33/48 through
keyboard and injected actions, checking other channels and read-only reconnect.
These are actual remote operator results, separate from independent sample proof.
Run6's second Brain negotiation was refused during a 500 ms restart gap. Source
and trace review showed the old media owner retained until the existing two-second
I/O timeout after finite process loss; run7 used a 2.5 s recovery gap without a
protocol or timeout change. Both fresh Brain episodes and actual dry/protected
output through loss and final stop then passed.

Final root-owned actual-provider acceptance used the same frozen Desk artifact:
reference **16 inputs / 18 analog outputs: 94 independent checks** (run8),
**32 inputs: 125 checks** (run9), and **48 inputs: 157 checks** (run7). Each includes
actual structural operations, both Brain episodes, exact raw REC/analysis channels,
patch/readback and loss continuity. The 16/18 reference begins unpatched and proves
unused/S/PDIF playback slots remain zero; the 32/48 companions additionally prove
the weighted PA gain/phase relation and duplicate physical patch samples. The
five-target high-channel evidence remains the successful run6 result. These are
finite synthetic software runs, not physical audio or throughput qualification.

Both warnings-denied Clippy feature sets, formatting, both application release
builds, Python publication checks and focused document-link checks pass. Build/source
manifests and all diagnostic failures remain private. Normal tests were serialized
on Pi4 without changing production deadlines. Explicit historical/audition,
exhaustive/long benchmarks, physical audio/MIDI/DMX, windows/GPU, socket/USB/ADAT
verification and hardware throughput classes were not run. The finite actual
operator/media runs above are separately accepted software evidence.

The coordinator owns final exact-source review, publication/CI and canonical
synchronization. No source push, binary release, deployment or physical activation
is performed by this worker. The earlier local handoff commits and normal cache
remain preserved.

## Task0015 Brain audio consumer — software validated, 2026-10-06

Desk implements actual authenticated Brain monitor, talkback and duplex-device
controls through the existing provider/frontend. It does not open PCM. Device
mapping, review, explicit arm and recovery retain exact device epoch/map/configuration
identity. Keyboard and injected-controller holds close on key-up, focus loss,
controller removal, expiry or disconnect; reconnect never resumes a hold.

The compact held proof, atomic same-lease maintenance and committed raw/Brain
readback passed independent review. Maintenance preserves identity, scope,
permission, expiry, cancellation and bounded replay checks, with conservative
first-send client expiry. All existing canonical scopes are supported; ordinary
mixer/PA scopes use maintenance only with explicit GP15 opt-in. Legacy clients
retain their existing behavior. The complete producer-generated 67-fixture corpus
matches byte-for-byte. Both views refresh only after the entire pair validates.

Current Desk validation: 240 default and 242 native normal tests, one explicit
CPU-headless native check, both warnings-denied Clippy configurations, default/native
release builds, release integration driver and nine Python tests passed. Normal
suites intentionally exclude 12 opt-ins each. Historical renders/research and
unrelated exhaustive tests were not repeated. Use the parent-held build lock,
Rust1.97.1, locked dependencies, one job and incremental compilation disabled.

Trials 31/32 passed on earlier source; failures 33–36 remain retained. Reviewed
corrections address stale device readback, revision-dependent passive maintenance,
premature recording-finalization reports and held-service proof/publication order.
The consumer reuses only an unused, still-valid original-send proof after current
context/gesture/floor checks, maintains before the final proof, and publishes
before idle waiting. The 30/50/150/20/250 ms bounds remain unchanged.

Trials 37–43 passed all seven scenarios on one frozen candidate: simultaneous
16/32/48 inputs, duplex stall/restart, controller stall and Stagebox restart.
Independent checks covered exact raw samples, talkback/operator audio and overlap,
closure without replay, continuing dry/PA/FX/REC, and fresh unarmed epochs.
The restarted Stagebox's interrupted take was preserved, not labeled finalized or
recovered; its new take passed exact-sample checks.

Provider-before-consumer publication, hosted CI and exact twelve-repository
receiving receipts are tracked by the coordinator's private ledger. Physical PCM,
socket mapping, clock lock, acoustic behavior and real display/controller
qualification remain separate. The owning [GigPies acceptance record](https://github.com/PaolaShultz/gigpies/blob/main/docs/BRAIN_AUDIO_ACCEPTANCE.md)
records integrated measurements and limits. Earlier detailed checkpoints are
preserved in the [dated draft](archive/brain-audio-before-final-2026-10-06/STATUS.md).

The development-PC screen-reference commit adds 12 inspected source figures,
per-screen notes, an offline gallery and exact publication allowlist entries. It
was merged with the task0015 source without changing Rust, dependencies or runtime
behavior. The companion Lightdesk collection contains 16 references. See the
[audio reference library](console-screen-reference/README.md); these are design
inputs, not newly implemented screens or physical-console acceptance.
