# Historical benchmark installation fixture

`benchmark-install-schema.zip` is used only by the exact historical installation
scenario in `test_benchmark_relocation.py`. Ordinary verification copies current
production bytes and authenticates current retained benchmarks/evidence; it never
restores missing current benchmarks from this archive.

The installation scenario reverses unchanged, authenticated layout edits with
`source_layout.project_bytes`. Later semantic changes need archived prior bytes.
Every restored file must match `proposed_files_sha256` in the pinned benchmark
receipt `8a08cc5b47c335b9e6aa2b7361ab7259fdc394c8dad16c29efb3e90d330ffbbc`;
the complete restored package inventory is checked afterward.

The original three entries (`Cargo.toml`, `src/validation/keywords/format.rs`,
`src/validation/keywords/mod.rs`) retain their bytes and ZIP entry attributes. Five added
entries were recovered from public baseline
`8a36ef5c96afcb985a9b3c4bbe54e734fa1ef0c7`, under physical owner
`src/foundation/data-storage/schema-validation`. Their physical bytes were passed through
the authenticated layout's strict `project_bytes` inverse, and the resulting
logical bytes matched the pinned benchmark receipt. Paths below are relative to
logical owner `src/foundation/schema-validation`.

| Path | Baseline physical Git blob | Restored SHA-256 (pinned receipt) |
| --- | --- | --- |
| `src/validation/error.rs` | `6a274ba565c21a8d5a35718222f4b6e3d1195689` | `5e6af5c9aeded80b93fa31df8fb2bd69939e560fac0dd18a2204b796ce4da281` |
| `src/validation/keywords/additional_properties.rs` | `367b0707655c80756204a164b066cbc524195a03` | `b2c89ec38a4d1838ae1e394d385271ead00136353f7ea1a170b4255e38eac113` |
| `src/validation/keywords/pattern_properties.rs` | `479b4e0f7f55029e8e28ee70237326afdfacea18` | `3ff19cf3618ba806d34d6ed8322dd458908342d9c27aa2f0bebd050f17583436` |
| `src/validation/properties.rs` | `733cd8272798f8842c1881178fed2440efe594cb` | `1195808b06849e39b4dfeb9a8bc4f2a7afdababec142c357884a9d6a2cf72acd` |
| `src/validation/regex.rs` | `d388be686dccee9475154ecabaef759146536eff` | `010c258f21de2a0a9c21b118ae335d984a64b6c14022fd507cb9adbd5a1b9322` |
