# Master EQ operator surface

The real frontend has a dedicated **8-band parametric + 31-band graphic EQ**
editor over SHR PA's version-2 program-input processing. Open it with **F11** in
a dynamic `pa_configuration` session. It requires fresh, matching raw and PA
readback. Other scopes, unsupported owner versions, incomplete EQ data and
ambiguous main-bus mappings are refused.

F11 is the **muted setup workflow**. The separate live editor is described below. Grant PA authority,
review/confirm output mute, wait for quiescence, then open the editor. Apply enters
the existing complete PA configuration review. Confirming does not rearm outputs;
rearm remains a separate reviewed action. The muted editor retains GP14; the optional live editor uses the independently versioned GP18 contract.

## Optional live EQ and calculated response

In dynamic PA scope, L probes `GP18-master-eq:1` and displays availability,
owner/graph/EQ/map identities, current/target and source recovery state. E opens
an editor only from fresh compatible settled GP03/GP18/full-PA readback with an
explicit PA grant. Applying pins the complete bus vector, map revision, owner
instance and graph/EQ generations and submits only two mapped inputs' PEQ/GEQ
settings. Gain, compressor, delay, routing, crossover and protection cannot enter
this patch. The existing full graph replacement still requires quiescence and
separate explicit rearm. Live EQ never rearms outputs. Completed retirement
storage does not permanently prevent another settled edit; provider prepare owns
its retirement. Optional-library absence preserves the working F11 muted editor.

All eight PEQ bands and all31 GEQ centres/gains are visible, with focused exact
controls and linked/independent L/R edits. The response is **calculated EQ
response**, using settings-only normalized coefficients checked against all12
accepted PA goldens at8/48/192kHz, including bypass and above-rate GEQ identity.
It is never a spectrum, room/protection response or audio DSP in Desk. During a
live fade, current and target static curves are separate; neither is the exact
running time-varying transfer. Disabled/above-rate graphic sections are explicitly
marked identity and stored gains stay visible. Exact values stay in the editor
and complete paged review.

The owner corpus retains producing PA revision
`46a03ec3a86333ec596bd19fd97148b077af5955` and GP revision
`06e0485324590a2ccb29897a180afa8be4eacbc5`. Their byte/hash tests are consumer
admission evidence. Actual-provider/sample acceptance and final suite counts are
reported in the private D handoff; physical/listening/clock-lock acceptance remains
separate. Guarded PA/narrow commit is allocation-free as producer-qualified;
surrounding provider control/source orchestration allocates and performs control
I/O. No whole-host hard-real-time or physical deadline claim follows.

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

Channel four-band EQ/compression is separate and unchanged. Monitor sends independently select confirmed GP18 raw/post-mute or processed pre/post-fader taps; see [sends](SENDS.md).

Focus/reconnect retains detached content but invalidates its context. This editor
refuses Apply after show, epoch, revision or generation changes: cancel and reopen
against current settings. A changed configuration or bus map is also refused even if its revision label
was reused. Apply additionally requires a current PA scope grant. Before editing
and serializing, the complete stereo EQ is validated and all non-EQ owner settings
are compared with the original readback. A failed edit never partially changes one
stereo side.
Current fixture previews explicitly identify offline, simulated data. The optional
live consumer uses the accepted producer extension; the separate actual Frontend
driver exercises the reviewed live and muted workflows with the accepted synthetic
provider and PCM witness. Final source-matched results belong to the D handoff.
Physical audio and hardware listening remain unverified.

## Historical Package A owner review

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
no replay/rearm. These are offline checks. At the Package A checkpoint, actual-provider transactions and response plots
remained later gates. Package D implements the calculated response and accepted
optional live transition consumer described above. Physical mappings, listening,
clock lock and hardware output still require separate authorization and evidence.
