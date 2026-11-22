# AES-128 packet signing for private ZXS SMB sessions

This module retains RustCrypto code under MIT or Apache 2.0. It is not a public
crypto API, a provider selection, or an independent Cargo package. The
[import inventory](../../../../vendor/rust/SOURCES.toml) keeps both original
archive SHAs and upstream commits:

| Original source | Upstream commit | Original ZXS Git blob |
| --- | --- | --- |
| `cmac` 0.8.0 `src/lib.rs` | `192737458879ba34d822c84950b1cca001c8a73e` | `ca7062b0bccad6a9ecf3cce1c45e1c7e137fe12d` |
| `cmac` 0.8.0 `src/block_api.rs` | same | `0aadb21991edead479d78dcada87465a08c29332` |
| `dbl` 0.5.0 `src/lib.rs` | `303c9e8e8fd5e08de4719bd6614935bc68c63a55` | `61a2d9199dd98cf24c955388c4ca45cd02f827c1` |

Original CMAC [README](README.upstream.md), [VCS record](upstream-vcs.json),
[MIT](LICENSE-MIT) and [Apache 2.0](LICENSE-APACHE) notices; original Dbl
[README](double/README.upstream.md), [VCS record](double/upstream-vcs.json),
[MIT](double/LICENSE-MIT) and [Apache 2.0](double/LICENSE-APACHE) notices.
These files were retained unchanged, not refreshed from upstream.

This private module uses the same 128-bit forward doubling, CMAC padding,
subkeys, AES type, buffer and erasure. The [fixed-output support](../../fixed_output/ORIGIN.md)
uses the original array types and MAC verification.

The tests include [RFC 4493 §4](https://www.rfc-editor.org/rfc/rfc4493#section-4)
known answers at the session signer, separate from the existing signed wire
fixtures. Those are not physical SMB client interoperability or release
acceptance evidence.
