# Pending digest and MAC blocks

Adapted from RustCrypto (MIT OR Apache-2.0), [revision `cbd0963c`](https://github.com/RustCrypto/utils/tree/cbd0963c685df025c42bed31f78c50d1bada3805/block-buffer).
Verified source archive SHA-256: `d2f6c7dbe95a6ed67ad9f18e57daf93a2f034c524b99fd2b76d18fdfeb6660aa`.
See the [source inventory](../../../vendor/rust/SOURCES.toml) and unchanged
[MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE) notices.

| Original source | Git blob | Local file |
| --- | --- | --- |
| `src/lib.rs` | `827d68db807c9c7f11b14d43882225afa6c1a927` | `mod.rs` |
| `src/sealed.rs` | `292f95554e26e1f5d0e3229d8571d8bf1355a382` | `sealed.rs` |

The block-size bound, eager and lazy cursor rules, checkpoint bytes, and
reset guard are preserved. Drop always erases the buffer and cursor, as in
the previously selected build. The unused read buffer and convenience APIs
are not included.
