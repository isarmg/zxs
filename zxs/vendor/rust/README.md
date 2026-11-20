# ZXS-owned Rust dependency sources

The private SMB transcript uses RustCrypto SHA-512. These are the complete
**library/build sources** of its pinned graph, including every published CPU
backend, not executable archives, a new product/workspace, or an alternative
registry. Paths are relative to the importing manifest; neither the invocation
directory, another project nor a host cryptographic service supplies a crate.

## Provenance and local changes

[SOURCES.toml](SOURCES.toml) records the archive URL, SHA-256 from the
crates.io index, upstream repository/revision, license, edition and minimum
Rust version of each package. The nine archive checksums were verified before
import (1,114,585 compressed bytes in total). No .crate archive or prebuilt
object is committed. Inside each package the published src/**, build.rs where
present, .cargo_vcs_info.json, README, LICENSE files, Cargo.toml.orig (author
manifest) and Cargo.toml.upstream (registry-normalized manifest) are unchanged.
The 572 original files can be compared by Git blob ID with the same paths in
the checksum-matching source archives (map Cargo.toml.upstream to Cargo.toml).

An active Cargo.toml starts from Cargo.toml.upstream, then adds a relative
path to any existing dependency section naming a vendored package (including
the `common` alias for crypto-common, matching dev entries and target
conditional entries) and omits explicit upstream test/bench targets that are
not imported. No Rust code, license, version constraint, feature or
architecture guard was patched. The upstream archives remain the way to
inspect development/CI assets or run their own tests; ZXS-authored SMB test
source is kept under `src/test/smb/`.

The chosen [sha2 0.11.1 release](https://github.com/RustCrypto/hashes/tree/sha2-v0.11.1/sha2)
includes [upstream's AArch64 round-constant fix](https://github.com/RustCrypto/hashes/pull/922):
the 256/512-bit backends now take a slice of the full SIMD width rather than a
reference to a single constant. This is not a locally implemented algorithm or
a reason to disable an architecture. [RUSTSEC-2021-0100](https://rustsec.org/advisories/RUSTSEC-2021-0100.html)
affects sha2 0.9.7, not this version. The advisory/source review is not a
certification, claim of no other vulnerabilities or substitute for future review.

## Active paths and build scope

The ZXS workspace excludes each vendored package, so its own upstream dev and
unselected optional dependencies do not enter the normal ZXS graph. The sole
direct dependency is sha2 0.11.1, with its default alloc/oid features off.
The checked-in path lock includes:

```text
zxs -> sha2
sha2 -> cfg-if, digest, cpufeatures [target cfg]
digest -> block-buffer [default block-api], crypto-common [alias common]
block-buffer -> hybrid-array
crypto-common -> hybrid-array
hybrid-array -> typenum [const-generics]
cpufeatures -> libc [upstream target cfg, including Linux AArch64]
```

cfg-if, libc and typenum have no additional active edges. All of these paths
are inside ZXS. Inactive optional/dev declarations are preserved, not silently
vendored or enabled; any change of feature set needs a new closure review.
Local path packages have no registry checksum field in Cargo.lock; use
SOURCES.toml to check their published origin. The highest imported minimum is
Rust 1.88 (sha2, edition 2024), below ZXS's existing Rust 1.100 / pinned
1.100.0. [Debian 13's stock rustc](https://packages.debian.org/trixie/rustc)
is older than the pre-existing project toolchain; this does not change ZXS's
compiler or authorize a toolchain install.

Scope here is source and manifest review against [Cargo resolver rules](https://doc.rust-lang.org/cargo/reference/resolver.html)
and static agreement of all enabled normal/build/target edges with the path
lock. Cargo resolution/build, build scripts, project tests, real clients and
release artifacts were not executed or verified. No other project's config,
crate source, network settings or service entry is changed.
