# Fixed-size arrays

Adapted from `RustCrypto` (MIT OR Apache-2.0),
[revision `09310c55`](https://github.com/RustCrypto/hybrid-array/tree/09310c55b75e2cd408951c7e2e86dba16c63fc68).
Verified source archive SHA-256: `27f864f10dfb56725ce5ce5472bc52252c8f93a4ab86327122cebf62c5f59a17`.
See the [source inventory](../../../vendor/rust/SOURCES.toml), original
[MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE) notices.

| Original source | Git blob | Local file |
| --- | --- | --- |
| `src/lib.rs` | `efd8fd90421059bc89a9806c0bc73deecff1e9f5` | `mod.rs` |
| `src/traits.rs` | `1df5bb871085ab191d39f580194fe82a0f79fbf8` | `shape.rs` |
| `src/from_fn.rs` | `8a507c67ebab1a22a90c908346ead19e77fe1a5e` | `initialize.rs` |
| `src/iter.rs` | `86c5d2fa2d7a5c233d134cbc6b95225e10a9cdb1` | `iteration.rs` |
| `src/sizes.rs` | `43cb00e19c9659a5a4f236478cc87788ec2a79f6` | `sizes.rs` |

The fixed-size representation, supported size pairs, checked slice views, typed
split and concatenation, initialization guard and selected zeroize behavior
are retained. Unused conveniences and unselected allocator, serialization and
additional-size features are not included. The array is private to SMB.
