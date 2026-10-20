# Ratatui instrument console

Run from this repository root so Cargo finds the checksum-verified shared package
in `.cargo/config.toml`. Build with Rust 1.97.1, `CARGO_INCREMENTAL=0`, `--locked`,
one job and `/home/shome/p/.gigpies-build.lock`. No native feature is needed.

```
shr-desk --terminal ENDPOINT SHOW_UUID EPOCH WRITER SCOPE
```

Use the existing native command's explicit provider and role options. Startup is
read-only; no role/grant is implied. All existing provider workers, semantic
actions, exact review context and freshness checks remain the authority boundary.
Ctrl+C closes the console and invalidates local input. Existing page/control keys
are preserved. Resize and focus loss invalidate held/review input. Below 160x45
cells the console shows a recovery message and refuses input; it does not mark a
clipped review as presented. Terminal release-event support is queried. Held
safety controls/confirmations that require release edges are refused on an
unsupported path rather than generating a false release.

Presentation is capped at 10 Hz (100 ms between completed submissions and the
next frame), including graphical knobs, meters and spectrum. Outside fullscreen EQ/RTA, input events do not
force extra frames: changes accumulate until the next presentation. Provider
pumping keeps its independent maximum 40 ms idle wait, and input wakes that wait
immediately. Slow frames skip missed deadlines rather than drawing catch-up frames.
Review pages count as presented only after an actual submission; resizing fences
input and discards old mouse targets until the next frame. Ratatui diffs text cells,
and material images upload once with only changed placements sent thereafter.
The cap reduces scheduled render work; whole-host CPU savings are not yet measured.

The [shared presentation contract](../../gigpies/docs/reference/INSTRUMENT_PANEL.md)
owns the Ratatui controls, font licence/authoring, primary Rio/font proof and
fallback rules. Set `SHR_PANEL_FONT='SHR Panel Mono:0.3'` only with that verified
application-local font configuration. Unset NO_COLOR for the colour presentation.
Ordinary Unicode is the readable fallback. No provider spectrum is synthesized.

The shared source lives in GigPies `src/instrument-panel/`. The local vendor copy
is a generated exact-version distribution, refreshed by GigPies'
`scripts/package_panel.py`; edit the owner source, not this copy. No path
dependency, network registry or publication is required for this checkout.

[GP-PANEL](../../gigpies/docs/development/MODULE_IMPLEMENTATION_PLAN.md#gp-panel--shared-physical-instrument-panel)
owns the complete plan, validation results and remaining acceptance gates. Native
and offscreen compatibility frontends remain available. Display screenshots prove
software terminal rendering, not physical audio, lighting or HDMI qualification.


With `--processing`, F2 opens the dedicated channel equipment page in FOH scope.
It contains working four-band EQ/compressor controls, exact fader/pan targets,
mute/Hold, monitor-send navigation and live owner telemetry. Click a processing
parameter to select its detached field, then edit with existing U/I/J/K/N/P or
numeric keys. F4 opens the full review; a separate Enter confirms. The EQ curve is
calculated settings response; the separate independently selected spectrum sits behind it. Missing engine features are
omitted. See the shared contract's selected-channel section for captures and
current mouse/terminal limits.


The primary operator input is MIDI. The selected-channel screen uses plain
control names and physical state lamps, with no computer-keyboard shortcut strip.
Blue selection and amber draft lamps are separate from settled owner processing
lamps. Keyboard bindings above remain available for development/accessibility.
This presentation change does not qualify a physical MIDI mapping; the owning
[controller plan](CONTROLLER.md) distinguishes proposed channel assignments from
implemented input paths. No MIDI device is opened by a preview.


## Selected Channel material preview

The Channel layout follows eight MiniLab columns: four EQ frequency/gain pairs,
three compressor pairs, and pan/level. Separate full-height EQ, compressor and
output plates retain these boundaries. The EQ plot occupies only columns 1–4.
Raw peak/RMS and actual gain-reduction meters sit above the compressor controls;
raw levels are labelled explicitly, and stale observations leave unlit scales.
Level-meter scales retain dim green/amber/red zones: green below −12 dBFS,
amber from −12 to below −4, and red from −4 to full scale. Bright cached LED
segments show measured levels; coloured tick values align with segment thresholds.
All four meters share 16 segments on rows 4–19, centred labels on row 3 and
readings on row 20. The right-hand OUT meter reads the actual `input-NN:processed`
tap after EQ/compression and before mute/fader/pan; it does not repeat raw input
or estimate output from GR. Gain reduction retains its separate amber dB scale.
These are digital levels,
not a calibrated VU or acoustic meter.
Eight pads occupy the same columns. Their
context menus and secondary 9–16 layer follow the [controller design](CONTROLLER.md#selected-channel-eight-physical-columns).
The positions are a design contract; physical MIDI mappings remain unqualified.
Section names and descriptive title bars are omitted; parameter labels, units,
contextual actions and operational state carry the useful information.

For the qualified local Rio 0.5.28 path, additionally set
`SHR_CHANNEL_MATERIALS=rio-kitty-v1`. This explicitly selects the bounded cached
material experiment on Channel. Other pages retain Ratatui and the existing font
contract. Unset the material marker for the separate readable fallback.

`channel_material` places original shaded plates, rounded pots/buttons, recessed
wells and LED lenses behind ordinary Ratatui text using Rio's Kitty protocol.
There is no screen pixel framebuffer. Assets are authored offline by
`/usr/bin/python3 scripts/build_channel_materials.py` (Pillow); generated Rust
contains their bounded PNG repertoire. Each used asset uploads once per console
session; changes diff placements, and unchanged frames send no material bytes.
Menu exit/minimum-size removes placements, re-entry reuses assets, and normal
shutdown deletes only this renderer's image IDs. A write error exits through the
existing terminal cleanup. The renderer has no provider or MIDI authority.

From GigPies, run the real private software owner and actual terminal:

```sh
/usr/bin/python3 scripts/preview_panel_providers.py desk --channel --materials --cpu-terminal --suffix channel
```

Add `--font-size 16` or `20` for the same 160×45 layout at smaller pixel sizes.
The helper uses an owned Xvfb/Rio pair and removes temporary state on exit. Its
role descriptors and software input are injected; no audio/MIDI device opens.
The regular connected invocation retains the provider and role arguments above.

The opt-in `channel_material_performance` test builds the same Channel renderer
from explicitly synthetic fixture changes, independently of the connected visual
proof. It exercises stable frames, rotating controls and changing GR meters,
counts ANSI plus material bytes, and measures construction/diff/encoding. Run
under the normal build reservation:

```sh
cargo +1.97.1 test --offline --locked -j1 --test frontend channel_material_performance -- --ignored --exact --nocapture
```

For actual Rio presentation, pass that compiled `frontend-*` test executable to
GigPies `scripts/measure_panel_terminal.py --channel-test PATH --workload controls
--cpu-terminal --name channel-controls`; repeat with `meters` and `unchanged`.
`--plain` compares the same layout's colour-font path. A framebuffer observer
acknowledges each unique marker; reported observation includes readback/polling
and excludes physical scanout. The marker changes one row even for stable
workloads. Benchmark data never enters the connected production screen.
Results and current acceptance belong only to GP-PANEL.


Selected Channel spectrum uses the optional local/authenticated-remote
`GP-SPECTRUM:2` observer, with explicit v1 fallback. Its
[owning contract](../../gigpies/docs/reference/INSTRUMENT_PANEL.md#selected-channel-spectrum-contract)
defines tap, digital amplitude normalization, finite payload, freshness and
mixed-version fallback. Desk performs no audio analysis. Actual software proof
and retained visual evidence are owned by the existing
[GP-PANEL card](../../gigpies/docs/development/MODULE_IMPLEMENTATION_PLAN.md#gp-panel--shared-physical-instrument-panel).

Explicit end-to-end test (no devices), after building the canonical GigPies
`spectrum-source` example:

```sh
flock -xn /home/shome/p/.gigpies-build.lock env CARGO_INCREMENTAL=0 GP_SPECTRUM_SOURCE=/home/shome/p/gigpies/target/debug/examples/spectrum-source cargo +1.97.1 test --locked -j1 --test spectrum actual_provider_spectrum_recovery -- --ignored --nocapture
```

The Channel render benchmark additionally accepts `--workload spectrum`; that
workload and the moving meter workload are labelled synthetic test observations.
They measure the actual renderer and terminal transport, never live audio.

## Fullscreen EQ / RTA

With the provider-backed Channel panel visible, F11 opens or closes the fullscreen
EQ/RTA. In fullscreen, `[` and `]` cycle the provider's analysis-source inventory; changing this
selection never changes the channel EQ target. Inputs are raw pre-processing;
main L/R is pre-PA; configured outputs are their actual post-patch playback slots.
The compact header identifies source/tap and the separate EQ target; ∅ marks
missing/stale observations. It is the screen's only text string. While editing,
it instead shows the EQ target, draft marker and selected frequency/gain/Q/entry.
Frame/clock identity stays in the observation and freshness fences, not a panel.

The plot fills the screen below that header, using width minus two border columns
(158 at width 160). Filled spectrum columns dominate. Four coloured points show
EQ frequency/gain, with horizontal nominal Q-width brackets (bandwidth f/Q,
geometrically centred). The selected bracket is heavier; bypassed points are
hollow with dashed brackets. There is no combined response curve, band-number
text, legend, footer, help paragraph or telemetry panel in fullscreen.

Click a point/bracket to select its band; existing E, U/I, J/K, numeric entry,
F4 review, Enter confirmation and Esc cancellation remain available. F11 or the
header exits. Supported size is 60×18 and larger; editing reviews still require
160×45. Outside fullscreen, `[`/`]` remain pan controls; PA F11 remains Master EQ.
Esc exits fullscreen only without a draft.

GP-SPECTRUM:2 retains the 4096-point FFT and publishes native bins at 10 Hz. The
renderer derives logarithmic column coverage from bin frequency boundaries;
more columns do not imply more resolved bands. A v1 fallback is labelled as 64
coarse bands. Spectrum height is fixed −96..0 dBFS; points use −18..+18 dB EQ gain. Frequency
runs logarithmically from 20 Hz to 20 kHz. The scales are fixed, with no level
normalization. Colour stops are dark green −96, green −72, yellow −54, orange −36,
red −18 and brighter red 0 dBFS; colours interpolate between these fixed levels.
Numerical legends are intentionally absent from this visual view. Fullscreen key interaction can redraw before
the ordinary 100 ms presentation deadline; the analysis worker remains at 10 Hz.
Both the private local socket and configured authenticated QUIC connection use
this observer, separate from editing authority. No DSP runs in Desk.

The [shared spectrum contract](../../gigpies/docs/reference/INSTRUMENT_PANEL.md#selected-channel-spectrum-contract)
and [GP-PANEL owning card](../../gigpies/docs/development/MODULE_IMPLEMENTATION_PLAN.md#gp-panel--shared-physical-instrument-panel)
define compatibility, resource bounds, validation and remaining qualification.
