# Private SHA-2 backend selection

This macro is Alex Crichton and the cfg-if contributors' `cfg-if` 1.0.5
implementation, [upstream commit `8b0a2b0f`](https://github.com/rust-lang/cfg-if/tree/8b0a2b0fc1fa3b3741b9e72895f390cf499fe36f).
Its original Git source blob was `2c3a7ab8600b4ddc44c6abb5c4defc6c2a411d79`,
from the reviewed archive SHA-256
`4e7648175b45a9a48536d676f68d918270699102aa8dab5496df06904c914600`.
See the original [VCS record](upstream-vcs.json), [README](README.upstream.md),
[Apache 2.0](LICENSE-APACHE), [MIT](LICENSE-MIT), and
[import inventory](../../../../vendor/rust/SOURCES.toml); nothing was refreshed.

ZXS retains the macro body with only its three recursive paths retargeted.
The private module no longer has a crate-level export. Its twelve existing
handshake call sites use that private path; predicates, nesting, order,
grouping and SHA-2 code are unchanged. The fixtures retain the upstream
multiple-items guard case and exercise first match, fallback, no fallback
and multiple statements inside a function. The separate `cpubits` macro
is not part of this move. No new compilation configuration or supported
architecture is implied.
