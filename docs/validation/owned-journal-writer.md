# Owned journal writer: boundaries and verification

Baseline: `f1d2f8c2`. This change preserves the five record tokens, canonical
payload/frame bytes, PREPARE → ACCEPT → FINALIZE → PUBLICATION barriers, configured
member receipts, retained operation results and independent offline proofs.

## Ownership and trust boundaries

- Each live Engine owns its metadata members' journal descriptors and current
  slot receipts. Appending encodes one frame, writes it to that held descriptor,
  and calls `sync_data`. Creating or admitting a journal during recovery additionally
  synchronizes its directory once, including a leaf left by a failed creation.
  The process-global tail map, FileStamp checks, reopen and append-time scans are
  removed. Mutable SMB upload descriptors retain their distinct verification rules.
- Startup/recovery reads external journal bytes once through the descriptor it
  will retain. Length, checksum, strict JSON, canonical representation, predecessor
  and monotonic sequence checks remain at that boundary. Canonical verification
  is required because independent receipts reconstruct canonical frame hashes.
- Recovery synchronizes the scanned bytes before using historical receipts.
  The invocation retains new receipts after each replayed slot, so checkpoint
  generation uses the entire known history, not just the writer's latest slot.
- `RecoveryMode::PreservePending` retains incomplete bytes for degraded-recovery
  evidence. `CompletePending` repairs the relevant member tail only immediately before an
  actual append after that candidate's preflight; existing receipt replay does not
  repair later slots. A final suffix repair requires fully converged recovery. A torn tail is an explicit writer state, not an empty or fake writer.
- Write/sync failures leave the writer `RecoveryRequired`, immediately block the
  Engine even at the first PREPARE, and return the I/O error. Another append does
  not automatically reopen, rescan or retry. Recovery requires reopening.
- Detached workspace preparation owns a read-only snapshot and retained pool
  locks. Its borrowed planning context contains no Engine or journal capability.
  Wire envelopes are parsed once before workspace lookup; request digests are
  reused after parsing. Live admission and retry-health checks retain their order.

Append callers supply candidate/object hashes from `object_hash`, prepared immutable
objects or validated recovery bindings; predecessor hashes come from verified or
newly encoded frames. The append path trusts that fixed field bound. External
frame scanning and independent receipt decoding retain their runtime size limits.

## Behavior coverage

- Existing canonical phase/frame/hash golden vectors and strict malformed-input
  tests remain.
- New lifecycle tests cover current-slot idempotency and chain continuity,
  injected partial writes and failed synchronization, real `/dev/full` ENOSPC,
  first-PREPARE blocking, stable operation identity after reopening, historical
  receipt/byte preservation, and contrasting recovery modes on the same torn tail.
- A multi-slot replay test compares the accumulated checkpoint input to a fresh
  disk scan after four slot advances, ensuring earlier newly written receipts are
  retained when the writer drops its old current-slot state.
- Detached preparation tests retain the lock-lifetime, no-journal-append, stale
  base, fencing, source ownership, replay and wire-error-priority cases.

## Reproducible syscall measurement

`tools/perf-check/journal-syscalls.py BASELINE_TEST_BINARY CANDIDATE_TEST_BINARY OUTPUT`
traces the unchanged
`journal::scanner_tests::typed_phases_survive_normal_publication_and_recovery`
fixture in both test binaries. The fixture publishes on three members, checks the
four phases, reopens and checks the recovered outcome. The script records raw
strace output, test stdout/stderr, elapsed traced runtime and journal/directory
syscall counts. Traced elapsed time is not an untraced throughput benchmark.

The local executor denies `PTRACE_TRACEME`/`PTRACE_SETOPTIONS`; no local syscall
reduction is claimed. The one-off `journal-measurement.yml` workflow has been
retired after its comparison completed. Its fixed baseline and candidate builds,
raw traces and `owned-journal-syscalls-<candidate SHA>` artifact remain in the
[completed comparison](https://github.com/isarmg/zxs/actions/runs/40264837864).
The measurement script and fixture remain available for explicit reproduction;
ordinary CI and releases do not rebuild this historical baseline. Historical
results do not establish the performance of later revisions.

## Local results

Pinned toolchain: Rust/Cargo 1.99.0. On the implementation above:

- `cargo test -p storage-engine --all-features`: 606 passed, 0 failed, 3 existing
  ignored tests; includes 533 library tests, 73 integration tests and doc tests.
- `cargo clippy -p storage-engine --all-targets --all-features -- -D warnings`: pass.
- `cargo fmt -p storage-engine --check`: pass.
- `cargo check -p storage-engine --lib --no-default-features`: pass.
- Static workflow, architecture, source-governance and script suite: pass.
- The trace parser's descriptor/directory and interrupted-syscall cases: 2 passed.

The baseline library suite was also run before editing: 534 passed, 0 failed,
3 ignored. Removed tests asserted global-cache eviction or mid-run private-file
replacement defense; replacement coverage checks owned-writer failures and the
startup/recovery boundary. No syscall performance result is reported locally.

## Review regressions

Creation and startup share one file-plus-directory durability fence. Feature-gated
opening I/O faults cover failures before file and directory synchronization,
residual leaf discovery, rejected reopen when the directory fence fails, and
successful publication after the fence succeeds. These tests exercise the protocol
and error propagation; they are not physical power-cut experiments.

Additional combinations preserve evidence after a published historical slot when
later pending payload preflight fails, when only a quorum is online, and when
preservation mode encounters a partially written PUBLICATION. Successful completion
of a pending torn slot must return a writable Engine, rather than poisoning it for
having reused an already durable ACCEPT. A real missing-seal read after lock
acquisition exercises incomplete journal admission: restoring the seal requires
reopening before degraded writes, rather than panicking at the internal writer
lookup. The recovery admission boundary still uses the existing quorum availability
decision; its legacy discarded seal-error diagnostics are outside this change.
