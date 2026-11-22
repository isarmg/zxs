# Portable AES word selection

The fixed priority and ARMv7/WebAssembly promotion in `mod.rs` come from
the RustCrypto Developers' CPU word-size selector, original source blob
`6589bb0bfb2fb22f9544c0e0ed9027a9c3077d7f`. See the [source inventory](../../../../../../vendor/rust/SOURCES.toml)
for its archive, commit and SHA-256. The original [MIT](NATIVE-WORD-LICENSE-MIT)
and [Apache-2.0](NATIVE-WORD-LICENSE-APACHE) notices remain.

The used predicates now select one private word type through the existing
ZXS compile-time selector. The public package macros and unused interfaces
are not needed. The selected type, fallback, and AES operations are unchanged.
