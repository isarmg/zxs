# SMB memory erasure

Adapted from `RustCrypto` (MIT OR Apache-2.0),
[revision `c66481d9`](https://github.com/RustCrypto/utils/tree/c66481d9952cb8f363e7140c7122e3d238581ca6/zeroize).
Verified source archive SHA-256: `85ccff8a4805aba1eaa4cd4afcd49f7557dd2cf5fd79ffc37f8959466effc5aa`.
See the [source inventory](../../../vendor/rust/SOURCES.toml) and the original
[MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE) licenses.

| Original source | Git blob | Local file |
| --- | --- | --- |
| `src/lib.rs` | `de53f2b8e0b085b5d91520b8c060a27cd4a2e58e` | `mod.rs` |
| `src/barrier.rs` | `5336bb22698ed05634fdd61c4fda3dfe77b9d9ce` | `barrier.rs` |
| `src/bulk.rs` | `70c69c3af0f41d0ef2751277dd6ddf97be72291b` | `bulk.rs` |

The scalar writes, bulk barrier and volatile fallback, fixed-array erasure,
and wipe on drop and before replacement use the original implementation.
Inactive features and uncalled erasure APIs are not included.
