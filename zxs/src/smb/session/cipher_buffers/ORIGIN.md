# In-place cipher buffers

Adapted from RustCrypto (MIT OR Apache-2.0), [revision `d730578e`](https://github.com/RustCrypto/utils/tree/d730578e69d68e185d07cb55e32eff1691133a46/inout).
Verified source archive SHA-256: `3a580c13f0ffc335ef42dd3f6b1a41b726051c5e1656d9a3aa6fddb3c5dfbbf9`.
See the [source inventory](../../../../vendor/rust/SOURCES.toml) and unchanged
[MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE) notices.

| Original source | Git blob | Local adaptation |
| --- | --- | --- |
| `src/inout.rs` | `1a82000d95fdec629f19b834fef486d0f3e94cbd` | Borrowed single value and array slot |
| `src/inout_buf.rs` | `5ab50df31dcf851bd016a4174c58a0a7c7c59eca` | Borrowed slice, groups, tail and iterator |

Only the original in-place construction and view operations are used by SMB.
Pointers and backing lifetimes stay paired until each borrow ends; no raw
admission, separate output, padding, or ownership transfer API is included.
The original bounds, array representation, and cipher callers are unchanged.
