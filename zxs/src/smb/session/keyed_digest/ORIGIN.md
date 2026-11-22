# Keyed digest for private ZXS SMB sessions

The five Rust sources are from **The RustCrypto Project Developers**,
`hmac` 0.13.0, [upstream commit `0236c8eb` at `hmac/src/`](https://github.com/RustCrypto/MACs/tree/0236c8eb50098dd7f277a71ab89caaeb1e7314df/hmac/src).
The originally verified `.crate` SHA-256 is
`6303bc9732ae41b04cb554b844a762b4115a61bfaa81e3e83050991eeb56863f`.
See the original [VCS record](upstream-vcs.json), [README](README.upstream.md),
[MIT license](LICENSE-MIT), [Apache 2.0 license](LICENSE-APACHE), and
[ZXS import inventory](../../../../vendor/rust/SOURCES.toml). None of these
original files were refreshed. The original Rust source Git blobs are:

| Upstream name | Git blob |
| --- | --- |
| `lib.rs` | `d21153bca4359216cc10a07bcbfe189e6b67c9cb` |
| `block_api.rs` | `3071f682d4a4f06dc594478e1f1122a75b3732e3` |
| `simple.rs` | `7c8de58b5eace5bcc4687c6d345d9f8d25cc1e8a` |
| `simple_reset.rs` | `c676aedaebcb56f41a8b670f5990a961afcb46b7` |
| `utils.rs` | `833108c778870a7e3035472138afef55ae0aa981` |

ZXS scopes this module to SMB and uses the [private fixed-output support](../../fixed_output/ORIGIN.md).
The key processing, buffering, erasure and session verification are unchanged.

This is only for the existing private ZXS SMB session derivations and NTLM
verifier, not a public MAC API or a general-purpose service. The synthetic
transcript comparator's type alias exists only under `cfg(test)`. The existing
fixed NTLM/SMB answers and comparator inputs are unchanged; no Cargo or
interoperability check was run for this move.
