pub(crate) trait RegexEngine: Sized + Send + Sync {
    type Error: RegexError;
    fn is_match(&self, text: &str) -> Result<bool, Self::Error>;
}

/// Reason a regex match failed, distinguishing real engine errors from recovered panics.
#[derive(Debug)]
pub(crate) enum RegexFailureReason {
    /// Real `fancy-regex` runtime error (e.g., the configured backtrack limit was hit).
    FancyRegex(fancy_regex::Error),
    /// Engine panicked during matching and was recovered via `catch_unwind`.
    Panicked,
}

pub(crate) trait RegexError {
    fn into_failure_reason(self) -> RegexFailureReason;
}

/// Failure mode for the `fancy-regex` backend: either a real engine error or a recovered panic.
#[derive(Debug)]
pub(crate) enum FancyRegexError {
    Engine(fancy_regex::Error),
    Panicked,
}

impl RegexError for FancyRegexError {
    fn into_failure_reason(self) -> RegexFailureReason {
        match self {
            Self::Engine(e) => RegexFailureReason::FancyRegex(e),
            Self::Panicked => RegexFailureReason::Panicked,
        }
    }
}

impl RegexEngine for fancy_regex::Regex {
    type Error = FancyRegexError;

    fn is_match(&self, text: &str) -> Result<bool, Self::Error> {
        // `regex-automata` 0.4 panics on some patterns (https://github.com/rust-lang/regex/issues/1344); catch to surface a regular error instead of aborting the host process.
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fancy_regex::Regex::is_match(self, text)
        })) {
            Ok(Ok(matched)) => Ok(matched),
            Ok(Err(e)) => Err(FancyRegexError::Engine(e)),
            Err(_) => Err(FancyRegexError::Panicked),
        }
    }
}

/// Marker error for `regex::Regex::is_match` panics. The underlying `is_match` is otherwise infallible.
#[derive(Debug)]
pub(crate) struct RegexBackendPanic;

impl RegexError for RegexBackendPanic {
    fn into_failure_reason(self) -> RegexFailureReason {
        RegexFailureReason::Panicked
    }
}

impl RegexEngine for regex::Regex {
    type Error = RegexBackendPanic;

    fn is_match(&self, text: &str) -> Result<bool, Self::Error> {
        // Same panic as fancy-regex (https://github.com/rust-lang/regex/issues/1344); see that impl for context.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            regex::Regex::is_match(self, text)
        }))
        .map_err(|_| RegexBackendPanic)
    }
}

/// Uninhabited error type for literal matchers — matching never fails.
#[derive(Debug)]
pub(crate) enum LiteralMatchError {}

impl RegexError for LiteralMatchError {
    fn into_failure_reason(self) -> RegexFailureReason {
        match self {}
    }
}

/// [`RegexEngine`] for literal patterns — either `starts_with` (prefix) or `==` (exact).
pub(crate) enum LiteralMatcher {
    Prefix {
        literal: String,
    },
    Exact {
        exact: String,
    },
    /// `^(a|b|c)$` — linear scan over a small sorted array.
    Alternation {
        alternatives: Vec<String>,
    },
    /// `^\S*$` — no ECMA-262 whitespace characters.
    NoWhitespace,
}

impl RegexEngine for LiteralMatcher {
    type Error = LiteralMatchError;

    #[inline]
    fn is_match(&self, text: &str) -> Result<bool, Self::Error> {
        match self {
            Self::Prefix { literal } => Ok(text.starts_with(literal.as_str())),
            Self::Exact { exact } => Ok(text == exact.as_str()),
            Self::Alternation { alternatives } => {
                Ok(alternatives.iter().any(|a| a.as_str() == text))
            }
            Self::NoWhitespace => Ok(!contains_ecma_whitespace(text)),
        }
    }
}

/// Result of analyzing a regex pattern for literal-match optimizations.
#[derive(Debug, PartialEq)]
pub(crate) enum PatternOptimization {
    /// `^prefix` — use `starts_with(prefix)`.
    Prefix(String),
    /// `^exact$` — use `== exact`.
    Exact(String),
    /// `^(a|b|c)$` — linear scan over a small sorted array.
    Alternation(Vec<String>),
    /// `^\S*$` — no ECMA-262 whitespace characters.
    NoWhitespace,
}

pub(crate) use crate::pattern::contains_ecma_whitespace;

/// Build a fancy-regex matcher, applying engine limits; `Err(())` on a rejected pattern.
pub(crate) fn build_fancy_regex(
    translated: &str,
    backtrack_limit: Option<usize>,
    size_limit: Option<usize>,
    dfa_size_limit: Option<usize>,
) -> Result<fancy_regex::Regex, ()> {
    let mut builder = fancy_regex::RegexBuilder::new(translated);
    if let Some(limit) = backtrack_limit {
        builder.backtrack_limit(limit);
    }
    if let Some(limit) = size_limit {
        builder.delegate_size_limit(limit);
    }
    if let Some(limit) = dfa_size_limit {
        builder.delegate_dfa_size_limit(limit);
    }
    builder.build().map_err(|_| ())
}

/// Build a standard-regex matcher, applying engine limits; `Err(())` on a rejected pattern.
pub(crate) fn build_standard_regex(
    translated: &str,
    size_limit: Option<usize>,
    dfa_size_limit: Option<usize>,
) -> Result<regex::Regex, ()> {
    let mut builder = regex::RegexBuilder::new(translated);
    if let Some(limit) = size_limit {
        builder.size_limit(limit);
    }
    if let Some(limit) = dfa_size_limit {
        builder.dfa_size_limit(limit);
    }
    builder.build().map_err(|_| ())
}

/// Analyze a pattern and return a [`PatternOptimization`] if one applies, or `None` if a full
/// regex engine is required. Shares [`schema_validation::patterns::analyze_pattern`] with the codegen backend.
pub(crate) fn analyze_pattern(pattern: &str) -> Option<PatternOptimization> {
    Some(match crate::pattern::analyze_pattern(pattern)? {
        crate::pattern::PatternAnalysis::Prefix(prefix) => {
            PatternOptimization::Prefix(prefix.into_owned())
        }
        crate::pattern::PatternAnalysis::Exact(exact) => {
            PatternOptimization::Exact(exact.into_owned())
        }
        crate::pattern::PatternAnalysis::Alternation(alternatives) => {
            PatternOptimization::Alternation(alternatives)
        }
        crate::pattern::PatternAnalysis::NoWhitespace => PatternOptimization::NoWhitespace,
    })
}

/// Preserve a property-name match failure at its pattern and instance locations.
pub(crate) fn property_match_error<'i>(
    error: impl RegexError,
    schema_location: &crate::validation::paths::Location,
    location: &crate::validation::paths::LazyLocation,
    tracker: Option<&crate::validation::paths::RefTracker>,
    key: &str,
) -> crate::validation::ValidationError<'i> {
    use crate::validation::{LazyInstance, ValidationError};
    let instance = LazyInstance::Ready(std::borrow::Cow::Owned(json_codec::Value::String(
        key.into(),
    )));
    let evaluation_path =
        crate::validation::paths::capture_evaluation_path(tracker, schema_location);
    match error.into_failure_reason() {
        RegexFailureReason::FancyRegex(error) => ValidationError::backtrack_limit(
            schema_location.clone(),
            evaluation_path,
            location.into(),
            instance,
            error,
        ),
        RegexFailureReason::Panicked => ValidationError::regex_engine_failure(
            schema_location.clone(),
            evaluation_path,
            location.into(),
            instance,
            "Regex engine failed to evaluate property pattern".into(),
        ),
    }
}

#[cfg(test)]
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../test/foundation/schema-validation/upstream/jsonschema/unit/src/regex/tests_182.rs"
));
