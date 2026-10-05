# Control and observation boundary

Status: live contract proposal, 2026-10-04. GigPies owns the final runtime contract.
`src/model.rs` is a synthetic authority/client demonstration, not a network protocol
and not a new canonical mixer schema. Default simulator/file modes open no socket. Explicit DS04 local mode opens one trusted private Unix endpoint through the accepted C-AUDIO:1 / GP03-rendered:1 contract; see [provider client](PROVIDER_CLIENT.md).

## Integrate yesterday's work

GigPies `docs/AUDIO_TRANSPORT.md` separates GPA1 audio from GPH1/GPC1/GPK1 control.
The control prototype already explores PA epoch, writer identity/lease, revisions,
retry, deduplication and fresh-state recovery. Its writable payload is one test
parameter. The stereo hardware host does not turn it into a multichannel API.

Extend that owned boundary deliberately: do not send Desk's in-process enums as
an undocumented wire format, overload an audio packet, use SSH to adjust live
audio, or treat successful send() as an applied value. Existing C PA/FX/REC ABIs
are engine embedding interfaces, not operator-control services.

## Proposed live messages

| Message | Required information |
|---|---|
| Hello / capabilities | Protocol/schema version; product/module identity; session epoch; known scopes; stable channels/buses/parameters; units/ranges/steps; flags for absent/read-only controls |
| Snapshot | Authoritative revision; routing identities; applied values; automation mode/parameter holds; protection; recorder and module state; relevant source frame |
| Write grant | Writer identity; explicit FOH/monitor/PA scope; lease generation and expiry; authorization independent of client selection |
| Command | Request ID; epoch; writer/lease/scope; stable target + parameter; expected revision; absolute desired value; bounded transaction scope where supported |
| Acknowledgment | Request ID/epoch; accepted/rejected/conflict; applied revision/value and effective source frame when known; explicit rejection reason |
| Telemetry | Producer epoch; monotonic sequence; source-frame identity and tap; acquisition basis; freshness; explicit sample-peak/true-peak/GR/units; bounded payload |
| Proposal | Owner/algorithm version; target parameter; current/candidate/delta; reason/evidence; allowed bounds; validity revision; required ramp; acceptance scope |

Production remote ports/authentication remain unimplemented. The reviewed local client uses bounded strict JSON in four-byte BE Unix frames and sameUID/private permission checks. A dedicated
LAN alone is not authentication. Capabilities must be truthful; absence is not a
healthy zero or an editable default. Units and enum domains are validated at the
engine, not trusted because the UI used a slider. Avoid serializing raw pointers,
platform-sized indices or float NaNs. Transactions must define all-or-nothing
behavior; do not emulate atomic routing with a sequence of unrelated scalar edits.

## Authority, failure and timing

One active writer per scope, with automation constrained by that scope's grants
and parameter holds. Conflicting clients get an explicit refusal. Engine-side
deduplication is bounded and tied to identity; old sequence IDs cannot become
new commands when the cache rolls over. Duplicate request IDs with different
payloads are rejected. Safety/protection controls cannot be bypassed by changing
automation mode.

Discrete actions and parameter streams have different overload rules. Coalesce
replaceable absolute values by target, preserving the final gesture. Never silently
drop mute, recall, arm/start/stop or confirmation. Return busy/overflow and show it.
Queue sizes, timeouts and per-message byte/channel limits are versioned. Initial
prototype capacity is **one pending command** and a 64-response simulator cache;
it is intentionally conservative and not a throughput claim.

Selection, page changes and controller mapping are surface-local. Applied values,
human holds, scenes and recording state are engine-owned. UI settings and engine
scene files remain separate. Importing a layout or learning a CC cannot recall
audio. On disconnect disable writes, discard the queued intents, preserve stale
values visibly and reset held controller gestures. Require a fresh snapshot and
grant before writes resume. A lost ACK does not imply that the command failed.

Live smoothing belongs in the engine at its parameter/block boundary. The
simulation applies values immediately and processes no audio; it cannot establish
click-free control or safe release ramps. Proposed manual/auto mode transitions
freeze current values and require explicit scoped release. Multi-parameter holds,
group permissions and slew behavior must be implemented by the authoritative
service before enabling live edits.

Telemetry is latest-value and may drop superseded frames. Control ACKs are not
telemetry. Meter freshness uses a local receipt deadline; cross-node timestamps
require declared clock relationships. NTP alignment and round-trip measurements
do not prove one-way audio latency. Recorder progress means written/committed
frames as defined by SHR REC, not packets received by the surface.

## Acceptance before live control

1. Agree one schema with GigPies; exact versions and normal fixtures on both ends.
2. Read-only attach shows real channel/capability/state identities and reconnects.
3. Parameter bounds, unknown targets, missing scopes, expired/wrong leases,
   malformed messages and wrong epochs fail without mutating audio state.
4. Lost, duplicate, reordered and delayed requests/ACKs settle to authoritative
   state without replay into a different show. Concurrent writers conflict visibly.
5. UI crash, MIDI loss, GPU loss and telemetry overload preserve local audio and
   recording; repeated recovery cannot recall old settings.
6. Only then accept real fader/pan/mute and engine-owned automation holds, followed
   by monitor routing and protected scene transactions.

The initial simulation tests covered a subset: identity/revision conflicts, deduplication,
bounded pending/cache state, refusal, mode/hold behavior, stale ACK and disconnect
recovery, note/pad edges and pickup. Those simulation tests did not test leases, authentication,
serialization, multiple writers, sockets, engine ramps or real audio continuity.

## Coordinated implementation baseline — 2026-10-04

The [GP-2026-10-04.1 contract decisions](https://github.com/PaolaShultz/gigpies/blob/main/docs/MODULE_CONTRACTS.md)
now fix the first implementation subset, examples, bounds and unresolved gates.
They supersede undecided integration choices in this earlier proposal for that
subset; the simulator remains in-process scaffolding with the limits above.
See [the task plan](GIGPIES_IMPLEMENTATION.md) for provider replacement gates.


The accepted task0008 GP03 codec/session and local client now validate leases,
strict wire data, correlated replies, bounded retries, private Unix transport and
engine-provided preview/ramp metadata; see [provider client](PROVIDER_CLIENT.md).
Task0009 [native frontend](NATIVE_FRONTEND.md) consumes that same provider rather
than the simulator, with explicit injected GP09 leases and offline software
recovery checks. These checks do not establish physical audio continuity,
protection, controller/display performance or remote production authentication.


## Task0014 structural consumer correlation and reviewed configuration

The explicit dynamic session consumes C-AUDIO:2 / GP03-rendered:2,
GP07-processing:3 and GP14-structure:1. Structural mutations share the existing
writer/lease/request sequence and one pending operation. A reply must match that
pending context; otherwise only an exact fingerprint of a previously validated
reply in this connection is ignored. The bounded cache holds 128 reply fingerprints
(two per producer history entry), clears on disconnect and never renews freshness.
Unknown, changed or wrong-context replies fail closed. Cached finals cannot finish
a later command. Snapshot queries remain read-only and independently validated: their wire state is
`snapshot`, with null mutation context and a required bound snapshot. Successful
`final` replies may carry only revision/effective-frame metadata; they invalidate
readback freshness until an explicit query confirms state. The final itself never
substitutes for a missing configuration/map observation.

PA owner JSON stays opaque to Desk DSP semantics. The detached editor supports
numeric/boolean values, enum strings, null, source objects, route arrays, program
bus maps and complete owner-document import. Apply reviews the entire replacement,
including all weighted routes, before submitting through the actual provider.
Owner admission remains authoritative; invalid topology does not become confirmed
state. Output routes use independently advertised physical ports and sources,
including explicit unassigned silence. Configuration and routes require their own
scoped grants and confirmed quiescence; rearm is a separate reviewed command.
Map-generation changes retire the old connection after final delivery, so the
operator explicitly reconnects for fresh readback and reacquires authority.
Physical socket/USB assignments and ADAT lock remain unverified observations.


The frontend retains the result of the last explicit operation independently of
periodic health polling. A pending command remains unconfirmed; a review-ready
result still requires protected confirmation. Grant availability derives only from
the Session's correlated scoped grant/renewal and its conservative expiry, never
from snapshot freshness. A retained historical success does not extend a lease or
survive as write authority after disconnect. Frontend updates expose both the
operation result and health/freshness, and reconnect clears the operation report.

Paired raw/structural refresh uses one absolute 250 ms deadline established before
both sends and checked again after decoding, plus the existing 64-document bound.
A complete late decode cannot make that refresh successful. Any failed pair clears
the structural receipt, so a later raw-only poll cannot restore structural freshness.
Immutable page assembly, identity/context checks, revision/topology coherence and
query receipts remain
required; cached mutation replies cannot refresh observations.
