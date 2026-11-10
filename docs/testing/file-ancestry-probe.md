# Lightweight file ancestry probe

This standard-library-only harness includes the actual production
`src/engine/src/business/file_ancestry.rs` and its actual separated unit tests
using `#[path]`. It has no production dependencies and does not compile the
engine crate. Minimal stand-ins model only State fields and error types used by
that helper. Passing establishes the helper's graph traversal/accounting behavior,
not integration with the actual engine State, endpoint projection, transactions,
HTTP routes, persistence, or complete engine compilation.

From the repository root, with a Rust 2024 toolchain:

```sh
rustc --edition 2024 --test -C debuginfo=0 test/engine/probes/file_ancestry.rs \
  -o /tmp/zxs-file-ancestry-probe
/tmp/zxs-file-ancestry-probe --nocapture
```

Actual-engine regressions remain under `test/engine/unit/business/` and
`test/engine/unit/engine/`, and are part of their engine crate's normal tests.
