# Recovery deduplication: raw observations

Measured source: `de187851aff545ad9684916407e1724f820beacb`; remote baseline: `3fd13d3e205e59b7b8f840ae164ad947e9a06533` plus observation-only overlay. See [report](recovery-deduplication.md).

- `source-provenance.json`: source/tree/fixture/lockfile/binary hashes and isolated targets.
- `observer-verification.json`: exact baseline-only observation check.
- `*-r1.*`: four completed formal processes; `*-smoke.*`: separate observer smoke checks.
- `summary.json`: validated complete sample comparison; not a production timing gate.
- `*-overlay.patch.gz`: decompress to reproduce measured source; hashes refer to decompressed bytes.
- `run.py`, `capture-provenance.py`, `summarize.py`: captured local harness; adjust absolute workspace paths before reuse.

Each formal process finished integrity checking with exit code 0. Repeated opens within one fixture are not independent datasets. Warnings and actual selected checkpoint sequences are part of every sample; a selected old prefix is not full warm recovery.
