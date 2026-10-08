# Monitor sends operator surface

The dynamic frontend consumes accepted `GP18-sends:1` and configured
`GP07-processing:4`. Legacy processing version2 remains unchanged; configured
version3 bytes are retained as historical evidence and rejected. Inventory comes
from actual GP03/GP18 capabilities. Input, monitor and channel-bank counts are
independent. There is no fixture-derived product ceiling.

F12 probes GP18 and opens the overview. F1 shows the selected monitor across a
channel bank; F2 shows the selected channel's monitor sends. Arrows select inputs,
PageUp/PageDown select channel banks, and U/I browse monitors. Browsing grants no
authority. O explicitly releases the old attachment lease and reattaches to the
selected monitor with a fresh writer. A fresh read and a NEW G grant are required.
F and V reattach to FOH and PA respectively. Q releases writer authority only;
it never releases parameter holds, changes modes/levels or rearms outputs.

E opens a local tap draft;1/2/3 select exactly `raw_post_mute`,
`processed_pre_fader` and `processed_post_fader`. Raw taps follow shared input
mute; processed taps follow channel EQ and compression and the same shared mute.
Post-fader taps also follow the channel fader, before pan. There is no trim.
S opens exact send-level entry in dB, preserving existing C-AUDIO range/step and
hold/mode/proposal semantics. F4 opens a complete review; every review page must
be rendered/presented before Enter confirms. Esc cancels entry/draft/review.
Immediate fader/knob changes retain their existing behavior. Level and tap edits
have separate reviewed transactions; they are never claimed atomic together.
Channel EQ/compression remains FOH-authorized even when it affects processed sends.

Current send coefficient gain is displayed in dB beside the dB target; this is
a gain conversion, not a measured level. Readable tap labels are Raw / Post-mute,
Processed / Pre-fader and Processed / Post-fader; complete reviews retain their
exact contract spellings. Raw and tap freshness are displayed separately, and
observation ages include elapsed presentation time. Cached rows are explicitly
last-confirmed when their paired observation is stale or unavailable.

The screen separates current level/tap, committed target and transition, local
unsent draft, pending operation and refusal. Fresh confirmed values come from
validated producer readback. GP18 reads drain queued traffic then explicitly query
both GP03 and GP18 under one original250ms/64-document budget, requiring advancing
raw and extension observations and matching identities/revisions/inventories.
Immutable page assembly retains original deadline/hash/order protections. Duplicate,
stale, partial and cached mutation replies do not renew edit freshness.
A correlated applied final remains known if its subsequent readback fails, but
failed or partial readback closes freshness and cannot authorize another edit.
Queued old raw telemetry or a duplicate final permits the required bounded read;
stale unsolicited legacy batches do not gain that continuation. No mutation is
replayed. The frontend reports completed/readback unavailable separately from
refusal and requires fresh readback and a new review for further edits.
Selection, scope, revision, epoch and generation changes revoke queued review
intent. Focus/controller/role loss fences authorization. Reconnect attaches
read-only with a new writer and never replays a draft or automatically grants.

## Provenance and validation boundary

Accepted producer heads: GigPies `42e038a80373fd9317692d5da770b7079d0205d0`,
SHR PA `85ed759499b5223e30a53e2dbea1e6fec03f04f9`.
The copied corrected corpus retains its actual producing revision
`24db18ea15a05c3f35311e37056b5129f918bcb2`; SHA256SUMS is
`348e9d78c4b8ca5dff9b7a4240f3976eaf4e5dcbd64b2cff952966e923c3b6b4`.
Old fixture bytes are unchanged. Consumer regressions distinguish accepted producer
bytes from adversarial/altered test inputs. The opt-in actual frontend driver uses
an explicitly supplied hash-pinned provider/owner witness and bounded synthetic
PCM. Its success/failed trials and final checks belong to the private handoff;
fixtures alone establish no engine, physical routing or listening acceptance.

D3 actual software episodes passed on both Pi4 and an independent Pi5 run of the
same pinned driver/witness: 10 sends captures and 6 master captures per run, clean
explicit host stop and no hardware. Sends, channel EQ/compression and shared
mute/recovery checks use independent PCM lanes and owner state. The master episode
checks exact6.125 dB L-side gain/phase, unchanged owner-mapped R/main/monitor routes,
non-EQ preservation, muted setup, separate rearm and unsent-review reconnect/no replay.
Stationary captures wait 48000 actual frames after relevant signal/PA history changes
under the original run deadline. See [Status](STATUS.md) for remaining final gates.
