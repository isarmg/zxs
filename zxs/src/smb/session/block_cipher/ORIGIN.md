# Origin of the private SMB AES-128 module

Adapted from the RustCrypto Developers' AES source, upstream commit
[9a84f2f8b581f62bcd00e16b5e11a2f6251916ee](https://github.com/RustCrypto/block-ciphers/tree/9a84f2f8b581f62bcd00e16b5e11a2f6251916ee/aes).
The original archive and SHA-256 are recorded in
[the source inventory](../../../../vendor/rust/SOURCES.toml). The original
[MIT](LICENSE-MIT) and [Apache-2.0](LICENSE-APACHE) license files and source
attribution are retained. Source is part of ZXS; these references document
origin, not a build dependency or an upstream update procedure.

The former package was built without default features and with erasure
enabled. ZXS retains the AES-128 general block implementation actually
used by SMB CMAC, preserving key expansion, round operations, CPU dispatch
and the constant-time portable implementation. Bulk backend and compact
software settings are retained. The same fixed-size object is erased
unconditionally through the previously enabled `zeroize_flat_type` and
continues to implement `ZeroizeOnDrop`. The generic cipher trait and CPU
support dependencies are separate remaining migration work.

The public hardware-reporting helper, the encrypt-only/decrypt-only type
variants, unused AES-192/AES-256 APIs, and disabled hazmat paths are not
part of the private ZXS module. This does not add SMB encryption or a
general cryptography interface.

| Original Rust source | Original ZXS Git blob | Integration |
| --- | --- | --- |
| `src/backends.rs` | `c6c1aaa68d85b74ab9747bda09cd42132882f257` | unchanged |
| `src/backends/aarch64_aes.rs` | `592d2fff829801d300b543d0f3df2f1ed598367c` | adapted |
| `src/backends/aarch64_aes/encdec.rs` | `7f5c290b877e6dbcb4d3ea7545f74603c8d3f88d` | adapted |
| `src/backends/aarch64_aes/expand.rs` | `8b01f8d87683eaec5120ea33105efdc1f531c9a6` | unchanged |
| `src/backends/aarch64_aes/hazmat.rs` | `bec5c38dc65f309c62d2104de4c60bf48d9459bc` | not selected |
| `src/backends/aarch64_aes/utils.rs` | `5c6a9cca27fdb042ecc9b4e37c4ce099d3f16253` | adapted |
| `src/backends/fixslice/aes128.rs` | `21efc8bb311d1463e49f1933a1857e8e3fd32bad` | unchanged |
| `src/backends/fixslice/aes192.rs` | `d1bb0117fcfec77a6e7738a7bc866ccb83b15d80` | not selected |
| `src/backends/fixslice/aes256.rs` | `82576d977541d48c84643bc2d4a1de7bd74eb788` | not selected |
| `src/backends/fixslice/hazmat.rs` | `1cddc34a64293880e30de15aae37b2cba98d70f0` | not selected |
| `src/backends/fixslice/mix_columns.rs` | `60aad2da8aa50dcfa38908fdcee5efef0b3f2faa` | unchanged |
| `src/backends/fixslice/mod.rs` | `b9ace40ab4d2cb155766672c47ae1694e89f5b72` | adapted |
| `src/backends/fixslice/sbox.rs` | `2a2e104aca4b315b25c1eabc3751f9d11e056d25` | unchanged |
| `src/backends/fixslice/utils.rs` | `fe38f495df93f5c18d13859ea5b71a62603e596a` | adapted |
| `src/backends/fixslice/word.rs` | `696edc6cb90907874fc3c0d5398022d7390a51db` | adapted |
| `src/backends/soft.rs` | `7487ad458a7601efaba921b867615fab5bf8ba39` | adapted |
| `src/backends/x86_aes.rs` | `a329db3248d7e8316f58a2bffc3c52a974b527d6` | adapted |
| `src/backends/x86_aes/encdec.rs` | `458b8012d8888778a46d27ccd08be94d1d586fdd` | adapted |
| `src/backends/x86_aes/expand.rs` | `9eeac1eb45b435af1396a044dc25b827290305e7` | adapted |
| `src/backends/x86_aes/hazmat.rs` | `07c5485ecf3d6bb6ebdaae8f013d57a10ea5eb4b` | not selected |
| `src/backends/x86_aes/utils.rs` | `924d0dd59f5889da18d6e63dbcdde969fd351744` | adapted |
| `src/backends/x86_vaes256.rs` | `5c40a3dfa633a648caf71a12d5f7c6b83afd3cf3` | adapted |
| `src/backends/x86_vaes256/encdec.rs` | `3fa9b79e1b868d654ec400be24e9fe296808a277` | adapted |
| `src/backends/x86_vaes512.rs` | `bf0974e98fd392f1b14baadc0cb17c44b3bcc805` | adapted |
| `src/backends/x86_vaes512/encdec.rs` | `b2d6bcdc5dfad4630a68cbad5b0c0b67457b524f` | adapted |
| `src/hazmat.rs` | `373340685850b4e67be8281b3f20616aeb684af6` | not selected |
| `src/lib.rs` | `5b52c98fe98f4e265151dd0d7270d507aa597491` | adapted |
