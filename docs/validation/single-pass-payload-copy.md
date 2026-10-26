# Single-pass payload preparation and export

## Scope and boundary

The recorded **local** measurements use production baseline
`3e414b5c81e72fbc55d59fbaf38a77519f9fe586`. Both local builds also use the recovery
test-call correction from `9298855066b014cbe017a8d890e336d678c64cf1` and the identical
new fixture. These overlays do not alter the measured paths; the original local
logs and results retain that baseline identity.

The **remote CI** workflow instead uses the published, fetchable commit
`9552cf36910b6b644d97d149f692e5c6e2cee841`. Its payload preparation, disk copy,
container installation and online-read production files match the local baseline;
the recovery test-call correction is already included. CI overlays only the
payload fixture and its module declaration, without replacing recovery tests.

- Each fresh payload copy computes the existing complete-file hash and checksum
  manifest from the bytes actually copied, compares the version commitments,
  sets the read-only mode and synchronizes the output before publishing its name.
  `VerifiedCopy` privately owns that output descriptor and borrows the exact
  `FileVersion`; installation consumes it and holds the descriptor through the
  move. The duplicate fresh-body readback is removed.
- Existing prepared bodies, already installed targets, startup/recovery inputs
  and published replicas remain disk-input boundaries with actual byte checks.
  An already present target is verified even when a fresh copy is available.
- Online export checks seal, publication proof and checksum-manifest metadata,
  then verifies the body while writing a temporary output. Source failures permit
  replica fallback. Destination, synchronization and cleanup failures stop the
  export and retain their cause. `NOREPLACE`, file sync before rename and parent
  directory sync after rename are retained.
- File-branch selection and merge precondition scans, offline export, wire/disk
  formats, dependency identities and the per-chunk plus complete-file hash
  commitments are unchanged. This change adds no verified-content cache.

## Behavior checks

New cases cover fresh installation retaining the ordinary single inode, rejection
of an existing corrupt target despite a fresh verified source, external resumed
body corruption, source growth after preparation planning, corrupt/truncated/
oversized replica fallback, corrupt manifest and missing seal fallback, all-bad
replicas leaving neither final output nor temporary copy, and destination
no-overwrite preserving the original error and bytes. Existing preparation,
recovery, publication and export commitment tests continue to run.

`cargo test --offline --locked -p storage-engine`: **622 passed**, 0 failed,
6 ignored (including the two explicitly invoked measurement fixtures).
The initial implementation passed all 19 performance-tool Python tests, including
four trace-parser cases, plus workflow and architecture checks. The workflow-only
provenance follow-up adds four passing lightweight tests; it does not rerun the
Rust build or measurement. Changed Rust files pass rustfmt;
`git diff --check` passes. Production `cargo check --lib --no-default-features`
and `cargo clippy --all-targets --all-features -- -D warnings` pass. The initial
clippy attempt hit executor ENOSPC; the completed rerun used one build job and
`CARGO_INCREMENTAL=0`. ENOSPC, file-sync and parent-directory-sync failure
propagation were reviewed in source; no new injected failure tests dynamically
exercise those paths. No physical power-cut or disk-fault claim is made.

## Same-fixture local measurements

Rust/Cargo 1.99.0, unoptimized test profile, Linux cloud overlay filesystem,
64 MiB payload, three alternating baseline/candidate rounds. Setup and final
integrity checks are outside the measured operation. Preparation includes all
three routed copies and their installations; export includes the complete public
`Engine::export_version` call. These are page-cache/cloud-filesystem results,
not HDD throughput claims.

| Complete operation | Baseline `rchar` | Candidate `rchar` | Baseline median | Candidate median |
| --- | ---: | ---: | ---: | ---: |
| Fresh prepare, 3 replicas | 402,654,189 B | 201,327,597 B | 397.35 ms | 214.45 ms |
| Online export | 134,239,247 B | 67,130,383 B | 140.65 ms | 115.33 ms |

The read-counter differences are exactly three payloads (192 MiB) and one payload
(64 MiB), respectively. `rchar` includes metadata and a small sampling overhead;
it measures bytes returned by reads, not physical device traffic. `wchar` is
unchanged: 201,353,946 B for prepare and 67,108,928 B for export. Median elapsed
time decreased 46.0% and 18.0% in this run.

Median process peak RSS, including setup: prepare 20,244 → 20,764 KiB; export
21,200 → 21,376 KiB. This is a process-lifetime high-water mark rather than an
isolated-operation peak. The fixture streams its input and the implementation
continues to buffer one checksum chunk, not the whole body.

## Reproduction and syscall evidence

The two ignored tests in
`test/engine/unit/engine/payload_measurement.rs` are installed byte-identically
into both source trees. Run:

```sh
python3 tools/perf-check/payload-syscalls.py BASELINE_TEST_BINARY CANDIDATE_TEST_BINARY OUTPUT --no-trace
```

The output keeps each round's log and `summary.json`; `--rounds` adjusts the
sample count and `ZXS_PAYLOAD_MEASUREMENT_BYTES` selects the common payload size.
The test samples `/proc/self/io`, `/proc/self/status` and `Instant` only within
its own code; production has no measurement hooks or counters.

Local ptrace is unavailable. Exact syscall evidence remains pending the dedicated
`payload-measurement.yml` workflow on `verify/payload-copy-*`. A job-level ref
guard applies equally to pushes and manual dispatch, excluding main and other
branches.
The workflow checks successful payload reads of **6 → 3 passes** for preparation
and **2 → 1** for export, with unchanged **3 / 1 write passes**. It also retains
raw fsync/rename syscall counts and traces in
`payload-copy-comparison-<candidate SHA>`. The tracer resolves ordinary body,
staged-body and temporary-output descriptors and rejects incomplete marker pairs;
its parser is independently tested. Before measurement, the artifact also records
`provenance.json`: actual baseline/candidate commits, the payload fixture hash
in each source tree, Rust/Cargo verbose versions and both executable SHA256 hashes.
`baseline-overlay.patch` includes tracked and untracked baseline fixture changes;
its SHA256 is bound by the manifest. Raw traces, per-round logs, build logs and
JSON results remain in the same artifact. Ordinary main pushes and manual
dispatches on main do not rebuild the measurement baseline.
