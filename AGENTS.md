# SHR Desk working agreements

Read README.md, docs/STATUS.md, docs/BLUEPRINT.md and the relevant owning document
before changing code. Follow ../AGENTS.md for peer coordination. This is the
GigPies Brain operator surface, not a mixer, automixer, PA, FX or recorder engine.
Keep existing sibling projects read-only without explicit authorization.

Rust 1.97.1, edition 2024, checked-in Cargo.lock; no sibling path dependencies.
Use CARGO_INCREMENTAL=0 and the normal target directory. Check free space before
substantial builds; review below 20 GiB free or above 5 GiB target, preserving
other sessions and unique evidence. Clean only owned disposable artifacts.

State clearly whether behavior is planned, implemented, offline-validated or
hardware-verified. The initial binary is a simulator and SVG layout renderer.
Never infer real engine or device acceptance from fixtures. Commands, confirmed
state and telemetry are different things. UI loss cannot stop audio or recording.
No automatic audio/MIDI/DMX/network/service or host-font changes. Hardware output
requires explicit session scope. UI/controller setup never silently recalls a mix.

The agent selects tests. Fast production, state, contract, recovery, safety and
focused regressions stay default. Run focused checks during development and the
complete normal suite for renderer/shared-state/routing/safety changes. Historical
renders, exhaustive matrices, hardware and long benchmarks are opt-in. Record
commands and intentionally skipped classes. Keep private data and generated
evidence under ignored artifacts/ or user/. Preserve drafts in docs/archive/.

Before committing inspect live Git state and named staged content. Preserve
unrelated edits. No publication is implicit in local development. Follow
docs/DEVELOPMENT.md for the current local-only publication boundary.

Task0014 capacity direction: input, monitor, PA port, physical socket, USB slot,
recorder track and screen-bank counts are independent. Eight-input legacy schemas
remain explicit compatibility contracts, not product ceilings. Successor provider
versions require actual producer fixtures before consumer acceptance. Keep logical
analog numbering contiguous while presenting physical socket and USB identities
separately; physical patch assignment is configurable, with no fixed PA/monitor
socket split. Expected interface maps and physical clock lock remain unverified
until an explicitly authorized session observes the actual rig.
