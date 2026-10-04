# MiniLab controller and keyboard plan

2026-10-04. The related SHR LUX/DAW documentation identifies an **Arturia MiniLab
mkII**, USB `1c75:0289`. No device was opened in this task. This is an evidence-based
starting profile, not a fresh inventory or proof of the active memory's messages.

The [Arturia manual](https://downloads.arturia.net/products/minilab-mkII/manual/minilab-mkii_Manual_1_1_EN.pdf)
describes 25 keys, sixteen rotary encoders, eight pads with two banks, and octave
transposition. SHR FX's interface records physical clicks on rotaries 1 and 9.
The encoders are endless controls, not motorized faders. Learn their actual
absolute/relative encoding; never infer it from appearance or CC number.

## Channel selection by pitch

All piano keys, including black keys and the upper end key, select channels.
No piano note generates sound, toggles mute or executes a scene. With a learned
anchor note C assigned to channel 1:

```text
channel = received_note - anchor_note + 1
bank    = floor((channel - 1) / 12)
slot    = (channel - 1) mod 12

C  C# D  D# E  F  F# G  G# A  A# B
01 02 03 04 05 06 07 08 09 10 11 12
13 14 15 16 17 18 19 20 21 22 23 24   next octave
25 26 27 28 29 30 31 32 33 34 35 36   next octave
```

The MiniLab octave buttons transpose transmitted notes; do not also add a
software bank offset or selection would move twice. The 25th key selects the
first channel of the next octave, not a special command. Out-of-range notes are
ignored. Sparse stable engine IDs resolve through an ordered channel map; the
initial fixture uses sequential IDs only. Reordered user banks must keep the
pitch-to-slot map visible and never silently change the engine ID being edited.

Qualify by MIDI channel and learned message type. Pads and keys may use identical
note numbers on different MIDI channels; ambiguous same-channel assignments are
rejected. Note-off and velocity-zero note-on do not select or execute actions.
Aftertouch does not retrigger a pad. Pad commands use a press edge, not every
nonzero event. Context changes cancel pending confirmation, and reconnect clears
held/modifier states. Do not forward these notes to instruments.

## Sixteen physical rotations

Physical rows are positions 1–8 and 9–16. The main Mix bank spans positions 1–12;
it wraps part-way across the lower row. The footer must draw the actual 2×8 shape
and channel IDs so this compromise is visible. Compare it on hardware with a
future selected-channel-first profile before deciding ergonomics permanently.

| Context | K01–K08 | K09–K12 | K13 | K14 | K15 | K16 |
|---|---|---|---|---|---|---|
| Mix | Channels 1–8 level in visible bank | Channels 9–12 level | Selected channel level | Selected pan | Select bus/scope, confirm before switching sends | Main level, only with explicit capability/grant |
| Sends | Send levels 1–8 to named bus | Sends 9–12 | Selected send | Send pan if supported | Destination browse/confirm | Current bus master |
| Channel: EQ | Four pairs of frequency/gain | Four Q/bandwidth values | Selected level | Pan | Section browse | Focused value/fine adjustment |
| Channel: dynamics | Gate/comp parameters in displayed order | Remaining supported controls | Selected level | Makeup if supported | Section browse | Focused value/fine adjustment |
| FX | Owner-provided named parameters | Owner-provided named parameters | Return level | Wet balance if meaningful | Slot browse | Focused value |
| Analysis | Cursor/range/zoom controls, no sound edits | Time/frequency controls | Tap browse | Freeze display | Panel browse | Focused value |

Unsupported positions show `--` and emit no commands. The initial simulator
implements Mix absolute-value controls K01–K14 only; K15/K16 refuse changes.
EQ, dynamics and FX maps are proposals dependent on descriptors, not hard-coded
DSP assumptions. Never map phantom power, routing, PA protection or recorder stop
to an unguarded rotary.

Learned clicks on K01/K09 can provide Open/Confirm and Back/Cancel, following SHR
FX's established convention. Every operation must remain available through pads
and the ordinary keyboard if those click messages are unavailable. Holding a pad
may provide fine mode only after release/timeout behavior has been verified.

## Eight pads, visible actions and feedback

Base page is deliberately stable. A dedicated menu/picker is allowed for less
frequent actions, but the footer must show what all eight pads will do right now.

| Pad | Base action | State shown |
|---|---|---|
| 1 | Mix / return to FOH view | Cyan when selected; blue when available |
| 2 | Selected Channel | Cyan when selected |
| 3 | Analysis | Cyan when selected |
| 4 | Selected channel mute toggle | Red only when confirmed muted; yellow pending |
| 5 | Hold selected automation parameter | Amber/yellow held; pending distinctly labelled on screen |
| 6 | Open Release preview; confirm separately | Target/delta/ramp/parameter shown before application |
| 7 | Open Auto/Assist/Manual picker | Current mode labelled; selecting it alone changes nothing |
| 8 | Status/menu; access Sends, Routing, FX, REC, PA, Show and setup | Alert colour supplements the current action label |

In a menu, pads choose labelled items and provide Back/Next; in a confirmation
panel, they show Confirm/Cancel. No global mode cycling or mix recall from an
unlabelled tap. Channel selection while a dialog is open cancels or explicitly
retargets it; never apply a preview to a newly selected channel by accident.

The second hardware pad bank initially mirrors the same eight semantic actions
and colours. It is not sixteen physical pads, extra modifier buttons, or a
different user memory. No dependency on a specific User2 preset is established.
Mapping both logical input banks and transient device gestures requires capture
and acceptance. The simulator currently models one fixture input bank; it can
encode feedback for both banks. P6/P7 in the simulator show guidance; the explicit
`release`/`mode` CLI commands confirm them.

SHR LUX and SHR DAW document the mkII transient pad message:

```text
F0 00 20 6B 7F 42 02 00 10 PP CC F7
PP = 70..7F; CC = 00 off, 01 red, 04 green, 05 yellow,
                 10 blue, 11 purple, 14 cyan, 7F white
```

This is a discrete palette, not arbitrary RGB. Source: the device-specific
[firsthand protocol notes](https://github.com/mhugo/sysex#arturia-minilab-mk-ii),
cross-checked against `../shr-lux/docs/notes/0018-analysis-and-pad-preview.md` and
`../shr-daw/docs/CONTROLLER_LED_FEEDBACK.md`. The Arturia manual does not specify
this packet. The implementation only constructs bytes and has no output driver.

Desk must be the sole owner of the physical controller while mixing. SHR LUX
must not run a pad light show simultaneously; show state can inform Desk's
feedback through an adapter. The eventual LED worker uses a latest-state mailbox,
changed-pad coalescing and a bounded rate (initial target ≤20 refresh batches/s).
LED failure does not block input. Feedback follows confirmed state, never raw
input echoes. No hardware memory, mapping, firmware or backlight writes are
required. Transient pad colour restoration needs a documented device policy.

## Pickup, scaling and keyboard fallback

Absolute CC controls use pickup: after bank/page/selection/reconnect or external
value change, ignore motion until it approaches or crosses the applied value.
Show an arrow and target. Relative modes are explicit per profile; the library
supports two's complement, binary offset and signed-bit decoding with bounded
eight-step deltas. The simulator uses absolute values only and rearms
conservatively after each accepted value change. Production relative scaling,
acceleration, fine mode and fader taper need a separate gesture model and tests.
The current linear dB mapping is a simulation convention, not an accepted taper.

Planned keyboard map: F1 Mix, F2 Channel, F3 Sends, F4 Routing, F5 Automation,
F6 Analysis, F7 FX, F8 Recorder, F9 PA, F10 Show; arrows/Tab navigate,
Enter opens/confirms, Esc returns/cancels, +/- edit, M mutes, H holds,
R opens Release, A opens mode picker. Text entry owns keys while focused;
the letter shortcuts must not fire when naming a channel. PFL uses an explicit
momentary action with release-on-focus-loss. The existing line simulator is not
yet this keyboard workflow or the proposed recovery TUI.
