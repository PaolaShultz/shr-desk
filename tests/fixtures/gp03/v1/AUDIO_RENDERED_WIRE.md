# GP03-rendered:1 offline capability extension

GP-2026-10-04.1 C-AUDIO commands remain byte-for-byte GP02 requests. The pure
GP02 harness/decoder/corpus remains unavailable for rendered state and release
commit. A distinct `GP03-rendered`, capability_version 1 envelope is required;
feeding this extension to the GP02 reply decoder must fail. GPA1/GPC1 unchanged.
This is an offline deterministic pump, with no realtime host threading, devices,
PA protection, autonomous algorithms or physical safety acceptance.

Envelope fields: context, capability, capability_version, state (`pending`, `final` or `backpressure`),
ticket (decimal string or null), effective_frame (decimal string or null),
ramp_frames (240 or null), outcome (unchanged GP02 Reply or null), snapshot
(RenderedSnapshot or null). Pending never carries an applied outcome. Final
rendered commit carries original ticket/frame and a new coherent snapshot.
Exact retries return the original outcome/snapshot, which may be older than a
consumer's presented state. Identity/lease checks still precede retry cache.

RenderedSnapshot contains capability/version, rendered_application=true,
release_commit=true, frame, faulted, protection=`offline-unprotected`, meters=null,
authority (GP02 Snapshot), coefficients (eight stable input IDs). Authority
`actual` fields remain null: target/hold metadata in integer mdb/pan units cannot
represent interpolated linear coefficients. The extension's coefficients are
current_nanogain, ramp_target_nanogain and held_nanogain arrays, in order fader,
pan-left, pan-right, shared mute, send1, send2. Unit is linear amplitude gain ×
1,000,000,000, rounded to nearest integer. Held coefficients describe actual
frozen mix during mode changes; the GP02 hold field remains semantic target
metadata. No fabricated signal meters. Quantization error ≤0.5 nanogain.

Eight mono inputs feed stereo main and two mono monitors. Monitor taps are after
shared mute and before FOH fader/pan. Fader/send use 10^(mdb/20000), including
finite −60000 mdb gain 0.001. Equal-power pan uses angle=(pan+100)π/400; hard
endpoints are exact (1,0)/(0,1). Each changed coefficient ramps linearly from its
actual boundary value over 240 frames, endpoint exact. Arrival at frame48000
commits strictly at48048, before that frame's sample, which uses the old value.
Coefficient observations refer to the next sample's absolute frame.

One pending graph transaction globally, one primitive renderer completion slot.
Transactions have 1..64 unique semantic targets (40 actual available parameters).
Preparation validates all targets and computes all transcendentals outside render.
Pending blocks every authority mutation (including renew/propose/preview), while
snapshots observe committed state. Pending exact retries share a ticket; changed
same-ID requests refuse. Live lease is revalidated before the offline pump renders
the boundary; expiry cancels, preserving authority/mix/holds. Connection loss
alone leaves the mixer running; without a live lease new writes refuse. A fresh
writer must snapshot and obtain its own exclusive scope grant.

Mode transitions freeze actual coefficients at effective boundary, without a
jump; manual set holds its destination. ASSIST proposals and explicit AUTO bounds
remain authority metadata, with no autonomous writer. Preview is bound to scope,
writer/lease/revision and 2000ms. Release commit ramps to exact proposal destination
and removes holds only with the DSP/authority commit. Cancel/stale/expired preview
leaves graph/hold unchanged. Read-only observation sequence and monotonic control
time survive staged commits.

`Mixer::process` uses fixed arrays and Copy prepared/completion values; its Copy
ProcessError does not allocate even on malformed buffers/clock exhaustion. Any
nonfinite source/arithmetic result latches zero output. Clock exhaustion never
wraps. The control adapter's `process` is explicitly outside a live callback and
allocates replies/snapshots. It slices rendering at a boundary, validates authority,
drains completion and commits authority between renderer invocations. A future
host adapter must supply bounded communication/retirement; none is claimed here.

Validation: `cargo +1.97.1 test --locked --test gp03 --test gp03_alloc -j1` under
shared build lock. Numerical tolerance 2e-14 for known arithmetic, exact equality
for partition invariance and target endpoints. Versioned producer-executed corpus
is in tests/fixtures/gp03/v1. Historical renders/benchmarks remain opt-in.

Before authority admission, a full pending slot returns outer `backpressure` with
null outcome/ticket/timing. This is transport flow control, not an authoritative
C-AUDIO busy refusal: it consumes no request ID and can be retried after drain
with a refreshed expected revision. Once admitted, a pending request reserves its
identity; conflicting payload under that ID refuses reused_id. Final failures at
commit (including preview expiry) are authority-processed and retain ordinary
cache/high-water semantics. Rendered final outcomes are bounded at64 per writer,
matching authority cache retention rather than a global FIFO. Expired/retired
writers' extension caches are purged on control receipt. Commit rebuilds the
candidate from current live authority before boundary rendering, validating token
expiry as well as lease and preserving current observation sequence/time.

`context` is a strict typed correlation object with exactly show_id, module, epoch,
writer, lease, request_id and expected_revision (same string/null units and domain
rules as C-AUDIO). Every pending/backpressure reply echoes its identified request;
final context matches its validated C-AUDIO outcome. Snapshot context keeps
show/epoch with null writer/lease/request/revision. Consumers must compare context
to the outstanding request/session before accepting pending or completion frames;
request IDs alone are not globally unique. Final outcome/context mismatches,
unknown fields, unsupported capability/version, invalid gain units, fabricated
meters or incoherent snapshot bindings are refused by RenderedReply::decode.
