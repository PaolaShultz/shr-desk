# C-AUDIO:1 GP-02 wire format

GP-2026-10-04.1, pure offline authority harness. It commits target/hold/proposal/mode
metadata only. `actual`, `effective_frame`, acquisition frame and observation age
are null; snapshots explicitly advertise `rendered_application=false`,
`release_commit=false`, `validity=unavailable`, `durability=volatile`.
GP-03 owns real 48-frame scheduling and 240-frame coefficient ramps. An `applied`
harness response does not claim sound or rendered application. GPA1/GPC1 unchanged.

UTF-8 JSON ≤65536 bytes, depth≤12, no duplicate/unknown fields or trailing content.
Integer values only. IDs follow the common domain grammar; show UUID lowercase.
Epoch/revision/lease/request/sequence counters are canonical decimal u64 strings.
Request keys exactly: `contract="C-AUDIO"`, `version=1`, `show_id`, `module="audio"`,
`epoch`, `writer`, `lease`, `request_id`, `expected_revision`, `kind`, `body`.
Snapshot has null writer/lease/request/expected_revision. Grant has writer and
positive request ID, null lease and expected revision. Others have a live positive
engine lease, writer, positive request ID and expected revision.

| kind | exact body |
|---|---|
| snapshot | `{}` |
| grant | `{ "scope": "foh" | "monitor1" | "monitor2" }` |
| renew, release | `{}` |
| set, propose | `{ "targets": [Edit] }` |
| set_mode | `{ "mode": "manual" | "assist" | "auto", "bounds": [AutoBound] }` |
| preview_release | `{ "targets": [Target] }` |
| cancel_preview, release_preview | `{ "token": domain-id }` |

Edit is `{ "target": Target, "value": integer-or-boolean }`.
Target has `parameter` and `input` (`input-01`..`input-08`). Parameters `fader`,
`pan`, `mute` have no additional fields; `send` additionally has `monitor`
(`monitor-1` or `monitor-2`). Fader/send −60000..12000 mdb in steps100;
pan −100..100 integer; mute boolean. Transactions contain1..64 unique targets,
all in granted scope. FOH owns fader/pan/mute; each monitor owns its send only.

Replies retain the eleven common keys; `kind` is applied/rejected/conflict/busy.
Body keys exactly `revision`, `reason`, `lease_remaining_ms`, `granted_lease`,
`scope`, `preview`, `snapshot`, `effective_frame`, `ramp_frames`, using null when
unavailable. A grant reports engine-issued lease and remaining2000ms. Renew
extends2000ms from injected monotonic time (client renewal guidance500ms).
Expiry/release retires permission, preserving holds and targets. Identity/epoch/
lease validation precedes retry cache. Exact cached retry precedes revision check;
changed reused ID refuses. Uncached ID≤high-water refuses even after cache eviction.
Cache64/session, synchronous one command at a time, ≤4 sessions (three available
exclusive scopes make three the current effective maximum). New session requires
new writer identity; retired writers cannot restart their ID space.

Manual set creates parameter holds and owner. Mode change freezes all scoped target
values into holds. ASSIST proposal changes proposals only; AUTO requires explicit nonempty scoped bounds; MANUAL/ASSIST require empty bounds.
AutoBound is `{ "target": Target, "min": integer, "max": integer }`, with valid
parameter ranges, min≤max, unique targets and no mute targets. Mode changes store
these bounds while preserving human holds. AUTO alone grants no algorithmic writes. No autonomous operation is advertised in GP-02.
Preview requires held non-mute targets with proposals, ties writer/lease/scope/
revision, expires2000ms, and advertises ramp240. Cancel preserves holds.
Release commit explicitly refuses unavailable pending GP-03 mixer timing.

Snapshot fields: show_id, epoch, revision, sequence, page, page_count, durability,
validity, acquisition_frame, age_ms, rendered_application, release_commit, inputs,
monitors, modes (scope/mode pairs), automation_bounds, parameters. Parameter keys: target, actual,
target_value, proposal, hold, owner. Eight mono inputs and two pre-fader monitors
are identities, not hardware capacity acceptance. The harness fits one page;
collector supports≤16 pages, same show/epoch/revision/sequence/count, complete
within2000ms of first local receipt; mixed/duplicate/expired sets discard all.
Meter freshness is a separate250ms local-receipt rule; no meter is fabricated here.

Versioned encoded corpus lives in tests/fixtures/gp02/v1. E03 validates target
metadata at epoch9/revision12 and explicitly separates its GP-03 scheduling
expectation; E03R tests41→43→late42→retry41, E03M preserves send targets across
FOH edits/mute without pretending to render monitor coefficients.

The harness advertises `session_history_capacity=1024` in every snapshot. It
retains every issued writer identity for the epoch and refuses further new grants
at that bound; callers must start a new authority epoch, never reset replay state.
Expired sessions' previews are removed during successful new grant cleanup.
At most four stored previews exist (one per active session); preview-only tokens
cannot release holds. Reply and snapshot decoders validate provider semantics,
not merely JSON syntax. Full page assembly requires exactly40 unique parameters
(5/input); split pages repeat identical inventory, modes and capability metadata.
Grant's `expected_revision` is **nonnull**; only `lease` is null.

The first grant request ID is exactly `"1"`; fresh identities cannot start at a
higher counter. Subsequent requests may skip IDs, but cannot wrap after u64 max.
Consumers retain their latest epoch/revision/sequence when presenting snapshots:
page collection only assembles coherent sets, and a cached older reply does not
replace a newer presented state. New authority instances require a new epoch;
this volatile harness does not persist or generate restart epochs.
