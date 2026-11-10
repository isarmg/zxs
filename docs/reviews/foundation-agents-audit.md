# Foundation / Rust tools AGENTS audit

Baseline: `8a36ef5c96afcb985a9b3c4bbe54e734fa1ef0c7`. This is a scoped audit,
not a claim that every foundation source line complies with AGENTS.md.

## Reproduce the candidate inventory

```sh
python3 tools/rust-sources/agents_audit.py > /tmp/foundation-agents-inventory.json
python3 test/tools/rust-sources/test_agents_audit.py
```

The read-only inventory covers every current `src/foundation/**/*.rs`, records
actual SHA-256 bytes (not merely the current receipt), maps exact original byte
matches, and lists line-numbered syntax candidates. At this baseline there are
5,488 Rust files: 2,215 retain identical bytes at their original package-relative
path, 164 match original bytes at another path, and 3,109 need changed/merged/added
source-mapping review. A byte match is provenance evidence, not a compliance pass.

Each candidate starts **unreviewed**. Regex also finds comments, documentation,
tests, normal cancellation and required public API checks. It misses multiline
expressions and many semantic rules. Neither candidate counts nor a zero count
are compliance verdicts. Do not mechanically replace all matches.

For each candidate, record its caller and input source, prior validation, concrete
error type, relevant public/FFI contract, disposition (confirmed violation,
required boundary, valid contract, unresolved), evidence, smallest repair and test.
Prioritize locally changed code and actual consumer error paths. Use the source
ledger's supported profile closures to prioritize, not as proof of final linkage.
Follow rename/merge/source-move receipts before declaring changed bytes to be a
local algorithm change. Original upstream code remains in scope, not exempt.

## Repairs and boundaries

- Rust tool SQLite failures now propagate their original error types. Schema
  errors use the existing owned validation error conversion. Contextual schema
  and document-link failures retain their source, including I/O kind and path.
- Validated `$ref` values and the first `str::split` segment use explicit
  invariants, not empty-string fallbacks. Schema meta-validation remains the
  external input boundary.
- Web build lock cleanup executes explicitly on success and returned failure.
  Cleanup errors fail the command before the success event. Simultaneous build
  and cleanup failure retains both concrete errors. Drop is only an unwind
  fallback and reports cleanup failure; it does not panic for that I/O failure.
- SQLite PRAGMA formatting into String expresses its infallibility with expect.
  Its connection, SQL/FFI, range, NUL and public API validation remain intact.

Required checks remain: document canonicalization and root containment, generated
link symlink containment, output-name injection checks, external numeric bounds,
I/O errors, and unknown-caller public library inputs. The nonempty collation guard
also remains: it avoids taking an asynchronous connection lock when there is no
collation work. CLI anyhow/boxed errors are not the forbidden library-wide policy.

## Verification and remaining scope

Focused tool unit/integration tests exercise source preservation, schema rejection,
link containment, build-lock success/failure/double-failure cleanup, and real SQLite
PRAGMA connect/optimize-close paths. The Unicode generator is checked against its
fixed inputs. These checks do not substitute for whole-product/browser/SMB checks.

Foundation semantic review covered SQLite connect/close/statement/reset/worker,
transaction rollback and pool-close failure reporting, Tokio filesystem task
cancellation, schema pattern execution and owned-error conversion, task-dispatch,
and or_poisoned. Targeted red/green regressions reproduced and then checked fixes
for RETURNING reset failures, failed rollback worker retirement (including queued
operations), pool-close source preservation, filesystem JoinError preservation,
and patternProperties execution errors. Cancellation and cleanup paths were
reviewed separately from normal success paths. FFI callback panic handling and
the remaining inventory are not represented as line-by-line approval; public
checks must not be removed based on regex.

After integrating foundation edits, refresh and review the canonical source
receipts and run `python3 tools/rust-sources/manage.py verify --metadata` in the
integration checkout. This audit intentionally does not rewrite immutable origins.
