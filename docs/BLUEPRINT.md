# SHR Desk blueprint

Decision date: 2026-10-04. This is the owning surface plan. It extends the
[preserved October 3 Brain plan](archive/brain-console-2026-10-03.md).
The earlier documentation-only restriction is historical; the current user
request authorizes this new project and its initial implementation.

## Product and boundary

GigPies is the complete live-band system. Automixing is one source of control
decisions within it. SHR Desk is the human interface: it observes, selects,
requests edits and explains what the core actually applied. It does not sum
audio, run channel EQ/dynamics, compute an automix, align speakers, record audio
or host effects. Full manual operation must be possible without an automixer.

**Physical node and repository ownership are different axes.** The Stagebox/PA
node owns the live mix graph; `shr-pa` owns speaker processing and measurement.
Do not turn the existing 2-in/6-out PA processor into the entire band mixer merely
because the node is called PA. The integrated live graph/host currently belongs
to GigPies. A future core extraction deserves its own task, API and migration;
this change does not create a speculative `shr-mix` repository or move DSP.

```text
                         BRAIN
  MiniLab / keyboard -> SHR DESK -> GigPies control service
                          ^             | authority / validation
                          |             | commands + acknowledgments
                   snapshots /         v
                   observations    STAGEBOX / MIXER
                                channel graph / buses / monitors
  automix / doctor proposals ---> arbitration -> applied parameters
                                |          |          |
                              SHR PA     SHR REC    FX sends
                            protection   local disk    |
                                              BRAIN: SHR FX
                                              wet return only
  SHR LUX <----- source activity / show cues, outside the audio path
```

The window, MIDI adapter, recorder observer and analysis panes can disappear
without stopping the Stagebox. A display restart reconnects to current state;
it never recalls yesterday's scene. Engine-held human overrides survive UI loss.

## Module inventory and gaps

Read on 2026-10-04, including each relevant README and the focused documents below.
Local docs include unreleased work; source revisions and dirty-state distinctions
are recorded in [status](STATUS.md). These are integration findings, not a claim
that every sibling was retested.

| Owner | Available evidence | What Desk consumes | Missing for the intended product |
|---|---|---|---|
| GigPies | Offline `src/automix`; frozen reports; GPA1; stereo USB host; `docs/AUDIO_TRANSPORT.md`, `AUDIO_HARDWARE.md` | Channel identities, applied state, control ACKs, health, automix proposals | Complete multichannel live mixer API, bus graph, scopes, manual/auto arbitration, safe scenes |
| SHR Desk | This offline model, MIDI primitives, state-driven SVG layout | No DSP ownership | Native window, focus/navigation, complete actions, adapters, performance and hardware acceptance |
| SHR PA | `docs/STATUS.md`, `DSP.md`, `EMBEDDING.md`; configurable PA v2 and preserved fixed-v1 embedding | Protection/clip/fault health, exposed PA controls | Actual PA/output controls; setup-mic/phase/alignment work remains in PA |
| SHR FX | `docs/PLAN.md`, `INTERFACE.md`, `ARCHITECTURE.md`; wet-only engines and source-frame embedding | Send/return identity, effect descriptors, parameters, reduced-mode health | Remote control/state adapter; surface must not duplicate its effect engine |
| SHR REC | `docs/STATUS.md`, `RAW_RECORDER.md`; bounded PCM24 library, journal and recovery | Written frames, active take, gaps, capacity, arm/start/stop acknowledgments | Complete integrated operator workflow; standalone shell buttons are not recorder readiness evidence |
| SHR LUX | `docs/index.md`, MIDI/analysis notes 0009 and 0018; simulation and MiniLab colour work | Cues, source activity and lighting status | Shared controller ownership, live DMX acceptance and a control API |
| SHR DAW | `docs/WORKSPACE_HANDOFF.md`, `CONTROLLER_LED_FEEDBACK.md`, MIDI pickup/learning | Interaction reference, profile conventions and provenance | Do not depend on its workstation state/graph to build this surface |
| SHR Synth / Sampler / Drums | Independent instrument engines; sampler live process contract; synth host/preset docs | Later named source/plugin status if GigPies integrates them | Not required for a band desk; no sequencer or instrument hosting in Desk |
| SHR Tone Over 9000 | Independent NAM/cabinet processor | Optional source identity/status | Amp processing stays there; unnecessary for first console |

Yesterday's two-Pi work is relevant but limited: GPA1 source frames, bounded
queues, recovery and wet-return handling exist. GPC1 currently changes a bounded
test scalar, not a real fader. H8 stereo USB/recording evidence does not prove
36-channel mixing, surface responsiveness, clock lock or full-show reliability.
Development SSH/Git exchange is not a runtime control transport.

## Shared structure

Begin with one small independently buildable crate, separated by responsibility:

```text
src/model.rs    surface state + explicitly synthetic authority for tests/demo
src/midi.rs     complete-message decoder, pickup, relative modes, LED encoder
src/render.rs   scene primitives, 1080p layout, font and SVG review backend
src/main.rs     offline CLI, injected events and gallery
tests/         production contract/recovery/layout checks
docs/          blueprint, decisions, contracts, screen/controller maps, status
assets/fonts/  unmodified licensed font and licence
artifacts/     ignored generated previews and temporary evidence
```

Next separate the simulator from the client as the real adapter arrives. Add
window/GPU, input worker and transport adapter modules only when they have
working implementations. No empty workspace crates, sibling path dependencies
or copied module algorithms. Runtime capabilities drive available controls.

Across modules use a common vocabulary: stable ID, module identity, parameter ID,
units/range/step, read/write capability, schema version, epoch, revision, source
frame, freshness, pending/applied/rejected, unavailable/fault/degraded. GigPies
owns the cross-module wire contract and version compatibility. Desk owns how
that contract is presented. A shared Rust contract crate is justified when two
real consumers need it; integrate through an exact released or Git revision and
Cargo.lock, not `../` dependencies. Versioned C DSP ABIs remain a separate boundary.

## Human and automatic control

| Mode | Human operation | Automixer behavior |
|---|---|---|
| AUTO | Every permitted control available; editing an automated parameter takes a hold on that parameter | Bounded changes only on parameters granted to automation; show current proposal/reason/bounds |
| ASSIST | Human owns applied changes and can accept a scoped proposal | Proposes and explains; does not apply |
| MANUAL | Full desk workflow, independent of automixer availability | No automatic musical parameter edits; observations may continue |

Protection, numerical fault handling and engine-owned bounds apply in all modes.
Mute is an explicit human state: automation cannot silently unmute a channel.
PFL/AFL are monitor auditions, not changes to the audience mix. Solo-in-place is
outside initial scope. FOH, each monitor mix and PA setup have separate ownership.
Selection is not write authority and never changes a parameter.

A manual change begins from the last applied value, not an unseen automix target.
It takes a persistent parameter hold; neither elapsed time nor releasing a key
returns it to automation. The console shows actual value, proposed value, owner
and pending operation. Returning to Auto is a reviewed scoped action: show the
delta and engine-specified ramp before committing. A mode change preserves current
levels and holds them until explicitly released. Never jump a whole mix on a
mode toggle. The prototype demonstrates this policy only for channel fader gain;
full per-parameter ownership and engine-side ramps remain planned.

## Native graphics direction

Use a native window on Brain with a **TUI appearance**, not terminal escape
sequences stretched to HDMI. Retain `winit` + `wgpu` as the first prototype choice.
Use the same scene/view model for headless visual review; the SVG exporter is
implemented now and is not the production renderer. A single primary native
frontend is intended; consider egui only if measured focus/widget cost warrants it.

Baseline: 1920×1080 physical pixels, 12×24 cell rhythm, 160×45 cells, the existing
Uni2-TerminusBold24x12 font, pixel-aligned borders and a small symbol vocabulary.
Glyph atlas batches and graph textures share the GPU. Graphs are pixel surfaces,
not restricted to character cells. A larger-text profile and display scaling need
actual viewing-distance acceptance. The first slice uses the real bitmap font in
SVG paths, so font review does not depend on a browser's substitute font.

Analysis runs outside the window and audio callback. Subscribe to selected taps;
do not pass full raw multichannel audio into widgets. Spectrogram columns use a
bounded rolling texture; stereo plots use a fixed point/density budget. Acquisition
epoch/frame/tap/calibration/freshness belong with each graph. Missing is `--`, not
zero. Meters have explicit sample/true-peak/GR meaning. FFT/window/dB convention
must be declared before a graph is presented as a measurement.

Targets inherited from the October 3 plan: 60 Hz presentation, renderer CPU p99
under 4 ms, GPU p99 under 4 ms when measurable, local MIDI dispatch p99 under 5 ms,
visible feedback submission p99 under 33 ms; meters 30–60 Hz, selected analysis
20–30 Hz. These are unmeasured targets. Event-driven idle rendering, bounded
mailboxes and analysis degradation precede any loss of command responsiveness.

## Delivery sequence and acceptance

| Stage | Work | Gate |
|---|---|---|
| D0, this checkpoint | Ownership, reference study, screen/controller plans, simulation, renderer drafts | Headless contracts and actual full-HD drafts; no claim of live operation |
| D1 | Native glyph/rectangle/line/texture renderer; Mix/Channel/Analysis navigation | Deterministic fixtures, readable HDMI, resize/device-loss recovery; measured frame costs |
| D2 | Complete keyboard/controller action table and recovery TUI | Every promised operation reachable without mouse; focus, modifier, pickup and stale-context tests |
| D3 | GigPies control API plus authoritative simulator/adapter agreement | Capabilities, scope leases, stale/conflict/retry/restart tests; read-only attach first |
| D4 | Real fader/pan/mute, monitor/PFL, automation holds and scoped scenes | Engine smoothing, no surprise recall, audio unaffected by UI/MIDI loss |
| D5 | FX, recorder, PA, doctor and lights pages through owning adapters | Each module's own contract and failure evidence; unavailable controls stay disabled |
| D6 | Controller + HDMI + combined-load acceptance on chosen Brain | Explicit resource reservation, exact firmware/build/load, full recovery and timing evidence |

D0 is deliberately bounded: three rendered screen drafts and a functioning control
simulation, not completion of D1–D6. Next implement the native window over these
primitives, then a complete navigation/action model. Do not bolt real UDP writes
onto the test model and call the result a console. CPU/GPU/thermal capacity and the
final Pi 4/Pi 5 runtime assignments remain measurement decisions.
