# C-AUDIO:1 encoded provider corpus, v1

Provider: GigPies GP-02 offline authority harness, GP-2026-10-04.1.
Source baseline99eb0f08050a9a86039af9f1ba2a3541649ca7b6 plus uncommitted
implementation; exact provider source and data hashes are recorded in manifest.json
and the private task0008/A source-manifest.json. Consumers copy reviewed data,
retain byte hashes and provenance, and decode using their own code.

Each scenario starts at show11111111-1111-4111-8111-111111111111 epoch9 revision12.
Request01 grants FOH to desk-a at monotonic0ms. Subsequent request indexN runs at
N−1ms. Lease1 is engine-issued. Initial/final snapshots have sequence1/2.
Encoded requests and responses are exact bytes; snapshots are standalone encoded
Snapshot bodies. Test `versioned_encoded_corpus_replays_real_authority` decodes
and reproduces every request/response and both snapshots using production logic.

E03 request41 changes fader−6000→−3000; exact retry preserves revision13;
request42 refuses−3001 range; request43 expected11 conflicts. Harness target
metadata is applied, effective_frame=null, actual=null, ramp_frames240. The
normative GP-03 DSP expectation is **not yet implemented**: source frame48000,
next48-frame boundary48048, linear amplitude ramp240 frames.

E03R request41 applies13, request43 applies14; uncached late42 expected14 refuses
expired_id; retry41 returns original13 without regressing final snapshot14.
E03M begins input-01 fader−6000, pan0, monitor-1 send0; fader−3000 and mute=true
preserve monitor send0 in metadata. Normative GP-03 coefficients remain pending:
monitor1 gain1, each initial main10^(−6/20)/sqrt(2), changed main10^(−3/20)/sqrt(2),
shared mute envelope to0. This corpus never fabricates measured or rendered audio.

One-time generation is ignored in the normal suite. On-demand command, under
parent-held host build lock with CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0:
`GP02_CORPUS_OUTPUT=/absolute/owned/output cargo +1.97.1 test --locked -j1 --test gp02 regenerate_gp02_corpus -- --ignored`
Output directory must already exist. Retain exact requested files and update
manifest hashes after provider review; normal tests never write fixture files.
