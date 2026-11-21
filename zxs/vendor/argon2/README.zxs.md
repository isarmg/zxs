# Bundled Argon2 reference library

The 14 files listed in `SHA256SUMS` are byte-for-byte from the [official Password Hashing Competition Argon2 20190702 release](https://github.com/P-H-C/phc-winner-argon2/releases/tag/20190702), commit `62358ba2123abd17fccf2a108a301d4b52c01a7c`, tree `04128837520d3328497a6710c9c2696d09289c50`. The original `LICENSE` and copyright headers are retained (upstream offers CC0 1.0 or Apache 2.0). This imports the portable C reference sources and required headers, not the upstream CLI, benchmarks, generated tools, platform SIMD variants or prebuilt libraries. There are no changes to third-party crypto code. This is ZXS-owned source with an unversioned functional directory and no upstream updater; the original version is recorded here only for provenance.

All 14 files also match Debian 13's `argon2_0~20190702+dfsg.orig.tar.xz`, SHA-256 `815f1017b5b0033204778c2e32443257274bc5d9dd16d355c4f7db8189e23dcc`. The [Debian trixie source package has four build-system patches](https://sources.debian.org/patches/argon2/0~20190702%2Bdfsg-4/), all to its Makefile. Debian source and binary index entries were checked against their signed trixie InRelease; the data comparison is separate from any ZXS release authorization. The legacy synthetic interoperability vectors use Debian `libargon2-1_0~20190702+dfsg-4+b2_amd64.deb`, SHA-256 `a54a6640be69c29c1e43b14ee464484a6f20e33fe73200c02949cfeb03228547`, extracted only for comparison, not installed or used by the new build.

ZXS compiles the six library translation units with the existing target C compiler and links the private `libzxs_argon2.a`. The Web verifier still calls the same upstream `argon2id_verify` API, with [Argon2id v19](https://www.rfc-editor.org/rfc/rfc9106.html#section-3), 65,536 KiB, 3 passes, 4 lanes, the existing 16-byte salt and 32-byte tag, and identical unpadded PHC encoding. The worker threads, comparison, memory clearing, accepted password byte length and one-at-a-time verification guard are unchanged. There is no system libargon2 fallback, credential conversion, format change or SMB authentication change.

## Recheck this component

From `zxs/` with the audited Rust 1.100 toolchain in a separate disposable Linux environment (no system Argon2 development or runtime package required):

```sh
(cd vendor/argon2 && sha256sum -c SHA256SUMS)
cargo +1.100.0 test --locked --offline --lib bundled_argon2_accepts_exact_legacy_verifiers -- --test-threads=1
python3 build_release.py /path/to/reviewed/rust-1.100.0/bin
readelf -ldW target/standalone/zxs
```

The [standalone build](../../BUILD.md) checks the complete ELF interpreter and
dynamic dependency list. The Argon2 fixture and the loader check alone do not
establish cold launch without Helper, client qualification, or deployment.
