# Native provider frontend

Task0009 adds optional native presentation over the accepted GP03 client. This
surface consumes exact authority/linear coefficient data; it contains no mixer
or provider algorithm. Fader targets remain integer milli-dB, pan uses the accepted
integer scale, and actual coefficient lanes remain nanogain. No integer-dB meter
is inferred from coefficients. Analysis remains unavailable; optional accepted GP05 module health is read-only. Main pages show signed dB,
pan direction/percent, mute on/off and readable ownership/proposal/hold labels.
Provider counters and raw coefficient arrays are confined to the Health page;
rendered gain is shown as a named linear ratio without inventing measured dB.

## Dependencies and resource review

Optional pinned winit0.30.12 uses only X11/rwh_06; wgpu0.20.1 disables defaults and
uses WGSL; pollster0.3.0 drives initialization. See official
[winit docs](https://docs.rs/winit/0.30.12/winit/) and
[wgpu docs](https://docs.rs/wgpu/0.20.1/wgpu/). Existing serde pins remain unchanged.
Root accepted the dependency/resource checkpoint and initial lock resolution.
Initial Pi4 target353MiB, free36GiB and availableRAM2.9GiB; initial growth budget
4GiB, jobs1 and no incremental compilation. These are dated build planning
observations, not native performance evidence. Normal target/profiles are retained.

The renderer rasterizes existing rectangle/line/text primitives and the bundled
PSF glyphs into a fixed1920x1080 RGBA texture, presented by a fullscreen triangle.
This preserves glyph identity without host fonts. Resize letterboxes with preserved aspect and integer upscale;
small displays downscale, and zero size suspends. A GPU glyph atlas and text scaling/viewing-distance acceptance
remain performance/accessibility follow-up work. No60Hz or frame-time claim.

## Explicit entrypoints

```sh
# No display/event-loop/GPU initialization:
shr-desk --headless /absolute/private/audio.sock SHOW_UUID EPOCH WRITER foh view.ppm
# Requires an explicitly authorized display session; not run in software checks:
shr-desk --native /absolute/private/audio.sock SHOW_UUID EPOCH WRITER foh
# Process-only CPU Vulkan selection; no display or actual GPU adapter:
env -u DISPLAY -u WAYLAND_DISPLAY \
  VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.json \
  VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json \
  shr-desk --offscreen /absolute/private/audio.sock SHOW_UUID EPOCH WRITER foh view.ppm
```

`--native` and `--offscreen` require feature `native`; default builds retain the
simulator/gallery, GP02 file reader, GP03 batch client and CPU headless frontend.
The trusted transport requires an owned canonical0700 directory,0600 UDS and
same-UID peer. No endpoint is discovered. An unavailable provider is displayed
honestly; presentation does not fall back to fixtures.

## Actions and recovery

Shared semantic keyboard table: arrows/Tab select, F1/F2/F6 Mix/Channel/Health page, +/- fader,
[/] pan, M mute review, H hold review, R engine release preview, A mode picker,
1/2/3 choose Auto/Assist/Manual, Enter confirms displayed review, Esc cancels,
F10 help. G explicitly grants the configured scope; Q releases its writer lease.
Auto requires already accepted engine bounds; absent bounds refuse at the codec.
Monitor scopes edit independent monitor send targets. F5 reconnects using a fresh
writer identity, discarding every queued intent and attaching read-only.

Input queue64, provider request queue8, provider updates1 coalesced, LED desired
state1 coalesced are separate. Provider I/O runs on a joined worker; window input
never waits for framing/retry/engine reply. UI exit sends no recall or release;
engine holds persist. Slow LEDs cannot delay provider or input. LED data is a
software mailbox only; no physical controller endpoint is opened or bound.
Focus/overflow/device loss cancel reviews and fence queued/held input. A command
already sent may remain uncertain; it is never labelled applied by input. Mutations
pin the displayed revision and confirmations name an already displayed review ID.
Protected review uses a paged modal; all pages must be presented before Enter.
A release review disappears when its engine-issued preview expires; the two-second
provider deadline is preserved. Fresh telemetry is coalesced in a bounded batch
(up to64 frames/4MiB,250ms); quiet stale observations trigger a snapshot request,
while fresh polling avoids duplicate requests. Saturated batches refuse freshness until the bounded read proves the queue drained.
Within a drained batch, GP15 Brain/device, GP14 and GP07 contract replies are
dispatched in wire FIFO order before newest-first raw snapshot coalescing. This
preserves pending/final and priority-close correlation; a contract final cannot
invalidate raw freshness after the batch has admitted its newest raw observation.
Raw and contract revision equality remains required for coherent Brain freshness.
A valid final arriving before its raw readback keeps the same refresh operation
open: it requests read-only snapshots and waits within the original total250ms
and64-frame budget. Later drained batches retain FIFO contract ordering. The
deadline and frame budget never restart, and no mutation or hold is replayed.
Unsolicited raw-only stale/regressive batches retain the legacy no-query refusal;
continuation applies to contract readback, Brain pairing or initial quiet attachment.
Available-frame Unix reads inherit the same absolute deadline, including a partial
prefix/body; readability never starts a fresh per-frame timeout. QUIC available
reads limit only the first-byte probe to1ms; body and segmented snapshot assembly
retain the original caller deadline rather than starting a new200ms allowance.
The shared pages wrapper propagates that deadline through every segment and checks
it before admission; ordinary pending-command reads/retries also retain their
existing operation deadline. Ordinary mutation reads wake at the existing100/250/500ms
identical-request retry offsets, capped by the original1900ms operation budget and
current lease (or grant lifetime). A no-first-byte timeout services that schedule;
partial frames still fail closed. Schedule inspection never advances retry count,
changes first-send time or renews authority, and exhausted retries wait only until
the remaining authority/operation bound. Ephemeral hold/heartbeat/close requests
are never retransmitted by this scheduler. Before allocating a new unheld GP15-session
renewal request, an own matched Brain query anchors the current authority revision
and raw readback must pair exactly. Coherent cached state or a queued older probe
cannot authorize that new renewal. This read-only probe is bounded by the remaining
lease and original operation deadline; it never rebases an existing request. Raw
telemetry has no query ID, so same-revision raw pairing is not claimed as proof of
a particular raw query response. Generic sessions without Brain retain their
existing renewal observation behavior. Any terminal renewal refusal is reported
as a failure with the lease unchanged, including a revision race after the probe. Ordinary key-up or context cancellation
is classified separately from wire faults: it closes the hold, discards matched
authorization and retains generation-tagged outstanding probe identities. A later
explicit gesture requires a new current-generation probe; no hold is replayed.
The worker preserves a healthy connection on a typed guard cancellation and
performs its normal new-generation transition before further admission. Malformed,
unmatched, partial or failed I/O remains terminal even during cancellation; the
first provenance fault is retained instead of overwritten by later poll errors.
Brain snapshot queries carry bounded local FIFO probe IDs and timestamps taken
before sending. Every strictly validated snapshot reply consumes one probe, even
when its frame/revision equals the cached state; pending/final replies consume none.
A full refresh waits for its own probe, not a previously queued observation.
Fresh raw state is reused only at the same revision; absent, expired or mismatched
raw state triggers a read-only query within the same refresh deadline. If another
writer advances raw revision beyond the matched Brain reply, a new read-only Brain
probe replaces that superseded pair within the original deadline and total64-frame
budget. Neither mutation intent nor the time/frame budget is restarted.
Held press/heartbeat/active-renewal admission instead uses the separately correlated
compact proof described below, within30ms, retaining20ms for the send and the
existing50ms source-frame freshness limit. It uses that proof's exact frame and
revision and never substitutes a later final's cached frame or receipt timestamp.
Probe faults stop heartbeats; there is no resynchronization or mutation replay.
Context cancellation discards matched authority while retaining generation-tagged
outstanding identities to drain. A new explicit interaction requires its own proof
in the current generation. The provider's150ms observation-bound deadman remains
authoritative; delay can cause a safe refusal rather than extend either limit.
An observed Brain revision must match raw state before refresh reports a coherent
pair; timeout or cumulative saturation still closes the uncertain client path.
Older rejected observations never renew freshness. Buffered telemetry does not add an artificial delay ahead of
correlated command replies. A fresh reconnect request arriving during a worker
receive wait is checked against the new generation before it is processed.
Renderer loss does not restart the provider or recall a mix. WGPU lost/outdated
surfaces reconfigure; device-loss callback causes reconstruction; out-of-memory
exits the window while the engine remains independent.

GP09 descriptor/ownership binding consumes the exact accepted root artifact. Native
controller/MIDI endpoint opening and physical LED dispatch remain outside this
software-only session. No alternative role schema is defined here.

## Reproducible checks

Serialize build/test/link work using the central
[one-build-slot procedure](https://github.com/PaolaShultz/gigpies/blob/main/docs/PARALLEL_WORK_PLAN.md#one-build-slot-per-host).
For a standalone checkout, a parent-held nonblocking lock can wrap each command:

```sh
flock --exclusive --nonblock /home/shome/p/.gigpies-build.lock \
  env CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 \
  cargo +1.97.1 test --locked -j1 --all-targets --features native
```

Run matching locked parent invocations for `cargo +1.97.1 fmt --check`
(without --locked/-j1), `cargo +1.97.1 clippy --locked -j1 --all-targets
--features native -- -D warnings`, and `cargo +1.97.1 build --locked -j1
--release --features native`. Coordinated workers use their coordinator's grant
helper instead; exact private commands are retained only in private evidence.
Offscreen regression `tests/frontend.rs` executes WGPU only when the exact CPU
ICD is explicitly selected; otherwise it checks refusal before adapter discovery.
The opt-in `tests/frontend_real.rs` uses `SHR_DESK_GP03` for the exact accepted
provider binary and `--ignored --nocapture`; verify its accepted SHA256 first.
The current-provider run also checks the explicit GP05 `available=false` response
when no module graph is enabled; transport failure and an available graph are
different outcomes, covered separately.
It creates only owned synthetic private endpoints, and joins/stops its children.
Historical/exhaustive/long, physical endpoints/display/controller, playback,
services/host changes and shared-load classes are intentionally skipped.

## Accepted GP09 role client

The accepted `gigpies-role-lease:1` broker is a separate process; Desk consumes only
its exact accepted protocol/data in `tests/fixtures/gp09/v1`. Native mode starts
read-only unless a live injected lease is supplied explicitly:

```sh
shr-desk --native /absolute/private/audio.sock SHOW_UUID EPOCH WRITER foh \
  --role-provider /absolute/gigpies-role-lease /absolute/private/role-registry acquire.json
```

The same option can follow the output filename in headless/offscreen modes. The
acquire file is an exact `gigpies-role-acquire:1` request including caller-injected
inventory and expected registry generation. Broker validation/locking remains in
GigPies; Desk never enumerates, opens or guesses physical devices. Each surface
owns one private child/pipes. Verification is every500ms, replies deadline1000ms;
strict decoding requires exact live binding/generation, independently tracking
registry CAS. Timeout, EOF, mismatch or death fences provider sends/retries and
input/LED generation; keyboard-only read-only navigation remains. No automatic
reclaim occurs. A new explicit acquire request/profile generation and F5 fresh
provider reconnect are needed before explicit G grant; no queued input is replayed.
Ordinary child exit retains saved intent. No physical role acceptance is claimed.

Headless provider-only integration remains an explicitly labelled software check
of the existing DS04 client; it conveys no native or physical role authority.


## Read-only optional module health

The accepted `GP05-modules:1` metadata consumer uses a separate private Unix
connection and worker, with one latest update slot, at most4Hz queries and one250ms
end-to-end connect/send/receive deadline. It starts after the explicit GP03 frontend has attached; its
I/O never runs in input, rendering or mutation/preview work. Unsupported old
providers, missing optional modules and timeouts remain unavailable/stale.

```sh
shr-desk --modules-status /absolute/private/audio.sock SHOW_UUID EPOCH
```

That explicit headless command issues only `module_status`, with null writer,
lease, request ID and expected revision. It prints strictly decoded confirmed
metadata or fails without any recorder/FX/PA write. The normal main page gives a
compact optional-module summary; F6 Health shows recorder state/outcome, accepted
versus written frames and durable UNKNOWN, fixed wet-only FX delay and actual
status faults, and the PA logical2->6 mapping with active0/1 and silent2..5.
The owner's sample limiter applies only to the main logical outputs; physical
and acoustic protection remain unverified. The GP03 raw mixer and optional
postmixer FX/PA graph are labelled separately.

The decoder retains native u64 FX reset counts without floating-point conversion.
Only named FX parameter fields admit finite f64 values; other counters retain
canonical decimal strings or their exact native integer ABI type. Required nullable
fields, duplicate/unknown fields, fixed capabilities and32-byte NUL-padded FX
identity are checked against the accepted data in `tests/fixtures/gp05/v1`.
The explicit ignored GP05 regression requires `SHR_DESK_GP05` and
`SHR_DESK_GP05_MANIFEST`, pointing to independently accepted executable and
activation artifacts whose hashes must be verified before use. It exercises the
real frontend health worker alongside GP03 grant, presented mute review and
confirmed revision change. With the exact CPU ICD environment above, feature
`native` also checks the real Health scene against Vulkan readback.
This passed against the accepted owner-library graph; physical protection,
controller/display behavior and signal-meter acceptance remain unverified.

## GP07 four-band channel processing (task0013)

The real frontend supports the accepted `GP07-processing:2` extension with an
explicit `--processing` option on `--native`, `--headless` or `--offscreen`.
Programmatic callers use `Frontend::enable_processing()`. This opt-in protects
legacy providers that close a connection on an unknown contract: ordinary GP03
attachment sends no GP07 query. Unsupported or failed probes remain unavailable;
use **F8** for an explicit legacy GP03-only reconnect (or start without the option).
F8 discards all local intents and attaches with a fresh read-only writer. A transport
disconnect is shown as the observed failure; it is not proof of an unsupported version.
There is no silent downgrade or edit retry. GP03 retains version 1. A valid processing snapshot must
prove capability before any processing editor is available.

GP07 uses the **same Operator, Unix connection, Session, writer lease, request-ID
counter and pending transaction** as GP03. It never creates a processing writer
or a second mutable connection. Optional GP05 health retains its existing separate
read-only worker. Complete channel replacement requires a fresh FOH grant, fresh
matching raw/processing revisions, no fault and all processing transitions ready.
A successful final confirms boundary application; it does not claim the 240-frame
crossfade is finished. Backpressure has no admission, consumes no ID and requires
a new explicit Apply after fresh ready state. Cached or unrelated processing replies
cannot refresh observed state or complete another contract's request.
Read-only processing polls retain raw telemetry coalescing while waiting for the
processing reply, so old GP03 observations cannot starve control/readback deadlines.
A missed bounded observation becomes stale and later read-only polls may recover;
there is no mutation replay. An explicit unsupported response disables the probe.

On **F2 Channel**, **E** opens a detached complete draft from the confirmed target.
**U/I** select the previous/next parameter; **J/K** adjust one accepted step, and
**N/P** adjust 100 steps. Either adjustment toggles a selected bypass. For direct
entry, type a decimal number in the displayed units and press **Enter** to accept
that local field: Hz, dB, Q, ratio or milliseconds; bypass uses `0` enabled / `1`
bypassed. **Backspace** edits the entry. This Enter does not apply processing.
**F4 Apply** opens the complete displayed review, **Enter** confirms after that
review has been presented, and **Esc Cancel** discards the draft or review.
Keyboard and injected `Action::Processing*` controller gestures pass through the
same bounded frontend action path and the same authority session. No physical
controller is opened.

The page separately labels confirmed settled config, committed target and unsent
local draft. During a transition, output is a blend; neither endpoint is labelled
as the instantaneous output. GR is positive detector attenuation excluding makeup,
not an input/output meter. It is labelled bypassed, transitioning, stale or faulted
when unavailable. All four stable bell bands expose independent frequency, gain, Q and bypass.
Band order is identity order even when frequencies cross. Global EQ bypass and
unchanged compressor parameters, explicit makeup and compressor bypass remain
editable: 24 mandatory atomic fields. The Channel scene groups each band in one
row with separate confirmed settled, confirmed target and unsent draft columns;
the selected field is named below. Apply lists every field in the protected review. Existing fader/pan/mute/hold/mode controls
remain available outside a processing draft. Selection/page/bank/revision changes,
role loss, focus/device loss, overflow and reconnect revoke drafts and reviews.
Reconnect uses fresh read-only authority and never replays settings.

Producer fixtures are pinned in `tests/fixtures/gp07/v2/ACCEPTED.json`. Normal
`gp07_codec` tests exercise exact bytes, strict domains, shared authority,
nonadmission and late/mismatched replies. UI regressions protect numeric entry,
keyboard/controller parity, layout and context loss. These fixture checks alone
are not actual-provider or audio acceptance.

Three explicit ignored tests in `tests/gp07_frontend.rs` use the actual frontend:

- `gp07_actual_release_provider`: `SHR_DESK_GP07` names an independently verified
  release provider. It launches and joins only its own temporary private service.
- `gp07_external_driver`: `GP07_EXTERNAL_ENDPOINT` names an already running private
  real LocalAudio endpoint, show `11111111-1111-4111-8111-111111111111`, epoch `100`.
  The host invokes the compiled test executable with
  `--ignored --exact gp07_external_driver --nocapture`; no Cargo is needed on the
  host. It performs at least 17 real reviewed edits, including two independent
  frequency/gain/Q settings per band, crossed cascade, all four per-band bypasses,
  global bypass, neutral, positive compressor GR and a distinct second channel.
  It waits for ready state and at least 576 additional source frames per edit,
  cancels context, reconnects without replay and drops Frontend within 60 seconds.
  Evidence includes each full config and the effective frame from the correlated
  successful final reply, plus individual semantic field-action evidence.
  The host owns sample, REC/analysis and module-order assertions. `GP07_DRIVER_EVIDENCE` optionally names
  the small JSON result. `GP07_SCENE_EVIDENCE` optionally names a single PPM scene.
  No REC authority is requested by Desk.
- `gp07_legacy_disconnect_requires_explicit_gp03_reconnect`: `SHR_DESK_GP07_LEGACY`
  names the independently verified unchanged v1 provider for the explicit F8
  recovery check described below.

Use the same parent-held nonblocking build lock and explicit CPU ICD environment
for native integration. Actual validation status is recorded in [Status](STATUS.md).

The original `gp07/v1` corpus is unchanged compatibility evidence. This consumer
refuses v1 snapshots and replies; it never relabels shelf settings as bell settings.
`gp07_legacy_disconnect_requires_explicit_gp03_reconnect` uses
`SHR_DESK_GP07_LEGACY` for a hash-verified unchanged v1 provider: ordinary GP03
attaches, the explicit v2 probe fails with the observed transport error, and F8
restores GP03 controls through a fresh read-only attachment with no replay.
The new provider's explicit unsupported-version refusal to old clients is a
producer-owned compatibility check.

`native::offscreen_at` uses the production viewport and shader to compare actual
CPU Vulkan readback with the aspect-fitted bitmap scene, including letterboxing.
Actual-provider layout acceptance checks Channel and protected review at full HD,
reduced, portrait and enlarged sizes; zero size suspends presentation and cannot
satisfy the review gate. Select only the explicit lavapipe ICD and unset display
variables. Physical readability, devices and displays remain unverified.

Local injected-controller field gestures, like keyboard field gestures, enqueue no
provider release messages; Apply still enters the same shared protected review.
This keeps a burst of unsent field edits from filling the authority queue or
delaying paired observations. The 250 ms gate and original revision pin remain.
The native presenter uses direct framebuffer-pixel mapping for deterministic
scaled texel selection and requests the adapter's supported resolution limits,
so enlarged targets are not incorrectly capped at the 2048-pixel fallback limit.

## Task0014 dynamic provider work (acceptance pending)

Add `--dynamic` to explicitly select C-AUDIO:2, GP03-rendered:2 and
GP07-processing:3. Default legacy1/1/2 remains supported; a selected session
refuses a different version. Standalone GP02 file mode stays legacy-only.
The producer supplies topology, common-clock observations and resource admission.
Logical input IDs, physical socket labels and USB stream slots stay separate.
F7 displays all inputs, measurement slots and output routes across pages, including
unassigned silent outputs. Expected/reference mapping is never hardware evidence.
Clock lock UNKNOWN remains unknown; no clock selector or configuration is added.

Processing selection resolves stable authority IDs against processing readback,
independently of array order. Inventory/map changes preserve stable selection where
possible, discard local intents and require a refreshed worker generation. Unknown
monitor scopes cannot alias monitor2 or FOH. Twelve-strip banking is presentation;
a final partial bank wraps to the first bank. Wire scopes retain monitor1/monitor2
and use the exact external enum object for additional monitors.

For `pa_configuration` or `output_routes`, G requests the separate configured
scope. F9 opens a detached editor from fresh structural readback. PA fields show
owner JSON paths and values; U/I select fields (including source objects, enum
strings, arrays and null), J/K adjust one numeric owner unit or toggle, and decimal
entry plus Enter sets a local field. F3 enters case-preserving JSON text for the
selected field: for example `{"input":1}`, `null`, or an owner enum string in
quotes. Enter accepts the local JSON; Esc cancels text entry. F3 also accepts
`@/absolute/path.json` to import a complete detached PA document with exactly
`configuration` (the owner JSON object) and `program_buses` (the ordered bus array).
The file must be regular (final symlinks and FIFO/device paths are refused), is
read only on this explicit action, and is bounded to the existing 48 KiB
command budget. `Action::StructureImport` provides the same semantic controller
path. Arrays and objects can be replaced to express weighted routes and topology;
the actual PA owner validates supported enums, dimensions, sums, cycles and ranges. This does
not perform DSP or advertise unavailable owner controls. Output-route fields cycle
explicit advertised sources for each physical output with J/K, including unassigned
silence. F4 opens the complete paged protected review; Enter confirms only after
every page was presented. Esc discards. Z requests reviewed output mute; X requests
reviewed explicit rearm under PA authority. Structural prepare requires confirmed
quiescence; no automatic unmute follows patch/configuration/reconnect.
All imported fields remain unsent until F4 and complete paged review/confirmation.
Preview rows abbreviate long values; the review contains the full replacement
document and bus map. Import does not grant authority, prepare DSP or rearm outputs.

`local_audio::AuthorityConnection` carries framed bytes while Operator/Session
retain leases, freshness, one pending request, retries and no-replay recovery.
The successor uses GP14 immutable document segmentation:64KiB frame bound,
8KiB UTF8 segments,1MiB aggregate admission, ordered indices, SHA256 identity and
2s deadline. Partial/mixed/reordered/corrupt sets are never observations.

`--remote-config FILE` explicitly chooses mutually authenticated QUIC for a real
frontend and implies dynamic versions. The JSON has `bind`, `server` (private
socket addresses), `server_name`, absolute DER `certificate`, `private_key`, `ca`
paths, `server_certificate_sha256` and paired `peer_id`. Provision privately; no
credential discovery, issuance or pairing occurs. The positional Unix endpoint and
writer arguments retain CLI compatibility; remote mode uses only the configured
QUIC endpoint and TLS-bound writer. CA validation plus pinned server leaf and TLS
exporter session binding are required. Hello grants nothing; fresh authority and
explicit scoped grants remain necessary. Remote mode does not start a separate
legacy Unix health observer. UI timing never advances the audio clock.

The normal suite includes legacy compatibility, scope, bank/context and immutable
framing regressions. `gp14_supplied_producer_documents` consumes root-supplied
profile files through `GP14_CORPUS`; the checked-in final corpus is hash-bound to actual producer execution; runtime
operator/sample acceptance is recorded separately.
`gp14_high_channel_external_driver` and `gp14_structural_external_driver` are
explicit ignored actual-provider drivers in `tests/gp07_frontend.rs`, bounded45s internally (use an external55s timeout to retain failure diagnostics).
They require `GP14_EXTERNAL_ENDPOINT`, `GP14_EPOCH`, optionally
`GP14_REMOTE_CONFIG`; evidence paths are `GP14_DRIVER_EVIDENCE` and
`GP14_STRUCTURE_EVIDENCE`. The host owns actual sample/module assertions. These
drivers are not proof of acceptance until executed against verified artifacts.
Remote replies reassemble the producer's complete paged Response envelope before
session-binding validation and payload dispatch; the already assembled payload
does not pass through the Unix 64 KiB frame gate a second time.
No normal run opens a physical endpoint/window; two-node runs require reservation.

The structural wire query uses `state: "snapshot"`. Boundary finals may omit the
snapshot; Desk retains the correlated outcome while treating readback as stale.
After a map change retires the connection, F5 obtains a fresh map before further
review or rearm. A final is not a configuration snapshot.


For the dynamic 48-input remote acceptance driver on Pi4, use the optimized release
profile. A finite three-pass diagnostic over actual captured producer pages found
that the unoptimized strict decode pipeline alone exceeded the unchanged 250 ms
paired-observation budget (297–307 ms debug; 34–40 ms release for the same captured
48-input pipeline). This is a software decode measurement, not network or
hardware throughput qualification. The explicit ignored library test
`remote::envelope_tests::captured_producer_decode_timing` reports separate assembly,
outer-envelope, raw decode/validation and structural decode/validation costs without
opening a network endpoint. `GP14_TIMING=1` enables paired-request stage timings,
observation flags and admitted revision/frame diagnostics in an authorized driver
run. It changes no limits or validation.

The operator line retains the last explicit operation alongside subsequent health
status. Actual acceptance waits for that grant confirmation and the confirmed scoped
lease's remaining lifetime; generic freshness cannot satisfy a grant. Detached
review creation is labelled review-ready, never as a newly applied command.

The actual structural acceptance driver pins every readback to the correlated final
revision. Mute waits for observed quiescence, PA replacement waits for the exact
owner document/bus map, and patch reconnect waits for the changed map/source with
no writer lease. Rearm requires an unquiesced observation at least 240 source frames
past the final's effective frame. A final boundary acknowledgment alone does not
prove that a ramp has completed. The separate 48-input driver covers all 24 fields
at inputs 16/17/32/33/48; the 16/32 profile runs use structural and media witnesses.

## Task0015 Brain operator audio (implementation in progress)

`--brain-audio` explicitly selects the dynamic C-AUDIO2 session and probes
GP15-brain:1. The API equivalent is `Frontend::enable_brain_audio()`. Attachment
stays read-only. Local listen, talkback destinations and protected FOH use the
separate `local_operator_monitor`, `talkback_destinations`, and `talkback_foh`
grants. They retain the existing session identity, request counter and revision;
Desk does not open PCM or run DSP.

F3 opens Brain. `0` selects none, `1` main, `P` selected-input PFL and `L` AFL.
`U/I` browse actual monitor buses and `O` selects that bus as the sole listen
source. Source changes are disarmed; after fresh device readback, `B` requests
separate arming; actual path readiness follows provider prefill. `M` toggles monitor mute, `D` dim and `+/-` adjust gain in 1 dB
steps. Every configuration change opens the complete existing review workflow;
Enter confirms only after every page was presented. Gains use integer centidB
from -9000 through 0. Dim is the -20 dB behavior; provider readback remains
authoritative. PFL is post EQ/compressor before mute/fader/pan, centered; AFL is
after all those controls, stereo. Performer monitor sends keep their raw
post-mute tap.

`V` toggles the browsed bus in the talkback destination list, `X` toggles talkback
mute and `[/]` adjust talkback gain. `F` requests protected FOH inclusion through
its separate grant. Default provider state is muted with no destinations and FOH
excluded. `T` is a held PTT edge: native key-up closes it. Typed controller actions
share this path. `configure_talkback_controller(channel,note)` and
`inject_talkback_midi(bytes)` accept explicitly configured complete semantic MIDI
edges without opening MIDI. A release is required after bind/fence; note-off and
note-on velocity zero close, and repeats/pressure cannot start another hold.

PTT generations come from the confirmed producer highwater. Heartbeats run only
while the UI continues pumping a live held gesture; an expired UI liveness token
cannot be revived by a later pump. Heartbeats include the latest confirmed source
frame, run at 50 ms, and require an observation no older than 50 ms. The provider
owns the 150 ms deadman and 240-frame/5 ms fade. Key-up, focus loss, input overflow,
controller removal, session/lease loss and faults stop the held signal. A bounded
priority close uses the same authority counter even while ordinary work is
pending; a stale revision can refuse it, so the provider deadman is the final
bound. Reconnect never replays a hold or configuration.

Sample meters are actual integer nano-amplitude observations (1.0 FS = 1e9),
presented with units. Requested state, granted authority, applied revision/frame,
readiness and stale observations remain distinct. Physical identity, clock lock,
controller/display operation and audio acceptance are not inferred from fixtures.
The ignored `gp15_frontend` driver uses real Frontend actions against an explicitly
configured mTLS endpoint; it requires a mutually acknowledged reservation.

Brain `E` opens a device draft copied from the latest confirmed configuration.
`F2` enters a complete JSON replacement, `F4` opens the paged complete mapping
review, and Enter applies it through GP15-device on the existing authenticated
connection. No guessed endpoint or socket labels are inserted. Confirmed mapping
includes one capture microphone and exactly two distinct stereo playback slots.
The provider owns capability admission. `pending`, `accepted_intent`,
`applied_device`, and `failed_device` are distinct outcomes; only a matching ticket
and exact configured mapping in actual device readback qualifies as applied.
Device configuration never implicitly arms the listen path.

The device pane presents provider ratio/skew in parts per billion, queue occupancy
and target in frames, queue/filter nominal latency in microseconds, error counters,
and nullable physical mapping uncertainty in milliframes. Capture drops count
48-sample microphone blocks refused by the bounded capture queue, as reported by
`status.capture_queue_dropped` (u64). Older v1 status may omit this additive field;
the pane shows `--`, never an invented zero. Unknown status keys and malformed
counters remain rejected. These are software
observations; clock lock and physical mapping stay explicitly unverified unless
supplied as actual producer observations.

Device drafts, queued reviews and confirmation independently pin the actual Brain
device epoch and map, in addition to authority revision and session generation.
Replacement/restart or stale device readback discards the old intent; a fresh
confirmed configuration requires a new explicit edit, review and confirmation.
These local pins do not change the GP15-device wire schema.

The ignored `actual_mtls_frontend_readonly_restart_probe` in `gp15_frontend`
attaches to an explicitly restarted provider using `GP15_REMOTE_CONFIG` and
`GP15_EPOCH`, with optional `GP15_SHOW`. It requests no grant and waits up to ten
seconds for fresh Brain and actual device observations, then asserts no writer
lease, held talkback, monitor arm or device arm. `GP15_EXPECT_BRAIN_EPOCH_MIN`
is an optional inclusive minimum; pass the old Brain epoch plus one to require
strict growth. `GP15_READONLY_RECOVERY` emits the exact observed snapshots.
Run only under the coordinator's existing bounded reservation; compilation alone
is not restart acceptance. The existing `GP15_FAULT_MODE` branch is unchanged.

The talkback driver's numerical witness retains an earlier Main interval, then
explicitly selects and rearms the last performer monitor used as its talkback
destination. That pre-talkback monitor return uses unity gain, unmuted and undimmed,
so the coordinator can compare it with the actual selected physical output.
The driver retains that selection for at least one second held and 350 ms after
release. The fault marker follows a one-second held witness; its existing closure
and two-second no-resurrection checks remain. These are driver assertions;
independent provider sample evidence is still required.


Held PTT uses a separate bounded worker service. Initial hold and heartbeat sends
record their send-start instant. Pending completion, the next solicited heartbeat
probe and its send share the previous send's 50 ms deadline; completion and
readback after a successful heartbeat use that new send's next 50 ms period.
A shared 64-frame cap spans the service. Due lease renewal uses that same remaining
period and exact correlated completion; it cannot enter the ordinary 1900 ms
mutation wait. If the period cannot accommodate the operation, the gesture closes
without retransmitting a heartbeat or automatically granting/rearming. The existing
30 ms probe / 20 ms send limits and provider 150 ms deadman remain unchanged.
Passive device, processing, structural and general polling run only outside a
hold. Queued ordinary actions end the current gesture before any blocking work;
a later hold always requires another explicit press. This software path still
requires integrated and physical acceptance; tighter scheduling is not a measured
network or acoustic latency claim.

Compact-proof reads and correlated completion check the live input at every
wait boundary and after strict reply validation. Key-up during held maintenance
stops the held service
without discarding outstanding query identities; passive release readback can
continue on the healthy connection. A successful no-byte wake can observe this
cancellation, but partial frames, malformed replies and correlation errors remain
terminal even when release occurs concurrently. The same original deadlines and
frame limits apply; passive queries do not require an active hold.

Authenticated remote PTT now uses `GP15-held-proof:1`, a fixed-size, explicitly
correlated witness for the `talkback_destinations` scope. A matched full raw/Brain
readback pins the reviewed talkback configuration digest while unheld. The digest
binds show, source epoch, capability/map generations, actual dimensions, destinations,
gain, mute and protected FOH intent. It excludes unrelated monitor selection.
Initial press and heartbeat verify that unchanged digest,
live scoped lease and source-frame authority with their own compact query; they
never fetch a full topology inside the held budget. Missing or changed baseline
requires full readback and another explicit gesture, without automatic replay.

Only one compact query may be outstanding. Cancellation retains its exact nonce
and generation until its validated reply is drained; a new query gets a new
send-start timestamp inside the same original deadline. Unknown, malformed,
partial or mismatched replies remain terminal. Proofs never refresh the full raw
snapshot, configuration review or draft. The UI separately shows current compact
held generation/readiness and the age of actual Brain sample-meter observations
from correlated finals. Passive monitor/FOH and unheld renewal use the separate
atomic lease-maintenance operation below, without a topology query. New Desk PTT
on Unix or older providers without authenticated compact
proof support is explicitly unsupported and stays closed. Brain-scope maintenance
also requires the authenticated additive operation; nonrenewal Unix controls and
the existing GP15-brain:1 wire contract are unchanged. No fallback opens a hold.

Authority validation reuses the typed semantic checks and a bounded counting
serializer instead of allocating and reparsing canonical authority JSON. Wire
admission still checks duplicate fields, nesting, integer values, exact target
field shapes and required fields. Public typed snapshot ingestion retains the
same version-specific serialized byte caps, including canonical nullable-option
semantics. These CPU changes do not alter freshness or held-service deadlines.


Set `SHR_DESK_TRACE_TIMING=1` before creating a Desk connection to opt into
bounded control timing diagnostics (disabled by default, sampled once per owner).
The existing first paired-observation deadline failure includes cumulative
microsecond counters and a four-frame ring; overwritten records and arithmetic
overflow have separate counters. No per-frame log or payload is retained.
Operator stages are receive wall time, contract discrimination/dispatch, raw
strict decode, and telemetry ingestion. Send time includes encoding; completed decision-span
time includes any follow-up send and therefore must not be added to send time.
The remote counters separate first-byte await, remaining prefix/body await,
page assembly, strict envelope decode, payload serialization, outbound envelope
serialization, command parse/envelope construction and write await. Write await
ends at local stream-buffer completion, not actual packet transmission. Outbound
sub-stages overlap the total send counter. Await includes
runtime scheduling and QUIC progress, not only network delay. The unchanged
release benchmark remains available for isolated CPU measurements.

Transport snapshots expose Quinn's actual connection-wide RX/TX MAX_DATA,
MAX_STREAM_DATA, DATA_BLOCKED and STREAM_DATA_BLOCKED counters, RTT and lost
packets, plus RX/TX UDP datagram counters. Before/after values can corroborate a stall, but cannot attribute it to
one request or prove flow-control causation. Send Pending counts are not measured.
`None` means the transport supplies no timing data, rather than measured zero.
The ring is indexed modulo four (the frame count identifies its oldest slot).
The terminal deadline-break decision span is not included in decision time.
Counters start after authenticated hello: the numeric TLS-bound session ID
identifies the connection, send attempts count validated commands entering the
write path, and completed sends count successful local stream writes. A partial
write increments attempts without completion. Completed reply documents count
only fully assembled, correctly session-bound `Reply` envelopes whose payload
serialization succeeded, not pages, refusals or wrong-session responses. These
counters saturate with the same overflow flag.
Only the existing failure path formats these fixed numeric records; successful
frames produce no trace I/O. No wire fields, flow-control windows, deadlines,
permission checks or retry behavior change.


Full readback and correlated completion can receive an opaque strict document
from the transport. Only the existing duplicate/depth/integer/UTF-8/byte-bounded
parser can construct it. Remote page assembly retains that parsed envelope;
exact envelope fields and the authenticated session are checked before moving
its payload subtree. A counting serializer preserves the historical canonical
payload byte length and version-specific admission. Raw reply shape, target
fields (including explicit null fields), typed semantics, freshness and FIFO
correlation still use the same admission code. Public byte receive and
`Assembly::offer` interfaces remain available; their byte behavior is retained.
Default connection implementations obtain a proof through the strict parser,
never from an unchecked `Value`. Small processing/Brain/device messages retain
their existing typed byte decoders; the large raw path avoids repeated envelope
parsing, payload serialization and raw reparsing.

The opt-in20-iteration16/32/48 benchmark uses the production strict-document
assembly, payload extraction, discriminator and raw admission path with the same
fixtures. Its stages measure elapsed `Instant` time including scheduling, not
thread CPU time or loaded network deadlines. Payload serialization reports zero
because that stage is eliminated. In live transport diagnostics, assembly now
includes the single strict envelope parse; envelope admission includes canonical
payload counting. The legacy byte adapter still records its payload serialization
when used. No deadline, retry, page checksum/order/size, authority or safety limit
changes accompany this optimization.


### Task0015 atomic maintenance and committed paired readback

Authenticated Brain sessions use additive `GP15-lease-maintenance:1` and
`GP15-paired-readback:1`. Maintenance extends only the already granted scope and
lease; it does not change the revision, configuration, held generation or heartbeat.
All three Brain scopes use it, including passive monitor/FOH and active talkback.
The local expiry is anchored to the first send plus 2000 ms. Retries preserve exact
bytes and the original operation/lease deadline at the existing 100/250/500 ms
wakes. Up to 64 validated terminal replies are retained to discard exact late
duplicates without extending authority. Only explicit terminal `unavailable`
permits a new maintenance ID within
that original budget. Unknown, canceled, unsupported or malformed outcomes clear
local authority; no grant, replay or legacy renewal fallback is inferred.

Full Brain refresh requests one committed raw/Brain pair using an independent
connection nonce. Both existing strict schemas, identity/map, topology, revision
and frame must validate before either view is installed. The original 250 ms and
64-document bounds include decoding, and both freshness clocks retain query-send
time. A compact held proof still independently gates every heartbeat; maintenance
never refreshes the UI pair or substitutes for proof. Held maintenance remains
inside the next heartbeat period and its 20 ms operation bound.

The opt-in driver checks held status immediately after the talkback frontend pump,
before pumping the other surfaces, and includes assertion-local age in failure
output. Its 50 ms freshness assertion is unchanged. This ordering is diagnostic
hygiene, not evidence that provider closure preceded any later consumer failure;
independent continuous provider sample checks remain required. Focused offline
checks are implemented; combined campaign and hardware verification are separate.

### Monitor device observation at admission

Every reviewed `brain_monitor_set` whose resulting state is armed (including gain,
dim and mute edits while armed) pins the UI's observed device epoch, map and complete
configuration before queueing. Review and confirmation refresh the actual device
after their raw/Brain reads, then recheck that pin and the original revision and
session generation. Refresh cannot adopt a replacement device or configuration.
Aged background cache may be refreshed; missing, disconnected, duplicate-stale or
changed observations refuse the operation. Device freshness remains 250 ms and
is conservatively anchored no later than query send; decoding and cancellation
remain inside the existing bounded read. The same device pin and freshness gate
protect byte-identical pending retries. Failure never replays a new mutation;
replacement requires a new explicit review/rearm. These are local safety checks;
GP15 and atomic maintenance/paired-readback wire contracts are unchanged.
