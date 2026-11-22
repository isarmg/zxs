# Private SMB fixed-output support

Adapted from the RustCrypto Developers' fixed hash/MAC traits. The
[source inventory](../../../vendor/rust/SOURCES.toml) keeps the original
archive SHA-256 and revision. Original [MIT](LICENSE-MIT) and
[Apache-2.0](LICENSE-APACHE) notices remain.

ZXS keeps the used buffer, block and fixed output adapters, without a
package-level dynamic digest, XOF or test framework. The two existing MAC
verification forms still validate tag length before using the same
constant-time byte comparison. Buffer kind, state encoding, widths,
reset and erasure are unchanged.

| Original selected source | Original Git blob |
| --- | --- |
| `src/block_api.rs` | `c417dbe32c4a4924f15dd4ce026ba71e601b98f8` |
| `src/block_api/ct_variable.rs` | `8c8dba0384bb97acd93c975619b4ce3b8a676dfd` |
| `src/buffer_macros/fixed.rs` | `f238b0dd5da2a1568d5462f1567263aa8d5fee01` |
| `src/digest.rs` | `30cb3d8afedfd6f4448fa2d24290bf4c30e02c90` |
| `src/lib.rs` | `95492c96e75ab8b4d3128ab471f234414391c952` |
| `src/mac.rs` | `dc2905255f19a9b392f73c4510b92be2d56535c1` |
