# ZXS-owned Rust dependency sources

The private SMB implementation uses RustCrypto SHA-512, HMAC-SHA-256 and
AES-128-CMAC. Its strict, offline NTLMv2 verifier additionally needs the legacy
MD5/HMAC-MD5 and RC4 primitives prescribed by MS-NLMP; they are not SMB file
encryption, a password-storage recommendation, or general crypto primitives
for new ZXS features. The remaining path packages retain their complete
**library/build sources**, including every published CPU backend; ZXS-owned
modules retain the used source. Neither an executable archive, a new product
nor an alternative registry is built here. Paths are relative to the importing
manifest; neither the invocation directory, another project nor a host
cryptographic service supplies a crate.

## Provenance and local changes

[SOURCES.toml](SOURCES.toml) records the archive URL, SHA-256 from the
crates.io index, upstream repository/revision, license, edition and minimum
Rust version of each upstream package. All 21 original archive checksums were verified before
import (2,557,868 compressed bytes in total). No .crate archive or prebuilt
object is committed. The active manifests, library and build sources, and required license texts
are in this tree. Earlier intake manifests are recoverable from Git history.

For the remaining path packages, the active manifest points dependency edges
at local paths and omits upstream test and benchmark targets not carried here. Code
incorporated into ZXS instead records its original blob and actual local
integration changes. ZXS-authored SMB test source is kept under `tests/smb/`.

The chosen [sha2 0.11.1 release](https://github.com/RustCrypto/hashes/tree/sha2-v0.11.1/sha2)
includes [upstream's AArch64 round-constant fix](https://github.com/RustCrypto/hashes/pull/922):
the 256/512-bit backends now take a slice of the full SIMD width rather than a
reference to a single constant. This is not a locally implemented algorithm or
a reason to disable an architecture. [RUSTSEC-2021-0100](https://rustsec.org/advisories/RUSTSEC-2021-0100.html)
affects sha2 0.9.7, not this version. The private [tag comparison](../../src/smb/fixed_output/TAG-COMPARE-ORIGIN.md)
uses the [private comparison backend](../../src/smb/tag_choice/ORIGIN.md).

[md-5 0.11.0](https://github.com/RustCrypto/hashes/tree/b5051e5a5e7dc86a6c27c1ec7a390744ebcfb97a/md5) and
[rc4 0.2.0](https://github.com/RustCrypto/stream-ciphers/tree/b516422ebb49ff0d9140b22ac8c2a627d10a452e/rc4) are unyanked
Rust 1.85 releases; the two additional archives are 17,404 and 15,591 bytes.
No package directory for either was found at the reviewed
[RustSec snapshot](https://github.com/rustsec/advisory-db/tree/074ea9a7ed619f511b5b7374c7a97e98d1e98be4/crates).
That is not an assertion that these primitives are safe for new protocols.
Upstream explicitly warns about RC4 and MD5. The ZXS verifier limits their use
to NTLM's keyed proof and MIC, standard key derivations and optional key
exchange/MIC wrapping. No MD4/password processing or host crypto provider
is added here. RC4 is now a [private key-exchange module](../../src/smb/session/strict_verifier/key_exchange/ORIGIN.md),
no longer a Cargo package; its recorded origin and licenses remain. It uses
the checked in-place stream method in the [private cipher interface](../../src/smb/session/cipher_interface/ORIGIN.md).

MD5 is now part of the [private NTLM digest](../../src/smb/session/strict_verifier/auth_digest/ORIGIN.md),
not a Cargo package. It uses the same compression choices and original
state and buffer erasure; its upstream OID and alloc options remain off.

HMAC now belongs to the [private SMB keyed-digest module](../../src/smb/session/keyed_digest/ORIGIN.md),
not a Cargo package. The [private fixed-output support](../../src/smb/fixed_output/ORIGIN.md)
provides the used block wrappers and MAC traits. HMAC-SHA-256 session
derivation and HMAC-MD5 verification are unchanged.

CMAC and the forward 128-bit doubling it uses are now part of the
[private SMB packet MAC](../../src/smb/session/packet_mac/ORIGIN.md), not
separate Cargo packages. Signing retains the same buffering, erasure and
traits. Unused doubling sizes and inverse operations were not moved. The
used [AES-128 implementation](../../src/smb/session/block_cipher/ORIGIN.md)
is now a private ZXS module rather than a separate Cargo package. AES and
CMAC use the block traits in the same private cipher interface.

The pinned SHA-256 and SHA-512 code used by SMB now belongs to the
[private handshake hash module](../../src/smb/handshake_hash/ORIGIN.md).
It keeps the same backend dispatch and clears its state as before.
No separate SHA-2 crate or unused fixed wrapper is needed in the product.

[CPU capability detection](../../src/smb/hardware_capabilities/ORIGIN.md) is private to SMB.

SHA-2 and AES share the internal [compile-time selector](../../src/smb/backend_select/ORIGIN.md).
The AES [portable word selection](../../src/smb/session/block_cipher/backends/fixslice/NATIVE-WORD-ORIGIN.md)
uses the same target predicates and priority without a separate Cargo package.

## Active paths and build scope

The ZXS workspace excludes the remaining local path packages, so their own
upstream dev and unselected optional dependencies do not enter the normal ZXS
graph. The [private cipher buffers](../../src/smb/session/cipher_buffers/ORIGIN.md)
retain the original in-place views using the same array types. The
[shared cipher and digest types](../../src/smb/crypto_types/ORIGIN.md)
keep key initialization and state handling unchanged. The [private block accumulator](../../src/smb/block_accumulator/ORIGIN.md)
keeps the same pending-block checks and erasure. The [private fixed arrays](../../src/smb/fixed_array/ORIGIN.md)
keep the same layout, size and erasure behavior; no array Cargo package remains.
No derive, alloc/oid, C provider, generated binding, system crypto or external
service is required by this selection. Enabled path edges, also spelled out in
Cargo.lock:

```text
storage-server -> libc, typenum [const-generics], zeroize
```

All other nodes are leaves in this selected closure. All of these paths
are inside ZXS. Inactive optional/dev declarations are not silently
vendored or enabled. Any feature change needs a new closure review.
Local path packages have no registry checksum field in Cargo.lock; use
SOURCES.toml to check their published origin. The highest remaining package minimum is
Rust 1.85, below ZXS's existing Rust 1.100 / pinned
1.100.0. [Debian 13's stock rustc](https://packages.debian.org/trixie/rustc)
is older than the pre-existing project toolchain; this does not change ZXS's
compiler or authorize a toolchain install.

Scope here is source and manifest review against [Cargo resolver rules](https://doc.rust-lang.org/cargo/reference/resolver.html)
and static agreement of all enabled normal/build/target edges with the path
lock. Cargo resolution/build, build scripts, project tests, real clients and
release artifacts were not executed or verified. No other project's config,
crate source, network settings or service entry is changed.
