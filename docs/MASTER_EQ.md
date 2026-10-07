# Master EQ setup editor

The real frontend has a dedicated **8-band parametric + 31-band graphic EQ**
editor over SHR PA's version-2 program-input processing. Open it with **F11** in
a dynamic `pa_configuration` session. It requires fresh, matching raw and PA
readback. Other scopes, unsupported owner versions, incomplete EQ data and
ambiguous main-bus mappings are refused.

This is a **muted setup workflow**, not a live EQ transition. Grant PA authority,
review/confirm output mute, wait for quiescence, then open the editor. Apply enters
the existing complete PA configuration review. Confirming does not rearm outputs;
rearm remains a separate reviewed action. No new DSP or wire contract is introduced.

## Controls

| Key | Action |
| --- | --- |
| F11 | Open Master EQ from current PA settings |
| B | Switch parametric/graphic section |
| C | Cycle linked L/R edits, left only, right only |
| U / I | Previous/next parameter; pages follow selection |
| J / K | Adjust selected value or cycle filter type / enable flag |
| F3, value, Enter | Enter an exact value; `bell`, `low_shelf`, `high_shelf`, `true` and `false` are accepted where applicable |
| F4 | Apply draft to the complete protected review |
| Esc | Cancel text entry first, or cancel the draft |

PEQ provides type, frequency, gain, Q and shelf slope for each of eight bands.
Q is used for bells; slope is used for shelves. GEQ uses SHR PA's fixed 31 centres
from 20 Hz through 20 kHz. Each section has its own enable flag. Adjustments are
10 Hz, 0.5 dB or 0.1 Q/slope; exact entry supports finer owner-admitted values.
Gain is bounded to -12..+12 dB, frequency to 20..min(20000, 0.45 × rate) Hz,
Q to 0.1..15.909 and shelf slope to 0.1..1. Disabled sections and unused Q/slope
fields still require valid values, as in the owner. Exact graph/input/EQ member
names, all bands and duplicate-free finite JSON are required. SHR PA's general
rate range is 8..192 kHz; the pinned GigPies module integration admits **48 kHz**,
48..8192-frame owner block capacity and exactly the advertised PA output count.
Final full-graph numeric/resource admission remains the PA owner's responsibility;
Desk does not duplicate routing, compressor or protection validation.

Linked mode only links **new edits to the selected parameter**. It never copies
an entire left/right curve. Existing differences are shown as separate L/R values;
switching sections or channel modes does not change the draft. No imports or
routing edits are accepted through this focused editor; F9 retains the separate
general PA configuration editor.

## Signal path and preservation

Main left/right are identified by their actual `program_buses` values 0 and 1,
not by the first two PA inputs. Each must appear exactly once. Monitor-fed PA
inputs, routing, sums, crossovers, protection and every unedited owner field are
preserved. EQ acts before the PA graph and output protection. Physical outputs
patched directly to raw main or monitor buses bypass this PA EQ.

Channel four-band EQ/compression is separate and unchanged. Existing independent
monitor sends use raw input after shared mute; selectable pre/post aux taps and
a dedicated sends overview are separate follow-up work.

Focus/reconnect retains detached content but invalidates its context. This editor
refuses Apply after show, epoch, revision or generation changes: cancel and reopen
against current settings. A changed configuration or bus map is also refused even if its revision label
was reused. Apply additionally requires a current PA scope grant. Before editing
and serializing, the complete stereo EQ is validated and all non-EQ owner settings
are compared with the original readback. A failed edit never partially changes one
stereo side.
Presentation and unit tests use explicit altered fixtures, not claims of physical
audio or of a live producer transaction. Hardware listening and hot EQ updates
remain unverified/unimplemented respectively.

## Package A owner review

Reviewed against SHR PA `401510bbe1d256883a29c74c494aadc28ba53f33`
(`graph::GraphConfig`, `config::Config::validate`) and GigPies
`0edb95f94acb4f7d89f9edec42d8f86a204a8a2e` (`module_graph::prepare_pa_change`,
`local_audio` structural admission). GigPies maps program buses 0/1 to the wet-return
inclusive main L/R sum; indices 2 onward identify monitors. Duplicate main references
are ambiguous and refused. The editor does not reinterpret physical socket indices.

Complete review, original revision/generation pinning, paired readback at stage and
confirmation, scoped lease admission and quiescence remain in the existing GP14 path.
Review pages must actually be presented before confirmation. Reconnect discards
review/queued intent and attaches read-only; detached EQ content requires cancel and
reopen. A PA apply queues no output rearm: X remains a separate reviewed action.

The regressions derive altered inputs from retained producer fixtures and exercise
owner filter boundaries, descriptor/rate/block refusal, unchanged-draft invalid
JSON refusal, stereo atomicity, preservation, authority/freshness/context loss and
no replay/rearm. These are offline checks. A current actual-provider EQ transaction,
physical mappings, listening, clock lock and hardware output remain integration gates.
Response plots and additional controls belong to package D; live transitions require
an accepted package C producer.
