# GP03-rendered:1 producer corpus

`e03-rendered.json` is produced by actual OfflineEngine encoded requests and
synthetic rendering, including pending admission, boundary commit, lost-ACK retry,
initial/final coefficient observations and samples at48000/48048/48288.
`manifest.json` records exact producer-source/fixture SHA-256 and contract identity.
GP02 corpus is unchanged; its unavailable decoder is deliberately separate.

Regenerate under the host build lock:
`CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 cargo +1.97.1 test --locked -j1 --test gp03_corpus regenerate_gp03_corpus -- --ignored`.
Normal `replay_actual_producer_corpus` re-executes producer and compares full data.
Generation is ignored one-time evidence; normal arithmetic and retry regressions
are in gp03/gp03_alloc. No recorded media, hardware or historical renderer.
