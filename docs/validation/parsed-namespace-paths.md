# Parsed namespace paths

Baseline: `f1d2f8c2`. This change preserves the fixed Unicode 16 naming tables,
NFC creation policy, canonical-equivalent lookup policy, case folding, component
budgets, traversal rejection and existing JSON field shapes.

## Boundary and ownership

`name-policy` owns `PortablePath` (creation/stored spelling) and `LookupPath`
(existing-name reference, including NFD). Both hold private display/key fields;
construction is fallible, and there is no mutable string accessor. Its concrete
error variants are independent of storage. `storage-contract` maps these causes
to the existing wire error codes while retaining the source.

- `Entry.path` is a `PortablePath`. `name()` and `lookup_key()` derive from that
  authoritative value rather than independently mutable strings. Entry serde
  preserves `entry_id/path/name/lookup_key/kind/file_id/version_id/size`. Disk
  deserialization parses `path` and rejects inconsistent redundant wire fields.
- `StagedFile.path` and the upload-preparation claim retain `PortablePath`.
  `prepare_upload_file` accepts a borrowed typed path from HTTP/SMB admission.
  Local filesystem adapters parse their raw input once; finishing a prepared
  upload checks current workspace facts but does not reparse the frozen path.
- `NamespaceOperation::parse` applies the creation or lookup rule according to
  the operation and parses the destination as a creation path. The enum carries
  a required destination only for rename; missing rename destinations and extra
  destinations on other operations are rejected as `INVALID_REQUEST` at admission.
  Its JSON fields
  remain `kind/path/destination`. Explicit adapter parsing can retain the original
  naming error code; serde parsing is the persisted-data boundary.
- Namespace mutations borrow retained keys. Ancestors and renamed suffixes reuse
  component keys; joining a suffix checks only the new aggregate byte/UTF-16/depth
  budgets. Published queries continue to binary-search borrowed keys.

The server adapter migration is a separate integration change. This document
covers the naming/contract/engine changes, not a claim that every unrelated raw
path API has been migrated. Root-directory strings in directory/backup metadata
remain outside `PortablePath`, which deliberately represents nonempty entry paths.

## Removed repeated work

`validate_entries` no longer clones each display path or calls `relocate_entry`
for every unchanged entry. It retains entry-count, normalized-key collision,
parent-kind and sort checks: these are relationships between entries, not facts
that an individual path type can prove.

`HistoryManifest` no longer repeats per-entry path validation, leaf-name
comparison or Unicode lookup-key recomputation. The wire-to-domain conversion
establishes those facts. Existing cross-record closure and scope checks remain.

Workspace context, selection and version-build helpers retain typed paths or
borrowed keys instead of converting back to raw paths. Commit lookups use
`StagedFile.path.key()`; dependency checks and
namespace operations use their retained keys. A rename borrows the source to
obtain component depth rather than cloning it. Only changed entries get new paths;
unchanged entries are not cloned or normalized by the namespace validator.

Batch admission parses each raw source/destination once, preserving its previous
strict NFC rule. The private plan carries the parsed target alongside its wire
reference; projection, execution and active rename operations reuse that value.
Copy identity caches borrow the retained leaf key rather than folding names again.
Backup composition borrows the resolved entry location and rebases retained files
without validating their already admitted components. Upload base lookups, staged
budget accounting and historical-save selection also consume retained keys.
`PortablePath` can become `LookupPath` by moving or cloning its two admitted
strings, with no parsing or Unicode work.

## Same-fixture cost comparison

Run:

```sh
cargo run -p name-policy --example path_profile --release --offline
```

The flat-file fixture uses `Straße-Café-报告-NNNNNN.txt`. Both arms execute 20
rounds of the same collision index and sort. The reference arm additionally runs
the former display clone, path validation, leaf-name allocation and key
recomputation. Both arms start with materialized entries outside the timed region;
afterward every display path, leaf name and key must match.

One local release measurement (Rust 1.99.0):

| Entries | Reference, 20 rounds | Typed, 20 rounds | Ratio |
| ---: | ---: | ---: | ---: |
| 1,000 | 83.11 ms | 2.88 ms | 28.86× |
| 10,000 | 889.77 ms | 41.90 ms | 21.24× |
| 100,000 | 8,520.86 ms | 507.72 ms | 16.78× |

This measures the path loop plus collision indexing/sorting, **not** complete
transaction latency, I/O or SMB open throughput. The flat fixture has no parent
relationships. Structurally, each unchanged entry loses one validation, one full
key recomputation and at least three owned-string allocations per validation pass
(display clone, name, key), excluding allocations internal to Unicode processing.
No production counters or test hooks were added for this measurement.

The ignored engine test also compares the actual namespace validator against a
conservative reparse reference and asserts equal canonical output:

```sh
cargo test -p storage-engine --lib --release --offline parsed_path_namespace_cost -- --ignored --nocapture
```

On commit `fa9d64d4`, 20 rounds took 39.22 ms versus 1.87 ms for 1,000 entries
(20.92×), and 389.35 ms versus 24.67 ms for 10,000 entries (15.78×). This reference
reparses each stored path before running the same actual validator; it omits the
old separately allocated leaf name and is not a checkout-to-checkout transaction
benchmark. The test passed with byte-identical canonical outputs.

## Behavioral coverage

Coverage includes Unicode aliases (ß/SS, sigma, Hangul and combining marks),
creation rejection of NFD, lookup acceptance of NFD, traversal/reserved-name and
raw length budgets, rebased suffix budgets, exact wire round trips, rejection of
inconsistent external name/key fields, case-only rename, collision and parent
errors, retained identity/content, historical lookup and reopen.

Tests that formerly constructed an impossible invalid in-memory entry now mutate
raw JSON and assert rejection at deserialization. Cross-entry corruption tests
still exercise publication rejection; private path fields are not bypassed.

Local verification:

- `cargo test -p name-policy -p storage-contract -p storage-engine --offline`:
  27 naming, 71 contract, 535 engine unit and 73 engine integration tests pass;
  four existing/explicit benchmark tests are ignored by the ordinary run.
- Doc tests pass (engine rerun serially after a concurrent dependency rebuild
  invalidated an earlier rustdoc artifact).
- `cargo clippy -p name-policy -p storage-contract -p storage-engine --all-targets --offline -- -D warnings` passes.
- Formatting and `.github/scripts/check-architecture.py` pass.
