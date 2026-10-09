> Historical snapshot, retired as an active tracker. Relative links were
> adjusted for relocation; source text and dated evidence retain their original scope.

# Screens and operating workflows

Design, 2026-10-04. Only Mix, Channel and Analysis have implemented offline scene
renderers. Unavailable fields in those drafts are labelled; planned pages below
are not implemented controls. Generate with `cargo run --locked -- gallery artifacts/screens`.

## Full-HD frame

The reviewed first draft uses 1920×1080 with 12×24 Terminus glyphs:

- 48 px persistent header: product/page, FOH or monitor scope, simulation/live,
  automation mode, connection freshness, recorder state and selected channel.
- 888 px body: Mix uses twelve 96 px strip slots, a 192 px main area and a 552 px
  detail area, including gutters. Detail pages keep selection stable.
- 144 px footer: two rotary rows/assignment context, eight pad actions, latest
  result/alert. This intentionally expands yesterday's 96 px footer to make the
  controller visible; it leaves 37 rather than 39 body rows.

The draft shows both actual rows of eight with labels and values; native input
must add pickup arrows and confirmed gesture context. No
hidden reassignment during a held gesture. Twelve strips are a bank size, never
an engine channel limit. The 36-channel fixture exercises three octaves only.

```text
SHR DESK | MIX / FOH | AUTO | LIVE/STALE | REC | SELECTED CH
12 input strips                   | LR/masters | selected channel
identity / ownership              | bus scope  | applied vs proposed
meters / GR / pan                 | monitor   | processing / sends
fader / mute / PFL                | alerts    | reason / bounds
K01..K08 assignments and values / K09..K16 assignments and values
P1..P8 labels + active state / bank + octave / pending/rejected result
```

Charcoal, pale text, cyan selection, amber holds/pending, red faults/mute and
restrained source colours. Selection outlines, HOLD/MUTED labels and stale marks
carry meaning without colour. Avoid decorative motion, dial pictures and repeated
miniature graphs on every strip. Numeric units and target identity stay visible.

## Screen map and feature priorities

P0 means required to call the integrated product a usable manual/assisted desk,
not that the feature already exists. P1 is useful after the first working desk.
P2 is outside the initial small-band scope.

| Page | Operator task and essential contents | Existing basis / missing dependency | Priority |
|---|---|---|---|
| Mix | Bank/select/name channels; fader/pan/mute/PFL; sample peak/clip/GR; main/bus context; hold state | Surface fixtures; missing full live channel/bus API and monitor audition routing | P0 |
| Channel | Input/source identity; trim/HPF/polarity; EQ; gate/compressor; sends; tap; actual/proposed/bounds | Offline GigPies models + owner descriptors; live per-parameter API missing | P0 |
| Sends / monitors | Select named destination; twelve send levels; pre/post tap; ON vs level; bus master and PFL; return to FOH | Engine bus graph and scope authority missing | P0 |
| Routing | Input-to-channel, channel-to-bus, bus-to-output; physical vs logical identity; preview and commit diff | GPA1 groups are not a patch matrix; transaction API missing | P0 |
| Automation | Mode; granted parameters; per-channel holds; intent; proposals/reasons/bounds; scoped accept/release | Offline reports exist; live arbitration and parameter leases missing | P0 |
| Analysis | Spectrum; rolling spectrogram; L/R stereo plot; correlation; selected tap, time and freshness | Synthetic drawing exists; live analysis workers and calibration absent | P1, basic trustworthy meters P0 |
| FX | Send/return paths; rack → effect detail → back; bypass/mute, tempo; degraded return state | SHR FX engines/embedding exist; remote state/control descriptors missing | P0 for existing supported effects |
| Recorder | Ready/armed/running/finalizing/fault; raw/tap identities; written vs queued frames; free capacity/gaps; deliberate stop | SHR REC bounded writer and recovery exist; host control/telemetry adapter needed | P0 status + safe control |
| PA | Always-visible protection/fault summary; setup-only processor page; capability-led controls | SHR PA engine exists; configurable embedding/measurement missing | P0 health, P1 setup integration |
| Show / recovery | Connections, writer scope, scene preview/safes, snapshot freshness, conflicts, rejected commands, recovery | Transport lease pattern exists; full show schema/authentication/atomic recall missing | P0 |
| Doctor / lights | Evidence and proposals, not automatic unreviewed action; cue state and module health | Offline evidence + SHR LUX simulation; adapters and acceptance missing | P1 |
| Controller setup | Device identity, learn one physical control, detect encoding, pad banks, preview LED ownership, reset mapping | Pure decoder/pickup/packet encoder exist; device I/O and learner absent | P0 before hardware use |

Source/preamp gain is not channel trim; one physical source may feed several
channels. Phantom power is an explicit source operation and never a fast Mix knob.
EQ drawing must reflect the actual engine response, not an invented curve. The
first Channel draft labels its curve schematic for that reason.

## Core workflows

**Manual mix:** choose MANUAL with a visible scoped preview; preserve existing
levels. Press a key to select a channel. Adjust the bank or selected fader; use
Channel for EQ/dynamics. Select monitor destination before changing sends. Exit
to FOH without changing audio. PA protection remains active.

**Override Auto:** key selects lead vocal; move its level; display pending then
applied gain and HUMAN HOLD. New automix proposals remain visible without moving
the held fader. Open Release, inspect target/delta/ramp, confirm that channel and
parameter. Reject the commit if context/revision changed. No automatic timeout.

**Prepare a monitor:** open Sends; choose the performer/bus; persistent header
changes to that scope. Bank rotaries now control sends, not FOH gains. Every strip
shows bus name and tap. PFL auditions the proper bus only when monitor routing is
supported. Back always restores the previous FOH selection and assignments.

**Recover a link:** retain last applied values as stale, disable edits, show last
received time and epoch. Do not replace stale meters with reassuring zeroes.
On reconnect obtain capabilities, snapshot and write grant; drop the old pending
queue. Renderer's restart never loads a show. An ACK lost during a disconnect may
mean the engine already applied a change: the fresh snapshot settles that state.

**Recall a scene:** browse without recall, preview changed targets and exclusions,
preserve recorder/protection/phantom safes by policy, then commit one engine
transaction. Scene undo also requires validation against current state.

## Useful later, unnecessary now

Useful P1: DCA/mute-group spill, custom user banks, A/B setting comparisons with
clear scope, virtual soundcheck from authorized recordings, local performer access,
short health history, larger text mode, operator-specific layouts and scene safes.

Defer P2: theatre actor/understudy tracking, per-send Nodal-style processing,
immersive panning, broadcast mix-minus/automated mic ownership, hundreds of channels,
large plugin markets, motorized surface replication, arbitrary macros and scripting,
DAW timeline/clip launch, synthesizer editing and a commercial-console skin library.
They add engine and training cost without being necessary for the small-band goal.

Do not duplicate every reference-console feature. The distinguishing screen is
Automation: humans must see what the automixer wants, what it changed, what they
own, and how to take over. A compact manual desk and understandable overrides take
priority over dense racks of processors.
