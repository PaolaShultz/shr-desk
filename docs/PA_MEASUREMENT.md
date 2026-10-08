# PA measurement review

GigPies owns bounded capture on a shared source timeline; SHR PA owns measurement
and alignment proposals. Desk presents results and requests transactions. It never
opens a microphone, copies an analysis algorithm or constructs a correction.
The first integration is software-only and requires an explicitly configured
provider. Old providers remain usable without the explicit measurement probe.

Native Y opens/probes the measurement page. The page reports admitted reference
input IDs, reserved microphone capture slots, common source/map/graph identity,
progress and result IDs. Measurement output indices and capture slots are zero
based and are separate from logical input IDs. A reserved microphone cannot join
the program or monitor mix through this workflow.

C opens capture entry: capture ID, logical reference input, reserved mic slot,
PA output index, position ID, sample count, maximum arrival search in samples,
and low/high analysis frequency in Hz. Valid captures contain32768..65536 samples
at48kHz; arrival search is1..2048 samples and frequency bounds20..20000Hz.
Enter prepares the exact review; every material page must be presented before
another Enter confirms. X explicitly requests capture cancellation. R opens a
result ID; every spectral bin and uncertainty/refusal remain scrollable.

P opens proposal entry: proposal ID followed by anchor/target capture ID pairs,
up to eight positions. Capture IDs designate already completed owner results;
they do not select a live microphone or start playback. The owner may return
proposed, no_change or refused. No-change/refusal never creates a PA transaction.

A selects a proposal for the existing muted whole-configuration review. Desk
checks its exact current source/map/graph/configuration/program-bus basis and
verifies that the supplied candidate changes only the owner-declared delay and
polarity fields. Outputs must already be muted/quiesced. The full configuration
is reviewed and applied with GP14; rearm remains an independent explicit action.
A successful capture, proposal or ACK is not an acoustic alignment claim.

Focus/resize/role loss and changed authority invalidate reviews and queued intent.
Detached text and useful result descriptions remain available, but require fresh
observations and a new review. Reconnect is read-only and never replays capture
or applies an old proposal. Synthetic/null acceptance is distinct from future
separately authorized microphone, listening and hardware-output sessions.

The same bounded editor is available without graphics:

```text
shr-desk --measurement-local ENDPOINT SHOW EPOCH WRITER pa_configuration --script FILE
measurement probe
measurement capture ID REFERENCE MIC_SLOT OUTPUT POSITION SAMPLES MAX_LAG LOW_HZ HIGH_HZ
confirm
measurement result ID
measurement propose PROPOSAL ANCHOR TARGET [ANCHOR TARGET ...]
confirm
measurement apply PROPOSAL
confirm
```

Grant and input-release remain explicit script commands. `cancel` discards an
unsent review; `measurement cancel ID` prepares cancellation of an active job and
requires confirmation. `Y`/`measurement probe` explicitly refreshes progress and
results. Repeated requests do not restart captures. The page's stored descriptions
are observations, not a continuously renewed permission to write.
