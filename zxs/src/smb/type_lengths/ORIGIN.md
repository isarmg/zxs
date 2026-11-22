# SMB type-level lengths

Unsigned width support adapted from Paho Lurie-Gregg and contributors (MIT OR Apache-2.0),
[revision `0db9a0f7`](https://github.com/paholg/typenum/tree/0db9a0f731981f29266b63586c29fa07e4477b1a).
Verified source archive SHA-256: `b6f5e870be6c3b371b77fe0ee0bafb859fa4964b4404c27de1d380043c4dda20`.
See the [source inventory](../../../vendor/rust/SOURCES.toml), original
[license selection](LICENSE), [MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE).

| Original source | Git blob | Local files |
| --- | --- | --- |
| `src/bit.rs` | `4e3f3e87efce39d76699b18474dd0e0b9a7a851c` | `bits.rs` |
| `src/gen/consts.rs` | `af0bf7d622387bbde70d74d87c3586948d2ae222` | `consts.rs` |
| `src/lib.rs` | `ed44783f6032fa2ce1d0b78eb8862bfb31348a2a` | `mod.rs`, `ordering.rs` |
| `src/marker_traits.rs` | `2f06e2fba36b6d828d7db7e0e8700931d65d0db2` | `mod.rs`, `bits.rs`, `ordering.rs` |
| `src/operator_aliases.rs` | `c554abcbafeebb0de0285c8377213aa1ec5689f6` | `mod.rs`, `division.rs` |
| `src/private.rs` | `4fe0144b5d22d8ae52d67b7902a6f8da3850495f` | `canonical.rs`, `ordering.rs`, `division.rs` |
| `src/type_operators.rs` | `a6525d2add0a60f7c707b0fe8960fe23e6a17270` | `ordering.rs`, `division.rs` |
| `src/uint.rs` | `1a577031f02c73e33e9f0956114da0cadf941959` | `unsigned.rs`, `ordering.rs`, `division.rs` |

The same sealed binary representation, 553 array widths, generic arithmetic,
quotient and ordering remain in use. Signed arithmetic, alternate mappings
and uncalled value conversions are not included.
