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

## GP07 channel processing (task0012)

The real frontend supports the accepted `GP07-processing:1` extension with an
explicit `--processing` option on `--native`, `--headless` or `--offscreen`.
Programmatic callers use `Frontend::enable_processing()`. This opt-in protects
legacy providers that close a connection on an unknown contract: ordinary GP03
attachment sends no GP07 query. Unsupported or failed probes remain unavailable;
start without the option for a legacy provider. A valid processing snapshot must
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
when unavailable. All three EQ bands, compressor parameters, explicit makeup and
both independent bypasses are editable. Existing fader/pan/mute/hold/mode controls
remain available outside a processing draft. Selection/page/bank/revision changes,
role loss, focus/device loss, overflow and reconnect revoke drafts and reviews.
Reconnect uses fresh read-only authority and never replays settings.

Producer fixtures are pinned in `tests/fixtures/gp07/v1/ACCEPTED.json`. Normal
`gp07_codec` tests exercise exact bytes, strict domains, shared authority,
nonadmission and late/mismatched replies. UI regressions protect numeric entry,
keyboard/controller parity, layout and context loss. These fixture checks alone
are not actual-provider or audio acceptance.

Two explicit ignored tests in `tests/gp07_frontend.rs` use the actual frontend:

- `gp07_actual_release_provider`: `SHR_DESK_GP07` names an independently verified
  release provider. It launches and joins only its own temporary private service.
- `gp07_external_driver`: `GP07_EXTERNAL_ENDPOINT` names an already running private
  real LocalAudio endpoint, show `11111111-1111-4111-8111-111111111111`, epoch `100`.
  The host invokes the compiled test executable with
  `--ignored --exact gp07_external_driver --nocapture`; no Cargo is needed on the
  host. It edits channel 1 through keyboard and channel 2 through injected semantic
  actions, checks confirmed readback/visible scene, cancels context, reconnects
  without replay, and drops Frontend within 20 seconds. The host owns sample,
  REC/analysis and module-order assertions. `GP07_DRIVER_EVIDENCE` optionally names
  the small JSON result. `GP07_SCENE_EVIDENCE` optionally names a single PPM scene.
  No REC authority is requested by Desk.

Use the same parent-held nonblocking build lock and explicit CPU ICD environment
for native integration. Actual validation status is recorded in [Status](STATUS.md).
