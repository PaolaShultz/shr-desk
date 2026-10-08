# SHR Desk

**The human control surface for GigPies.** A full-HD, TUI-style desk on Brain,
operated with an Arturia MiniLab mkII and an ordinary keyboard. Humans can
adjust the automix, hold individual parameters, or run the show manually.
The mixing engine continues to own audio, routing, protection and applied state.

Started 2026-10-04. The default CLI retains the explicit simulator and SVG gallery.
The accepted GP03 local provider client is now shared by the real frontend and
batch operator paths. The optional `native` feature adds a winit/wgpu window;
`--headless` renders the same real-provider scene without a window, and
`--offscreen` checks CPU Vulkan texture presentation/readback. Default/native normal suites, actual GP03/GP09/GP05 integration, CPU offscreen
checks and both release builds pass; [status](docs/STATUS.md) records the evidence
and limits. Physical HDMI/controller/audio acceptance
remains separate. The GP03 raw mixer remains offline and unprotected; meters and analysis remain
unavailable. Optional GP05 read-only health is separate from the raw mixer;
REC/FX writable controls remain unavailable. The explicit dynamic session adds
scoped PA configuration and output-route controls, with task0014 acceptance tracked
in [status](docs/STATUS.md). Attaching is read-only, grants are explicit,
and reconnect discards local intents without recalling a mix.

FOH Mix/Channel includes detached exact **D** fader dB and **C** pan entry.
Enter accepts text locally; F4 requests a complete review, followed by presented
review/Enter confirmation. Fader uses -60.0..+12.0 dB in 0.1 dB steps; signed
integer pan uses -100 left, 0 center and +100 right. See
[native frontend](docs/NATIVE_FRONTEND.md#exact-foh-fader-and-pan) for recovery,
strict entry rules and the opt-in actual synthetic-provider check.

The four-band channel-processing extension uses legacy `GP07-processing:2` or explicitly configured `GP07-processing:4`,
four independent bell frequency/gain/Q/bypass controls and the unchanged compressor.
It adds explicit `--processing` capability
probing and a complete Channel EQ/compressor draft, Apply/review/confirm/Cancel
workflow on the same real provider session. Current validation and remaining gates
are recorded in [status](docs/STATUS.md); physical acceptance remains separate.

See [native frontend](docs/NATIVE_FRONTEND.md) for dependencies, controls,
queue/recovery behavior and software-only validation commands. The explicit
[GP02 file reader and GP03 batch client](docs/PROVIDER_CLIENT.md) remain available.
No default invocation opens devices, sockets or windows.

The dedicated [Master EQ setup editor](docs/MASTER_EQ.md) exposes SHR PA's existing
eight parametric and 31 graphic bands for mapped main L/R inputs. F11 opens it in
a dynamic PA-configuration session. Linked or independent muted setup edits retain the existing mute/review/apply/separate-rearm workflow. The separately probed optional `GP18-master-eq:1` path offers narrow live EQ-only review without rearming outputs. F12 opens the [monitor sends surface](docs/SENDS.md).

Build with the parent-held procedure in [development](docs/DEVELOPMENT.md), then:

```sh
target/release/shr-desk gallery artifacts/screens
# Open artifacts/screens/index.html in a browser to review all three screens.
target/release/shr-desk simulate
```

In the simulator, try `select 11`, `level -3`, `propose 11 -12`, `release`,
`mode manual`, `page channel`, and `render artifacts/channel.svg`.
`help` lists all commands. No state survives exit; no audio is processed.
The gallery is a review artifact, not a web frontend or live GUI.

| Document | Owns |
|---|---|
| [Blueprint](docs/BLUEPRINT.md) | Boundaries, shared structure, manual/automatic operation and delivery sequence |
| [Console study](docs/CONSOLE_STUDY.md) | WING, DiGiCo, A&H, Soundcraft and Yamaha screen observations and choices |
| [Screens](docs/SCREENS.md) | Screen map, full-HD geometry, workflows and scope priorities |
| [Controller](docs/CONTROLLER.md) | Octave selection, sixteen rotaries, eight pads, pickup and LED policy |
| [Integration contract](docs/CONTROL_CONTRACT.md) | Proposed live state/command/telemetry boundary and prototype limits |
| [Status](docs/STATUS.md) | What works, what is unverified and next concrete tasks |
| [Development](docs/DEVELOPMENT.md) | Validation, fonts, artifacts and publication boundaries |

GigPies remains the final product. Sibling references are source ownership
references, not path dependencies. This package builds independently with
Rust 1.97.1, edition 2024, and a locked crate with pinned serde/serde_json for the provider boundary and optional pinned native graphics dependencies.

Code: MIT. Bundled unmodified Terminus Font: SIL OFL 1.1; see [notices](THIRD_PARTY.md).

Task0015 adds explicit `--brain-audio` monitoring/talkback and duplex-device
controls. Atomic scoped lease maintenance, paired raw/Brain readback, exact device
review and held-action safety passed independent review and full Desk validation
(240 default/242 native tests, 12 opt-ins excluded per suite). The same frozen
candidate passed all seven final two-Pi software scenarios, including 16/32/48-input
coexistence and the declared stall/restart cases.
See [status](docs/STATUS.md) for evidence and [native frontend](docs/NATIVE_FRONTEND.md)
for control boundaries. Physical audio remains unactivated.

The [retained audio-console screen library](docs/console-screen-reference/README.md)
contains 12 manufacturer references with per-screen notes and an offline gallery
for future original layouts; it does not change the implemented frontend.
