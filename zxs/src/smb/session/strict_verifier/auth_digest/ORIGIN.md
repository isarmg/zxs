# Digest for private ZXS NTLM verification

The RustCrypto Developers' `md-5` 0.11.0 library at
[upstream commit `b5051e5a`](https://github.com/RustCrypto/hashes/tree/b5051e5a5e7dc86a6c27c1ec7a390744ebcfb97a/md5)
is retained here only for the existing strict NTLM proof and channel setup.
See [original VCS record](upstream-vcs.json), [README](README.upstream.md),
[Apache 2.0](LICENSE-APACHE), [MIT](LICENSE-MIT), and the
[import inventory](../../../../../vendor/rust/SOURCES.toml) for the unchanged
SHA-256 of the originally verified source archive:
`69b6441f590336821bb897fb28fc622898ccceb1d6cea3fde5ea86b090c4de98`.

| Original Rust source | Original ZXS Git blob |
| --- | --- |
| `src/lib.rs` | `e1f1dbb2f44150c2170e6b90a7a5d4824f534c48` |
| `src/block_api.rs` | `5bb096dbcbfcd2e73c30bae812cc00f854ae2bba` |
| `src/compress.rs` | `10b25f6349e994a83a5e8fa90fda32a5d65c17df` |
| `src/compress/soft.rs` | `dd4c232d3d5dcb5240c4fb7916aa4c1a497c530a` |
| `src/compress/loongarch64_asm.rs` | `7eac2a63571fa96c974521448735b5be840bbfec` |
| `src/consts.rs` | `f26b8c4a430f7097e81ec2802c07799284136ea1` |

ZXS only modified module and config plumbing. The constants are identical;
both compression backends now use the same relative path to that module.
The compression instructions, state layout, padding and digest bytes are
unchanged. The original selector used software
everywhere except LoongArch without an explicit software selection; both
cases remain, though the ZXS recipe qualifies only x86-64 Linux GNU.
The former package was built with defaults off. The same Digest traits and
buffer are used, without introducing the optional OID or alloc features.
The selected core Drop still wipes state and length through the same
`zeroize` code, and the wrapper retains unconditional compile-checked
`ZeroizeOnDrop` for that core and buffer. None is a switch in this module.

This is not a new primitive, provider or public digest API. MD5 is only
retained for required legacy NTLM calculations, not authentication for new
protocols. The [RFC 1321 A.5](https://www.rfc-editor.org/rfc/rfc1321#appendix-A.5)
fixtures and existing signed synthetic NTLM transcript are development
checks, not acceptance with a real client or platform certification.
