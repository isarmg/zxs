use super::{Diagnostic, Level};
use proc_macro2::Span;

#[test]
fn diagnostic_levels_require_exact_names() {
    for level in [Level::Error, Level::Warning, Level::Note, Level::Help] {
        assert_eq!(level.to_string().parse(), Ok(level));
    }

    for name in ["noterror", "warnings", "noteworthy", "unhelpful", " error", "note "] {
        assert_eq!(name.parse::<Level>(), Err(()), "{name:?}");
    }
}

#[test]
fn ordinary_error_labels_survive_diagnostic_conversion() {
    for message in [
        "[noterror] literal diagnostic",
        "[warnings] literal diagnostic",
        "[noteworthy] literal diagnostic",
        "[unhelpful] literal diagnostic",
    ] {
        let original = rust_syntax::Error::new(Span::call_site(), message);
        let diagnostic = Diagnostic::from(original);
        let restored = rust_syntax::Error::from(diagnostic);
        assert_eq!(restored.to_string(), message);
    }
}
