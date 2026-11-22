# Private SMB compile-time selector

Alex Crichton and the cfg-if contributors' selector; see
[Apache 2.0](LICENSE-APACHE), [MIT](LICENSE-MIT), and the [source inventory](../../../vendor/rust/SOURCES.toml).
The original source blob is `2c3a7ab8600b4ddc44c6abb5c4defc6c2a411d79`.

ZXS scopes the macro to SMB and reuses it for the AES and handshake hash
backends. The recursion, first-match predicates and grouped expansion
are unchanged.
