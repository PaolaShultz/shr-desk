# GP-09 v1 injected role corpus

All descriptors are invented; no hardware paths or endpoint discovery are present.
`acquire-a.json` → `grant-a.json` is exact encoded request/grant data.
`process-corpus.json` drives independent provider processes using the exact acquire
and reclaim files: disjoint roles, acquisition-specific verify generations,
kill/reclaim and EOF retain intent. `tests/gp09_native.rs` replays these fixtures
against the actual executable, without copying the provider's role algorithm.
Additional normal tests protect E02 prior generation 7, strict private storage,
replacement inodes, stale forget, ambiguity and 32/33-pair history bounds.

Protocol and lifetime obligations: [ROLE_BINDING.md](https://github.com/PaolaShultz/gigpies/blob/main/docs/ROLE_BINDING.md).

Consumer documentation link adapted for this repository; encoded corpus data is
unchanged. PROVENANCE.json retains the accepted upstream delivery hashes.
