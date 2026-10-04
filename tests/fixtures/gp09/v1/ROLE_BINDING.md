# Injected role binding and live leases (GP-09)

Implemented descriptor/OS-lock plane; hardware enumeration/open, actual windows,
MIDI input/LED output and dual-device acceptance remain GP-H2. This provider never
searches for devices. The caller supplies a verified, bounded inventory. Stable
controller identity and profile must match exactly; missing/duplicate serials and
duplicate EDIDs require a unique explicit operator label. Enumeration order and
port numbers confer no identity. Injected descriptors are assertions from the
caller, not physical verification by this executable.

## Independent-process artifact

`gigpies-role-lease ABSOLUTE_PRIVATE_DIRECTORY` is the version 1 provider executable.
Desk and Lightdesk each spawn their own child with private stdin/stdout pipes; no
GigPies library, sibling path dependency or copied role algorithm is required.
Consumers exchange the exact JSON data described here. Fixtures are under
`tests/fixtures/gp09/v1`. There is no device, service or socket startup.

The first newline-delimited JSON object is `gigpies-role-acquire`, version 1,
with `expected_generation` (canonical decimal-string u64), `binding` (C-ROLE:1),
and `inventory`. Displays contain `connector`, `edid`, `operator_label`; controllers
contain stable `identity`, optional `serial`, exact `profile`, `operator_label`.
Every field is required, including nullable fields. At most 16 descriptors of each
kind are accepted; IDs follow C-ROLE's 64-byte identifier rules. Labels contain
1–128 bytes and no control characters. Duplicate connector/controller IDs cannot
be disambiguated by labels. Duplicate EDIDs or serial-less controllers require
unique labels matching the persisted binding. This is an injected descriptor
plane; callers must not derive identity from temporary enumeration order.

A successful acquisition returns `gigpies-role-lease`, version 1, `generation`
and exact `binding`. It holds connector and controller locks for this child’s
lifetime. This generation identifies the acquisition; another role's acquisition
or release does not invalidate it. Saved intent is C-ROLE:1 `registry.json`, whose
global generation serializes updates. At most two roles may be assigned.

Subsequent objects contain `format: "gigpies-role-command"`, `version: 1`,
`operation`, `generation`, and exact `binding`:

- `verify`: generation must equal the live acquisition generation. Returns
  `gigpies-role-verified`, version 1, `lease` (the original grant) and
  `registry_generation` (the current global CAS generation).
- `release`: exact live generation/binding; terminates the child and drops its
  locks while retaining saved intent. Returns `gigpies-role-released`, version 1,
  and the live `generation`. EOF and ordinary process death also retain intent.
- `forget`: exact binding and current **registry** generation; removes saved intent,
  durably increments global generation, then drops the locks. Returns
  `gigpies-role-released`, version 1, with the resulting registry `generation`.
  Explicit reassignment is forget then acquire with the resulting generation.
  An intervening update refuses the CAS; no overlapping handoff is granted.

Errors emit a diagnostic on stderr and exit nonzero, dropping live locks. No JSON
error/grant is emitted on refusal. Every line is at most 65536 bytes including
newline; shared strict decoding rejects duplicate/unknown fields, depth above 12,
invalid UTF-8 and noncanonical counters. Complete-line/inactivity deadline is 2000
ms. Consumers should verify every 500 ms, allow at most 1000 ms for the reply and
stop authority on any timeout, EOF, mismatch, malformed reply or child exit. Replies
are below PIPE_BUF; stdout is nonblocking with a 2000 ms poll deadline. Consumers
must drain stdout and keep pipes private. Linux parent-death SIGTERM also releases
the child when its spawning surface dies, even if a descendant inherited a pipe.
The consumer must stop input/LED/render authority before dropping/releasing its
lease; it must not keep operating from the last received grant. A JSON grant alone
has no authority. Reconnect requires a fresh process, inventory/profile verification
and a new generation. Keyboard-only read-only UI can remain available after loss.

## Private storage and recovery

Caller creates the registry directory with mode 0700, owned by effective UID;
files are 0600, same owner, regular, single-link. Absolute paths are traversed with
no-follow openat for every component; a pinned directory fd anchors all subsequent
operations. Files are opened no-follow/nonblocking, so symlinks, FIFOs and devices
are refused. No physical endpoint is opened. Generation/identity/profile validation
and both nonblocking exclusive device flocks precede any grant. The registry mutex
is held only for bounded low-rate updates; it is not a shared input/LED queue.
Locks are CLOEXEC and never unlinked by production code.

`initialized` records registry/device lock device+inode identities and SHA-256
pins of the exact selected display/controller descriptors (including serial/profile).
A changed serial cannot reclaim an existing stable ID. First adoption of an existing
C-ROLE:1 registry, only before any native creation marker exists, verifies the
binding against the injected inventory and establishes
these descriptor pins; later reclaim requires exact pins. Explicit forget allows a
new descriptor assignment; it retains historical lock inode records. Replacement
lock inodes, lost/corrupt registries and invalid permissions fail closed; a saved
assignment cannot become an unlocked live claim. This map retains at most 65 lock
files; it is not an unbounded device history. An exclusive `native.created` provenance marker is fsynced before the first
native metadata write. Missing map with existing marker, missing marker with
existing map, or corrupt marker fails closed; lost metadata never becomes first
adoption. Atomic same-directory rename and file/directory fsync persist the map
and registry before the grant. Interrupted initial
creation can leave an initialized but missing registry and therefore requires
operator recovery; it never grants from guessed state. Private scratch leftovers
fail closed and are not silently deleted. A failed durable write may retain newer
intent; callers must reread the exact registry before retrying and never assume a
failed request rolled back filesystem side effects.

Two cooperating surfaces under one UID are the supported trust boundary. This is
not protection against a hostile same-UID process that can rewrite all private
files, ptrace a surface or falsify descriptor assertions. No expiry or crash erases
intent. Crash reclaim uses the saved exact binding and verified profile, obtains
both now-free locks, increments generation and persists before granting. E02
conflict and stale CAS preserve registry bytes. Disjoint live audio and lighting
leases remain valid independently.

## Validation commands

Follow the parent-held nonblocking reservation procedure in
[one build slot per host](PARALLEL_WORK_PLAN.md#one-build-slot-per-host).
From this checkout, use the following commands inside that reserved parent shell
with `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1`:

```sh
cargo +1.97.1 fmt --all -- --check
cargo +1.97.1 test --locked -j1 --test gp09_native
cargo +1.97.1 test --locked -j1 --all-targets
cargo +1.97.1 clippy --locked -j1 --all-targets -- -D warnings
cargo +1.97.1 build --locked -j1 --release --bin gigpies-role-lease
```

The normal regression corpus uses invented descriptors, temporary private storage
and independent provider children. It covers simultaneous roles, conflict, stale
forget, retained intent, kill/EOF, generation reclaim, ambiguity, wrong profile,
symlinks, hardlinks, bounds, replacement locks and idle expiry. Hardware, private
media, historical exhaustive studies and shared-load tests are intentionally outside
this task. Provider artifact acceptance and consumer integration belong to root.
