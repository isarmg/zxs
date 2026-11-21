# Private NTLM key exchange source

The RC4 implementation is from **The RustCrypto Project Developers**,
`rc4` 0.2.0, [upstream commit `b516422e` at `rc4/src/lib.rs`](https://github.com/RustCrypto/stream-ciphers/blob/b516422ebb49ff0d9140b22ac8c2a627d10a452e/rc4/src/lib.rs).
The provenance is also in [SOURCES.toml](../../../../../vendor/rust/SOURCES.toml)
and the unchanged [upstream VCS record](upstream-vcs.json).
Original source blob (Git): `2150aa83071c12d120a9096760e8d0b7e1749192`;
the originally verified `.crate` SHA-256 is
`840038b674daa9f7a7957440d937951d15c0143c056e631e529141fd780e0c92`.
Preserved unchanged: [MIT license](LICENSE-MIT), [Apache 2.0 license](LICENSE-APACHE)
and [upstream README](README.upstream.md). No upstream sync or build-time download.

Only the crate-to-module imports, visibility and crate-only attributes changed.
ZXS previously enabled `zeroize` for every use, so the existing `ZeroizeOnDrop`
and the same three erasures are unconditional here. The key schedule, keystream,
length checks and protocol call sites are unchanged. The only consumer is the
existing verifier's optional key exchange and its SPNEGO MIC handling. This is
not a general cryptography API, new authentication capability or file encryption.

The existing public NTLM known answers (with and without key exchange) remain
in `src/test/smb/strict_verifier.rs`; the additional source fixture cites the RFC
6229 first-block answer and checks the erasure trait is present. No tracing or
inspection of freed memory is part of the fixture. None of these were run for
this source-only move.
