# Development and local review

Use Rust 1.97.1 and edition 2024. The crate builds independently with pinned provider codecs and optional native
graphics dependencies; Cargo.lock is part of the project.
Use the normal target directory and `CARGO_INCREMENTAL=0` for agent builds.

```sh
flock -xn /home/shome/p/.gigpies-build.lock cargo +1.97.1 fmt --check
flock -xn /home/shome/p/.gigpies-build.lock env CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 cargo +1.97.1 test --locked -j1 --all-targets
flock -xn /home/shome/p/.gigpies-build.lock env CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 cargo +1.97.1 clippy --locked -j1 --all-targets -- -D warnings
flock -xn /home/shome/p/.gigpies-build.lock env CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 cargo +1.97.1 build --locked -j1 --release
target/release/shr-desk gallery artifacts/screens
target/release/shr-desk simulate
```

The normal suite protects selection, MIDI press/release/pressure/channel routing,
profile ambiguity, absolute pickup, relative mode interpretation, LED byte bounds,
command capacity, retries/conflicts, reconnect, manual/auto holds and scene geometry.
Run the complete normal suite after rendering or state changes. All fixtures are
synthetic. No normal test opens audio, MIDI, DMX, a window or TCP. Focused normal client tests use bounded owned private Unix sockets with synthetic frames and join their own workers.

Visual review is opt-in via `gallery`; retain only a small useful set of SVG/PNG
screens and this result summary. No historical/exhaustive/long-running tests exist
in the initial crate. Actual MIDI, HDMI/GPU, combined-load and disconnect trials
are separate explicit hardware sessions. Their absence is not hidden by a
headless pass. Source/graph accuracy tests must arrive with the real analysis
adapter; the current graphs are illustrative display fixtures.

`simulate` is a line-oriented review shell, not a recovery TUI. It supports
injected complete MIDI messages (hex bytes), synthetic proposals and confirmed
state transitions. Its one pending command, bounded retry cache and absolute
rotary pickup exercise intended rules, but it has no schema parser, scope lease,
authenticator, concurrency, durable state or network transport. Numeric parsing
rejects nonfinite/out-of-range gain. EOF/quit discards the simulation.

## Fonts and artifacts

The source contains the unmodified decompressed Debian PSF2
`Uni2-TerminusBold24x12.psf`, originally installed under `/usr/share/consolefonts/`.
Its Unicode table maps codepoints to bitmaps; do not assume glyph index equals
codepoint. Missing glyphs use `?`. Read [THIRD_PARTY.md](../THIRD_PARTY.md) and
retain the OFL text when redistributing the font. No host font/palette was changed.

The SVG backend emits actual bitmap glyph paths at integer coordinates, so its
font does not depend on browser installation. The native texture presentation uses the same font data through CPU scene
rasterization. A GPU glyph atlas is a later performance optimization. Normal layout tests check viewport fit and fixture glyph coverage;
physical readability, Unicode shaping/fallback and HiDPI behavior still require
separate acceptance.

Generated previews belong in ignored `artifacts/screens/`. Manufacturer PDFs and
screenshots were temporary read-only research and are not vendored. The useful
record is [the source-linked study](CONSOLE_STUDY.md). No third-party recordings,
private session files, downloads or generated audio belong in this repository.

## Publication boundary

Reviewed source publication is authorized. The independent versioned
[publication policy](PUBLICATION.md) defines complete-index and outgoing-history
checks, hooks and the exact font/licence exception. Enable the hooks after checking
existing configuration, inspect named staged content and run the publication guard.
New reusable scripts require an explicit policy entry and review. Provider binaries,
private roles, generated renders and worker evidence stay outside Git. Source pushes
do not imply a binary release, installer, service deployment or hardware acceptance.

Keep local artifact directories ignored and inspect live Git state before staging.
No `git add .`, broad cleanup or reset of sibling work. Shared contracts integrate
through exact versions/revisions, not path dependencies. The complete normal
production suite is required before release; hardware verification must still be
reported separately.


DS04 normal coverage includes accepted GP03 and preview/renew corpus replay,
typed transactional validation, lease/retry/counter/epoch recovery and adversarial
private Unix CLI tests. Actual service integration is a separate bounded synthetic
process check using the accepted binary/provenance in the ignored provider path.
Private exact commands, hashes and child cleanup evidence are retained in task0008
pass3/P4-A handoff. Historical/exhaustive/long, hardware/load/native GPU and remote
production endpoint classes were intentionally skipped.


## Task0009 native software checks

Read [NATIVE_FRONTEND.md](NATIVE_FRONTEND.md). The optional native backend must
compile and pass its feature suite, Clippy with warnings denied and release build.
Normal tests remain device-free: only explicit private UDS fixtures are opened.
Offscreen WGPU execution additionally requires the exact CPU lavapipe ICD in the
process environment, with display environment removed. Actual provider integration
is opt-in against an independently accepted executable; no provider binary enters
public source. Cargo commands use the documented parent-held nonblocking build lock,
Rust +1.97.1, --locked -j1, CARGO_INCREMENTAL=0 and CARGO_BUILD_JOBS=1. Initial new
lock resolution without --locked was separately accepted by root.

## Task0014 continuation validation and handoff

The canonical Pi4 Desk worker makes scoped local handoff commits only. The root
coordinator independently reviews exact staged content and owns upstream pushes,
CI and both-node source synchronization; no publication is implicit in a worker
commit. Preserve the earlier partial handoff and all producer corpora.

Run the complete default and native suites after shared-state, routing, remote or
renderer changes. Serial test execution (`-- --test-threads=1`) is permitted on the
Pi4 while retaining every production deadline assertion. Record a parallel-run
failure rather than increasing a deadline to make it pass. Native checks explicitly
select `/usr/share/vulkan/icd.d/lvp_icd.json`, after inspection, with `DISPLAY` and
`WAYLAND_DISPLAY` unset. This is CPU-headless validation, not a window/GPU test.

All Cargo/fmt/Clippy/release invocations use the parent-held nonblocking shared
build lock and `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`; Cargo tests, Clippy and builds
use Rust +1.97.1, `--locked -j1`. Complete both warnings-denied Clippy feature sets
and both release builds, plus formatting, Python publication tests, document links,
complete-index and outgoing-history guards. Actual provider drivers remain explicit
opt-ins under the coordinator's finite private-network reservation, with frozen
source/executable hashes, bounded stage diagnostics and separate sample witnesses.
Driver success alone does not prove physical mapping, ADAT lock or audio safety.
