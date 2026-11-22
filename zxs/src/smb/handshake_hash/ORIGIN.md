# Hashes for the private ZXS SMB handshake

These sources are from the RustCrypto Developers' `sha2` 0.11.1,
[commit `138dc5a0`](https://github.com/RustCrypto/hashes/tree/138dc5a05b93f8e49cda177fc6cfd7cc43673e5d/sha2).
The original imported `src` Git tree is
`d79b13ca602c63276eea1cddff9522d9e46c3a74`; the previously verified archive
SHA-256 is `47d7069beb7d6ac7b9acd1039986e73443f24234f41074da099d6f994ac9ad19`.
See the original [VCS record](upstream-vcs.json), [README](README.upstream.md),
[MIT](LICENSE-MIT), [Apache 2.0](LICENSE-APACHE), and
[import inventory](../../../vendor/rust/SOURCES.toml). No upstream refresh.

ZXS exposes only the internally used SHA-256 (session key derivation) and
SHA-512 (preauthentication transcript) wrappers. The selected macro traits,
byte-level algorithms and constants are unchanged. The removed OID support
and alloc feature were already inactive. The same compile-checked buffer
and variable-core erasure remains mandatory; no caller flag can turn it
off. Changed upstream implementation files carry their own integration
notice; compression differences are only paths to the same constants.

The original selector files, default fallback and alternate backend code
are retained. The pre-existing experimental intrinsic crate attribute
stays conditional on its original selectors and is absent from the
primary build. This move neither adds nor qualifies forced CPU options
or other platforms. Only the primary Linux x86-64 GNU product is tested;
the [RFC 6234 §8.5](https://www.rfc-editor.org/rfc/rfc6234.html#section-8.5)
answers and existing SMB/NTLM synthetic transcripts are not real client
or formal acceptance.

The [compile-time selector](../backend_select/ORIGIN.md) and
[CPU detection](../hardware_capabilities/ORIGIN.md) stay private. The
[fixed-output support](../fixed_output/ORIGIN.md) retains the required
buffering and type widths.
