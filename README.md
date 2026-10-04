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
REC/PA/FX writable controls remain unavailable. Attaching is read-only, grants are explicit,
and reconnect discards local intents without recalling a mix.

See [native frontend](docs/NATIVE_FRONTEND.md) for dependencies, controls,
queue/recovery behavior and software-only validation commands. The explicit
[GP02 file reader and GP03 batch client](docs/PROVIDER_CLIENT.md) remain available.
No default invocation opens devices, sockets or windows.

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
