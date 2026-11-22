# SMB tag comparison

The byte-slice adapter in `mac.rs` comes from the RustCrypto Developers'
constant-time utilities. The [source inventory](../../../vendor/rust/SOURCES.toml)
records the original archive and revision. The original
[MIT](TAG-COMPARE-LICENSE-MIT) and [Apache-2.0](TAG-COMPARE-LICENSE-APACHE)
notices remain.

The adapter retains the original byte-slice conditional move, zero initial
choice, input one, and optimization barrier before conversion to a boolean.
The [comparison backend](../tag_choice/ORIGIN.md), public tag length checks,
and tag erasure are unchanged.

| Original used source | Original Git blob |
| --- | --- |
| `src/choice.rs` | `a631419939d46307fde3c7b029979c22dd3328bb` |
| `src/traits/ct_eq.rs` | `f30b8f060d03d87b6768d3cc7ce574cf926c5d60` |
