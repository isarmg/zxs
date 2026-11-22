# SMB tag comparison backend

The RustCrypto Developers' conditional comparisons are retained under their
original [MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE) notices. The
[source inventory](../../../vendor/rust/SOURCES.toml) records the original
archive hash and revision.

Only the original byte-slice equality path is used. Chunking, tail handling,
comparison instructions and the portable fallback are unchanged. The original
source took the slice chunk helpers from Rust core. Its
[caller](../fixed_output/TAG-COMPARE-ORIGIN.md) still checks tag lengths and
converts the byte choice through the original optimization barrier.

| Original used source | Original Git blob |
| --- | --- |
| `src/backends.rs` | `decaf158e86e627f62749ba42f905200622d4e73` |
| `src/backends/aarch64.rs` | `cf9859bb6babb260994787595a7d5414976d036c` |
| `src/backends/soft.rs` | `851db876fe25c1330bc534db20b32ccbbc578ab0` |
| `src/backends/x86.rs` | `d5371dadd1eb66682637feed2d802c04fa2fe41f` |
| `src/lib.rs` | `288ee16872f30dda98ae3eada1ecaf2ce0200ada` |
| `src/macros.rs` | `ab42a20b3b1b32ec092a2754612913fc2f516e9b` |
| `src/slice.rs` | `03bc756faa8a8f2682d9e634fcfa36b32340b50f` |
