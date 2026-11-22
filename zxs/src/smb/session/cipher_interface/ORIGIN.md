# Private SMB cipher interface

Adapted from RustCrypto (MIT OR Apache-2.0), [revision `f836f71f`](https://github.com/RustCrypto/traits/tree/f836f71fadb3b975b577a293d57851d044f0a44b/cipher).
The original archive SHA-256 is `e8cf2a2c93cd704877c0858356ed03480ff301ee950b43f1cbe4573b088bfa6c`;
see [source inventory](../../../../vendor/rust/SOURCES.toml).
The [MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE) notices are unchanged.

| Original source | Git blob | Local file |
| --- | --- | --- |
| `src/block.rs` | `9f0008d12425c3b56576ba8125f88f4502e9d77c` | `block.rs` |
| `src/block/backends.rs` | `95c4fb5720c335795cadc24ef43d7b85879d44e7` | `block/backends.rs` |
| `src/block/ctx.rs` | `72b53a2a5d88b03ddbfe0c904d998ee098138f2b` | `block/ctx.rs` |
| `src/stream.rs` | `33902bb1936912c2e3d683331579cf98070e6e2a` | `stream.rs` |
| `src/stream/errors.rs` | `e30576da61738fea6d203d259cdddbcffee1325c` | `stream.rs` |

AES-128 and CMAC retain the original immutable single-block, parallel group and
tail methods and closure dispatch. NTLM's RC4 retains the original length
check before the in-place stream call. Data types, input/output buffers and
key erasure use the same support code and features; no cipher backend or
algorithm changes. ZXS does not include the unused mutable block-mode, tweak,
padding, stream seek/wrapper or development APIs.
