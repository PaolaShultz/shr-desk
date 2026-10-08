# Actual remote FOH stereo FX operator controls

GP21-fx:1 controls one actual Brain owner, ordered `foh-left`, `foh-right`.
GP05 fixed local fallback health keeps its existing read-only meaning. This page
is not a rack or an engine. Desk never implements delay processing.

Use an explicitly authenticated dynamic provider session with `fx_configuration`
permission and scope. **X** explicitly reattaches the existing worker to the FX
scope read-only; a new probe and grant are required. **W** opens and explicitly probes the owner. Attachment
alone sends no GP21 probe and grants no writer. **G** requests the ordinary shared
scope lease; **Q** releases it. Scope switches use the existing session worker.
Old providers and old owner libraries remain unavailable for GP21 writes.

**L/R** selects left/right without changing audio. **E** opens detached exact
numeric entry: `delay_ms feedback damping wet_gain`. Bounds are 1..500 ms,
0..0.85 feedback, 0..0.99 damping coefficient, and 0..1 wet gain. Values must be
finite; fractional values occur only inside the bounded opaque owner JSON. Literal
lowercase text is preserved; command keys normalize before dispatch. **Enter**
requests a complete review, then the ordinary presented-pages/Enter confirmation
applies it. The unselected partner and selected bypass value remain unchanged.

**Space** reviews selected-channel bypass. It settles excitation controls over
20ms and then drains old tails; settlement is not silence. **P** reviews panic
for the selected channel; **B** reviews both. Panic clears history/filter state,
not persistent mute. Every operation shows complete basis, target and partner,
selected mask, ordered binding/lifetime and software lead policy4800 source frames.
**Esc** cancels unsent text/review. A sent mutation is not cancelled or undone by
Esc; producer prepermit authority/session cancellation differs from irrevocable
postpermit unknown. There is no invented operator cancellation wire.

The same AuthorityConnection, Operator, Session, queue, writer/request IDs,
revision and lease own these controls. Reads pair actual-owner and ordinary
snapshots; review and confirmation requery and fence the exact basis. Material
owner state includes session/source/map/capability/instance/library identity,
ordered channels, generation/reset, settled controls and complete target/partner.
Source progress may advance while the reviewed material basis stays identical.
Focus/role/controller/attachment/context loss invalidates intent and review.

Preparing, permitted, applied and settled are separate. Permit advances shared
authorization, not DSP success. A later completion revision may exceed expected+1.
Desk validates original context/ticket/apply frame, exact owner target/generation,
reset and settlement evidence. Panic retains the previous configuration frame;
its reviewed mask is correlated to the producer-validated ticket/apply frame,
reset increment and owner source progress. Public replies do not echo the mask.
Independent samples must verify selected-channel effects.

Unknown/lost-owner/authorization replay outcomes never imply application, even
if historical nested authorization says applied or targets match. No automatic
retry, regrant, replay or reconnect rearm occurs. Further editing is blocked until
an explicit fresh settled owner probe reconciles uncertainty and the operator
starts a new review. Reconciliation reports current actual state; historical
operation outcome remains unknown. Retained evidence is not fresh authorization.

The fixture is pinned to GigPies14504a648ae2494f901b4f25027342ccb1ca614d and
actual SHR FX974ab23753dbe777f5d3401356028b683e4725d0. Normal tests are offline;
the explicitly ignored `fx_actual_frontend` episode requires a separately launched
hash-pinned provider/Brain, explicit readiness, private TLS and independent
streamed wet/PA/raw REC evidence. No physical devices, windows, playback, acoustic
acceptance or hardware clock qualification are implied.
