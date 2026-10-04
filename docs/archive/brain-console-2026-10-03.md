# Brain console: full-HD implementation plan

Status: **future implementation; no console code or hardware acceptance yet.**
Owner: GigPies integration and operator interface. Recorded 2026-10-03.
This document owns the console task. [Architecture](ARCHITECTURE.md) owns node
responsibilities; [Components](COMPONENTS.md) owns the sibling module boundaries.
The present task authorizes documentation only. The phases below are future work.

## 1. Fixed requirements

- The Brain uses an **HDMI display at 1920 × 1080**. Full HD is the baseline.
  The small display belongs to the PA unit and does not constrain the Brain layout.
- Build a native graphical console with the speed, density and keyboard operation
  of a TUI. Use crisp drawing glyphs, deliberate shading and Turbo Vision-inspired
  panels, with pixel graphics for spectrograms and stereo views.
- A small MIDI keyboard/controller is the intended physical control surface.
  Keep ordinary keyboard navigation available. The exact controller model and
  messages are still to be inspected; proposed control counts are not facts.
- Target 60 Hz presentation. Performance figures below are acceptance targets,
  not measurements or promises about either Pi.
- PA owns the continuous mixer, physical audio I/O, protection and local NVMe
  recording. Brain owns the console, analysis/doctor, lighting coordination and
  the richer send/return FX. Music-analysis ideas belong to a separate future task.
- Live audio design remains 48 kHz, 24-bit capture and `f64` mixer DSP. Display
  geometry/textures may use ordinary GPU formats; FP64 audio does not require
  FP64 graphics. Recording containers and network sample formats need their own
  contracts and are not selected by the display design.
- Brain/UI failure must leave PA main/monitor audio and recording running. Local
  PA controls and a defined reduced FX mode provide recovery operation.

These requirements supersede the earlier small-screen Brain proposal and the
earlier placement of multitrack recording on Brain. Physical Pi model assignments
remain open; development coordinator/worker roles do not decide runtime roles.

## 2. What exists and what is missing

Repository review at `fbcc9cb551508ef8d3e6533127928226e59e019a`:

| Area | Available now | Work needed for this console |
|---|---|---|
| Offline mix and review | Rust DSP, frozen settings, measurements, CLI and HTML EQ review | Read-only adapters and explicit simulated control state |
| Native UI | No windowing, GPU or TUI dependency in GigPies | Renderer, layout, focus, input mapping and screen components |
| Live control | Intended node boundaries | Authoritative state, commands, acknowledgments and recovery protocol |
| Analysis | Offline measurement and report code | Bounded live workers, subscriptions and time-stamped display data |
| PA / FX / lighting | Developing modules in their owning repositories | Stable interfaces and separate integration acceptance |
| Recording | Existing workstation reference code; SHR REC application shell | PA recorder implementation, bounded buffering and recovery |
| Peer development | Local runner and private task ledger installed | Assign bounded work when implementation is authorized |

An offline interactive console can precede the live engine. It should operate on
synthetic fixtures and saved reports, visibly labelled **SIMULATION** or
**OFFLINE REVIEW**. A working screen is not evidence of live routing or recording.

## 3. Rendering and application structure

Preferred direction: **`winit` + `wgpu` + a small GigPies console layer**.

- `winit` owns window/input events; `wgpu` draws batched glyphs, rectangles, lines
  and graph textures. Start with the target's supported Wayland/X11 session.
  Bare DRM/KMS and kiosk boot configuration are separate deployment work.
- Bundle a licensed monospace font and an explicit set of custom drawing symbols.
  Cache glyphs in an atlas; align borders to physical pixels. Rasterize new glyphs
  outside audio work. Support readable channel names and missing-glyph fallback.
- Own a small set of components: strip, meter, value editor, tab, table, dialog,
  routing cell and graph panel. Add layout/focus/theme helpers as needed; extract
  a reusable crate only when another application needs a stable boundary.
- Keep the existing CLI usable without a display or graphics initialization.
  Select a feature-gated console target or separate binary during C1, with locked
  dependencies and the repository's Rust version. Avoid sibling path dependencies.
- Keep a compact Ratatui/Crossterm recovery view on the same command/state model.
  It need not reproduce graphical analysis panes. Scope and write authority must
  remain explicit if multiple operator clients are connected.
- `egui` with custom GPU painting is the fallback implementation choice if custom
  focus/widgets consume disproportionate effort. Make this choice after the first
  bounded renderer prototype; do not maintain two primary graphical frontends.

The Brain control service owns state and command processing independently of the
window. Analysis and FX run outside the renderer. UI restart reconnects to current
state; it does not reload an old mix onto the PA.

```text
Keyboard / MIDI -> command router -> Brain control service -> PA commands + ACKs
                            ^                 |
                            |                 +-> module status / state snapshots
HDMI console / recovery TUI -+                              |
           ^                                               v
           +----- bounded snapshots and analysis frames ----+

PA: audio I/O -> f64 channels / buses -> monitors / PA protection -> outputs
                 |             |
                 |             +-> FX sends -> Brain FX -> wet returns to PA
                 +-> recorder worker -> local NVMe
```

FX send taps/routing remain in the PA graph; the richer effect engines run on
Brain. A failed Brain removes those returns through a defined fade/recovery
policy while the local dry mix, protection and recorder continue. Fallback FX,
channel capacity and audio transport are separate engine acceptance work.

## 4. Visual design and screen map

Start with a 12 × 24 pixel cell rhythm: **160 columns × 45 rows** at full HD.
Fonts and graph labels must be reviewed on the actual HDMI display. The cell
grid organizes controls; graphs use the full pixel resolution inside their panels.
Offer a larger-text layout within 1920 × 1080 if readability requires it.

Suggested main layout, including each panel's borders and padding:

- Top: two rows / 48 px for show name, page, control scope, PA link and recorder.
- Body: 39 rows / 936 px. Twelve 96 px channel strips, a 192 px master/bus area,
  and a 576 px selected-channel inspector fill the 1920 px width.
- Bottom: four rows / 96 px for controller assignments, bank, shortcuts and alerts.
- Full-width analysis/routing pages reuse the same header, status and navigation.
  Strip count is a view choice; it does not limit engine channel capacity.

```text
╔═ GIGPIES ═ SHOW / MIX ═ scope: FOH ═ PA: CONNECTED ═ REC: RUNNING ═╗
║ CH 01…12 — bank 1     │ MASTER / BUSES │ SELECTED: LEAD VOCAL     ║
║ labels · meter · GR  │ L/R + monitor  │ EQ / dynamics / sends    ║
║ pan · fader · mute   │ clip / status  │ spectrum + parameter     ║
║                     │               │ values and clear units   ║
╠═ BANK / CHANNEL ═ controller assignments ═ shortcuts ═ alerts ══╣
╚════════════════════════════════════════════════════════════════╝
```

This is a layout sketch, not an implemented screen or a measured screenshot.

Use charcoal backgrounds, pale text, cyan focus, amber attention and red faults.
Use box-drawing families, `░▒▓█`, inset value fields and small hard shadows.
Keep state readable through labels/shapes as well as colour. Use stable positions,
short animations and steady meter scales; avoid decorative motion during mixing.
Save explicit focus and controller-bank indicators on every page.

| Page | First useful contents | Dependency |
|---|---|---|
| Mix | Banked strips, master, selected channel, meters, mute and sends | Simulated state first; PA contract later |
| Channel | EQ curve, dynamics, parameter values, group/input identity | Existing offline models, then module capabilities |
| Routing / monitors | Source-to-bus matrix, send scope, pending/applied state | Authoritative routing contract |
| Analysis | Spectrum, rolling spectrogram, stereo field and correlation | Synthetic signals first; live analysis later |
| PA | Read-only health and integrated controls exposed by SHR PA | SHR PA interface; no duplicate alignment algorithms |
| FX | Sends, returns, engine state and reduced-mode indication | SHR FX integration and transport |
| Recorder | PA arm/run status, disk space, written frames and errors | PA recorder acknowledgment and telemetry |
| Doctor / lights | Current findings, proposed actions and module status | Existing evidence adapters; later owning-module work |
| Show / recovery | Saved scenes, connections, control owner and reconnect status | Versioned state/persistence contract |

Unavailable modules display their actual state. Empty data must not look like a
healthy zero reading. Preserve the distinction between an observation, a proposal
and a change actually accepted by the PA.

## 5. Control and data contracts

Define these independently of graphics before implementing live controls:

1. Stable channel/bus IDs, units/ranges, capabilities, scope and authoritative
   revision. Selection/banking changes focus without changing a parameter.
2. Commands include identity, target, expected revision and bounded values.
   Display pending, applied or rejected state; an acknowledgment identifies the
   applied revision. Reconnection refreshes state before enabling writes.
3. Define one active write authority per scope. Reject stale edits, deduplicate
   retries and never replay a disconnected backlog into a newly connected show.
4. MIDI mapping uses the actual device's note/CC/relative behavior. Define pickup
   for absolute knobs and scaling for relative encoders. Bank changes, reconnect,
   note-off and duplicate events must not cause accidental fader jumps.
5. MIDI command ingestion proceeds independently of GPU presentation. Keyboard
   events enter the same command path. Bounded command queues report overflow;
   discrete actions cannot silently disappear. Coalesce only parameter updates
   whose contract permits replacing an older value.
6. Telemetry may drop superseded frames. Each snapshot carries source identity,
   revision, acquisition time/frame position and freshness. Show missing/stale
   data explicitly; define cross-node timing before claiming signal latency.
7. UI settings persist separately from engine scenes. Reset has a visible scope.
   Loading a layout or restarting the UI cannot recall a mix automatically.

The PA recorder uses its own bounded worker queue and reports committed recording
progress, gaps and storage errors. Its status panel and network connection are
observers; rendering and Brain availability cannot become recording prerequisites.

## 6. Performance design and acceptance targets

Render only the visible detail graphs, while retaining cheap overview meters.
Use batched drawing and reusable buffers. Never send the entire raw multichannel
stream through widget code or run FFTs in the window event handler.

| Item | Initial target / implementation rule |
|---|---|
| Presentation | 1920 × 1080 at 60 Hz; 16.67 ms frame period |
| Renderer CPU work | p99 below 4 ms per active frame, excluding presentation wait |
| Renderer GPU work | p99 below 4 ms where reliable GPU timing is available |
| Control scheduling | MIDI receipt to local dispatch p99 below 5 ms; report PA acknowledgment separately |
| Visible feedback | Input receipt to submitted feedback frame p99 below 33 ms; physical display latency measured separately |
| Meters | 30–60 updates/s, with defined peak hold and decay |
| Spectrum / spectrogram | Initially 20–30 updates/s, selected-channel subscriptions |
| Spectrogram storage | Bounded rolling intensity texture; upload new columns only |
| Stereo view | Bounded samples or density texture; fixed history/decay and point budget |
| Idle pages | Event-driven redraw; no busy loop to refresh unchanged content |
| Memory / queues | Explicit capacities, overflow behavior and stable memory after warm-up |

The timing targets are provisional budgets to test on the chosen Brain. CPU/GPU
times can overlap and must not be presented as measured end-to-end latency.
Declare the adapter, driver, display mode, build, visible graphs, synthetic channel
count, analysis load and FX load with every result. A large synthetic channel count
tests UI scaling and does not certify mixer capacity.

Under pressure, reduce analysis refresh/history and decorative effects first.
Keep commands, critical state and meters responsive. Validate renderer recovery
after surface/device loss and HDMI reconnect without recalling state or stopping
the PA. Live audio callbacks retain bounded work with no allocation, locks or I/O.

## 7. Dependency-ordered implementation backlog

All entries are **planned**, with no worker assigned or task dispatched.

| ID | Deliverable | Depends on | Acceptance |
|---|---|---|---|
| C1 | UI state/command boundary, synthetic fixtures and dependency choice | This plan | Headless tests cover identity, scope, revisions and failure states; existing CLI remains independent |
| C2 | Full-HD renderer, font, navigation, strip and graph primitives | C1 | Deterministic fixture screens, readable 1080p layout and initial frame measurements |
| C3 | Offline console: Mix, Channel and Analysis | C2 | Complete keyboard workflow; known synthetic graphs; saved-report inspection without source mutation |
| C4 | Simulated MIDI adapter and compact recovery TUI | C1, C3 | Bank/pickup/reset/reconnect tests; both clients share commands and reflect conflicts |
| C5 | Live-control protocol and module adapters | C1 plus owning engine interfaces | Malformed/stale/duplicate/reordered/disconnected traffic tests; actual applied state visible |
| C6 | MIDI hardware mapping and Brain HDMI acceptance | C3, C4; explicit hardware session | Actual controller inventory, full-HD display checks, bounded performance run and recoverable UI restart |
| C7 | Integrated PA / FX / recorder / doctor / lighting pages | C5 plus each owning module | Per-module acceptance; Brain failure leaves the verified PA audio and recorder behavior intact |
| C8 | Deployment and combined workload acceptance | C6, C7 | Repeatable startup/recovery, declared load and retained performance/failure evidence |

C1–C4 can advance without completing the live mixer. Start C2 with one bounded
prototype and select the renderer before building many screens. C5–C8 depend on
missing live contracts/engines and separate integration or hardware authorization.
Finish and verify each slice before opening another; do not leave competing GUI
prototypes or speculative empty framework crates in the production tree.

## 8. Verification and retained evidence

- Normal tests: pure UI state, focus/banking, units, routing scope, command limits,
  recovery, recording status and bounded-queue semantics. Use deterministic
  synthetic data; run focused tests during each slice.
- Run the full normal production suite when shared models, rendering, concurrency,
  routing/persistence or safety behavior changes. Preserve the offline audio
  regression contracts when adding adapters. GPU/display tests supplement headless
  tests and must identify the actual backend used.
- Visual checks: readable labels and values, font fallback, edge alignment,
  keyboard focus, non-colour state cues, clipping and larger-text mode at full HD.
- Graph checks: calibrated tones, silence, known stereo correlation and bounded
  history; distinguish sample peak, true peak and reduction rather than relabelling
  one meter. Record FFT/window/binning and units with spectral fixtures.
- Opt-in hardware/performance work: full-HD presentation, controller behavior,
  combined UI/analysis/FX load, thermal state, HDMI reconnect and node failure.
  Two-node load or interruption work requires the private ledger reservation.
- Retain a concise report, exact revisions, reproducible commands and a small
  representative screenshot/trace set. Keep private sessions and generated media
  ignored. Remove disposable prototypes/build caches after useful evidence is
  retained; preserve source recordings and prior unique research evidence.

No runtime test, benchmark, render, MIDI operation or peer dispatch is part of
writing this plan. Documentation validation checks links and patch consistency.

Planning checkpoint: local documentation links and whitespace checks passed;
both archived drafts match their preceding committed bytes. The updated
architecture SVG parses and its revised card labels fit their available width.
Production, historical-media and hardware test classes were intentionally skipped
because this checkpoint changes documentation only.

## 9. Using the other Pi during future implementation

Follow `/home/shome/p/AGENTS.md` and [the node lab](NODE_LAB.md). The installed
`/home/shome/.local/bin/gigpies-peer` launches a new bounded Codex session over
pinned SSH and returns its final reply. It defaults to read-only and 600 seconds;
an assigned write task uses an isolated checkout and the helper's write mode.
Existing interactive sessions remain independent.

This is external worker dispatch, separate from the chat's built-in subagent list.
The private Git ledger supplies task state:
`queued -> running -> ready-for-review -> accepted`, with failed/cancelled outcomes.
There is no graphical Kanban board, persistent dispatcher or automatic wake-up of
an existing interactive session. A live coordinator launches work and reviews it.

Useful division once authorized:

| Coordinator / integration owner | Bounded peer work after prerequisites exist |
|---|---|
| Command model, owning plan and integration review | Read-only contract review and missing-case report |
| GPU renderer and target-display measurements | Synthetic fixtures / headless tests in explicitly assigned files |
| Final module adapter and merged behavior | Portability checks at an exact source commit |
| Shared resource reservations | Agreed peer side of a declared two-node experiment |

Assign a task ID, base commit, owned paths/resources, deliverable, allowed commands,
validation and stopping rules. One receiver runs at a time per node; its lock does
not reserve hardware or other interactive work. Pin revisions and patch hashes,
inspect partial results after timeout, and explicitly accept reviewed evidence.
Peer checks on Pi 4 do not establish the chosen Brain's graphics performance.
Keep operational prompts, logs and mutable task states in the private ledger;
update this plan with accepted milestones instead of creating a second task queue.

## References

- [winit](https://docs.rs/winit/latest/winit/): window/input events and Linux backends.
- [wgpu](https://docs.rs/wgpu/latest/wgpu/): native graphics and backend capabilities.
- [egui GPU integration](https://docs.rs/egui-wgpu/latest/egui_wgpu/): custom painting alternative.
- [Ratatui backends](https://ratatui.rs/concepts/backends/): terminal recovery interface.

Library versions/features will be selected and pinned during C1/C2 against the
actual target. These references support the proposed stack, not a performance claim.
