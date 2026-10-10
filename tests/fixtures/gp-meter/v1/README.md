# GP-METER:1 provider corpus

Produced by GigPies `tests/metering.rs::freeze_provider_corpus`, with deterministic
finite source windows and a controlled acquisition clock. `manifest.json` names
the producer base revision, uncommitted source witnesses and exact SHA-256 files.
No committed revision is invented for this local implementation slice.

`valid-*` includes measured silence, DC, sine, nonfinite tap invalidity, positive
dBFS, repeated/next windows and expired age. `unavailable-*` covers the bounded
provider refusal reasons. `reject-*` must fail strict structural/numeric parsing.
Identity, query correlation, regressing/overlapping frames, repeated-window expiry,
map changes and total deadline are exercised by the owner tests, not claimed from
schema decoding. `schema.json` specifies structure; Rust validators additionally
check canonical u64 ranges, ordered inventory and numeric/frame relationships.

Consumers copy this complete corpus byte-for-byte. Updating it requires explicit
producer export followed by independent consumer acceptance; normal tests do not
rewrite fixtures. See the sole GP-METER card in `docs/MODULE_IMPLEMENTATION_PLAN.md`.
