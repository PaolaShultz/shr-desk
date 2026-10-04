# GP03 preview-flow:1 amendment

Actual OfflineEngine encoded requests and typed rendered replies, with synthetic
48-frame input01=0.125 pumps. Includes assist hold, proposal, preview, cancel,
canceled-token refusal, renew (paired scope/remaining, no granted lease), release
pending/final and ramp observations, plus expiry at commit with a renewed live
lease preserving hold/coefficient state. Every response is strict-decoder checked.
The original ../v1/e03-rendered.json bytes remain unchanged.

Normal replay: `cargo +1.97.1 test --locked --test gp03_preview_corpus -j1`.
Explicit regeneration: `cargo +1.97.1 test --locked --test gp03_preview_corpus
regenerate_gp03_preview_corpus -- --ignored --exact` (include `-j1` before `--`).
All Cargo commands require the parent-held nonblocking shared host build lock,
CARGO_BUILD_JOBS=1 and CARGO_INCREMENTAL=0. Generation is one-time opt-in evidence;
normal replay protects current provider/client wire interoperability. Manifest
binds exact producer source, contract and fixture hashes. No hardware/media.
