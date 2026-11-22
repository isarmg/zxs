# Shared cipher and digest types

Adapted from RustCrypto (MIT OR Apache-2.0), [revision `93dee26c`](https://github.com/RustCrypto/traits/tree/93dee26c6bde3741a197f1c5f6b7baac277705f3/crypto-common).
Verified source archive SHA-256: `ce6e4c961d6cd6c9a86db418387425e8bdeaf05b3c8bc1411e6dca4c252f1453`.
See the [source inventory](../../../vendor/rust/SOURCES.toml) and unchanged
[MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE) notices.

| Original source | Git blob | Local file |
| --- | --- | --- |
| `src/lib.rs` | `4ebf38272ee161139e21d8a1fdee14066b9062c7` | `mod.rs` |
| `src/hazmat.rs` | `ae5ce5b03d314fb2979878a42d078fae52697883` | `state.rs` |

Key constructors retain their original borrow, length and inner-cipher rules.
The state interface leaves the existing digest codecs and wrapper validation
in place. Array representation and erasure support are unchanged. Unused key
generation, IV handling and generic primitive snapshot codecs are not included.
