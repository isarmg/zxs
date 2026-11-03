//! Existing regression suites that do not require the absent upstream regex-test provider.
#[path = "../foundation/regex/tests/misc.rs"]
mod misc;
#[path = "../foundation/regex/tests/regression.rs"]
mod regression;
#[path = "../foundation/regex/tests/regression_fuzz.rs"]
mod regression_fuzz;
#[path = "../foundation/regex/tests/replace.rs"]
mod replace;

#[cfg(feature = "dfa-build")]
mod byteset_roundtrip;
