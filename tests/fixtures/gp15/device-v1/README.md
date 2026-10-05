# GP15-device:1 producer fixtures

These payloads are produced by `HostAuthority` using real authority grants,
source-boundary configuration-intent commits, and the production `BrainHost`
fake-device adapter. `pending` and `accepted_intent` do not mean that the device
has applied a configuration. `applied_device` follows exact configuration and
fresh duplex epoch/map readback; `failed_device` here follows a real
`CapabilityMismatch` from device preparation. Every fixture passes the existing
integer-only remote decoder. Physical verification remains false.

The normal hardware-host unit suite regenerates each payload in memory and
compares it with these frozen files. Explicit regeneration:

```sh
flock -n /home/shome/p/.gigpies-build.lock env CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 UPDATE_BRAIN_DEVICE_FIXTURES=1 cargo +1.97.1 test --locked -j1 --features hardware-host --lib remote::brain_audio::device_fixture_tests
```

Review fixture changes before accepting a contract change. Meter values use
normalized full-scale billionths (`*_peak_nano`); ratio and signed oscillator
estimate use parts per billion (`ratio_ppb`, `skew_ppb`). Queue/filter delays use
nominal microseconds. Unknown physical mapping uncertainty is null, and physical
clock lock is never inferred from the software estimate.

Device status includes `capture_queue_dropped`, an unsigned64-bit cumulative
count of microphone blocks refused by the bounded capture queue. The runtime and
fixture producer call the same telemetry helper; consumers must explicitly admit
this integer field while preserving strict rejection of unknown fields. It changes
health observation only, not authority, device configuration or readiness.
