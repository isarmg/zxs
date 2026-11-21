# ZXS-owned Rust dependency sources

The private SMB implementation uses RustCrypto SHA-512, HMAC-SHA-256 and
AES-128-CMAC. Its strict, offline NTLMv2 verifier additionally needs the legacy
MD5/HMAC-MD5 and RC4 primitives prescribed by MS-NLMP; they are not SMB file
encryption, a password-storage recommendation, or general crypto primitives
for new ZXS features. These are the complete
**library/build sources** of its pinned graph, including every published CPU
backend, not executable archives, a new product/workspace, or an alternative
registry. Paths are relative to the importing manifest; neither the invocation
directory, another project nor a host cryptographic service supplies a crate.

## Provenance and local changes

[SOURCES.toml](SOURCES.toml) records the archive URL, SHA-256 from the
crates.io index, upstream repository/revision, license, edition and minimum
Rust version of each upstream package. All 21 original archive checksums were verified before
import (2,557,868 compressed bytes in total). No .crate archive or prebuilt
object is committed. Inside each remaining package the published src/**, build.rs where
present, .cargo_vcs_info.json, README, LICENSE files, Cargo.toml.orig (author
manifest) and Cargo.toml.upstream (registry-normalized manifest) are unchanged.
The original intake comprised 735 such files. Unchanged files can be compared
by Git blob ID with their matching archive paths (map Cargo.toml.upstream to
Cargo.toml).

An active Cargo.toml starts from Cargo.toml.upstream, then adds a relative
path to any existing dependency section naming a vendored package (including
the `common` alias for crypto-common, matching dev entries and target
conditional entries) and omits explicit upstream test/bench targets that are
not imported. That policy still applies to the separate packages. Code
incorporated into ZXS instead records its original blob and actual local
integration changes. ZXS-authored SMB test source is kept under `src/test/smb/`.

The chosen [sha2 0.11.1 release](https://github.com/RustCrypto/hashes/tree/sha2-v0.11.1/sha2)
includes [upstream's AArch64 round-constant fix](https://github.com/RustCrypto/hashes/pull/922):
the 256/512-bit backends now take a slice of the full SIMD width rather than a
reference to a single constant. This is not a locally implemented algorithm or
a reason to disable an architecture. [RUSTSEC-2021-0100](https://rustsec.org/advisories/RUSTSEC-2021-0100.html)
affects sha2 0.9.7, not this version. The constant-time support releases
cmov 0.5.4 and ctutils 0.4.8 postdate the fixes listed in
[RUSTSEC-2026-0003](https://rustsec.org/advisories/RUSTSEC-2026-0003.html)
and [RUSTSEC-2026-0385](https://rustsec.org/advisories/RUSTSEC-2026-0385.html),
respectively. The advisory/source review is not a
certification, claim of no other vulnerabilities or substitute for future review.

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
the already enabled stream-wrapper and zeroize features of the same cipher
package rather than adding or updating any library.

HMAC now belongs to the [private SMB keyed-digest module](../../src/smb/session/keyed_digest/ORIGIN.md),
not a Cargo package. Its RustCrypto block-buffer macro and MAC traits still
come from the same `digest` package. The product's fixed HMAC-SHA-256 session
derivation and HMAC-MD5 verifier, including their typed digests, are unchanged.

## Active paths and build scope

The ZXS workspace excludes the 19 remaining local path packages, so their own
upstream dev and unselected optional dependencies do not enter the normal ZXS
graph. The cipher manifest has no default feature; ZXS makes the already active
stream-wrapper and zeroize edges explicit on its one direct cipher dependency.
The one direct digest dependency retains the already active block-api, mac and
zeroize features of the retired HMAC edge.
No derive, alloc/oid, C provider, generated binding, system crypto or external
service is required by this selection. Enabled path edges, also spelled out in
Cargo.lock:

```text
zxs -> aes, cipher [stream-wrapper, zeroize], cmac, digest [block-api, mac, zeroize], libc, md-5, sha2, zeroize
aes -> cipher, cpubits, cpufeatures [target cfg], zeroize
cmac -> cipher, dbl, digest
md-5 -> cfg-if, digest
sha2 -> cfg-if, cpufeatures [target cfg], digest
cipher -> block-buffer, crypto-common [alias common], inout, zeroize
digest -> block-buffer, crypto-common [alias common], ctutils, zeroize
block-buffer -> hybrid-array, zeroize
crypto-common -> hybrid-array
dbl, inout -> hybrid-array
hybrid-array -> typenum, zeroize
ctutils -> cmov
cpufeatures -> libc [target cfg, including Linux AArch64]
```

All other nodes are leaves in this selected closure. All of these paths
are inside ZXS. Inactive optional/dev declarations are preserved, not silently
vendored or enabled; any change of feature set needs a new closure review.
Local path packages have no registry checksum field in Cargo.lock; use
SOURCES.toml to check their published origin. The highest imported minimum is
Rust 1.89 (aes, edition 2024), below ZXS's existing Rust 1.100 / pinned
1.100.0. [Debian 13's stock rustc](https://packages.debian.org/trixie/rustc)
is older than the pre-existing project toolchain; this does not change ZXS's
compiler or authorize a toolchain install.

Scope here is source and manifest review against [Cargo resolver rules](https://doc.rust-lang.org/cargo/reference/resolver.html)
and static agreement of all enabled normal/build/target edges with the path
lock. Cargo resolution/build, build scripts, project tests, real clients and
release artifacts were not executed or verified. No other project's config,
crate source, network settings or service entry is changed.
