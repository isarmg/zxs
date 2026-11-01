use super::*;

type Expected = std::result::Result<&'static str, &'static [&'static str]>;

#[path = "prop_options_cases.rs"]
mod cases;

fn attributes(source: &str) -> Vec<Attribute> {
    Attribute::parse_outer.parse_str(source).unwrap()
}

fn snapshot<const COMPONENT: bool>(options: PropOptions<COMPONENT>) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{:?}|{}",
        options.optional,
        options.optional_no_strip,
        options.strip_option,
        options
            .default
            .map(|expr| expr.to_token_stream().to_string())
            .unwrap_or_default(),
        options.into,
        options.attrs,
        options.name.map(|name| name.to_string()),
        options.marker
    )
}

fn diagnostic(error: Error) -> Vec<String> {
    error.into_iter().map(|error| error.to_string()).collect()
}

// Captured from the previous parser on 794c3514. The generic derive dependency
// is deliberately absent: tests must not retain the retired build-time closure.
#[test]
fn accepted_values_and_diagnostics_match_the_reference_grammar() {
    assert_eq!(cases::CASES.len(), 269);
    for &(source, component, slot) in cases::CASES {
        let attrs = attributes(source);
        let owned = |expected: Expected| {
            expected.map(str::to_owned).map_err(|errors| {
                errors
                    .iter()
                    .map(|error| (*error).to_owned())
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(
            PropOptions::<true>::from_attributes(&attrs)
                .map(snapshot)
                .map_err(diagnostic),
            owned(component),
            "component: {source}"
        );
        assert_eq!(
            PropOptions::<false>::from_attributes(&attrs)
                .map(snapshot)
                .map_err(diagnostic),
            owned(slot),
            "slot: {source}"
        );
    }
}

#[test]
fn errors_point_to_the_relevant_input() {
    for (source, expected) in [
        ("#[prop(unknown)]", "unknown"),
        (
            "#[prop(optional = false, optional = true)]",
            "false, optional = true",
        ),
        (
            "#[prop(optional(false), optional(true))]",
            "false), optional(true",
        ),
        ("#[prop(optional, strip_option)]", "optional, strip_option"),
        ("#[prop(default = 1, default = 2)]", "1, default = 2"),
    ] {
        let error = PropOptions::<true>::from_attributes(&attributes(source)).unwrap_err();
        assert_eq!(
            error.span().source_text().as_deref(),
            Some(expected),
            "{source}"
        );
    }
}

#[test]
fn renamed_props_are_identifiers_with_the_literal_span() {
    for name in ["renamed", "r#type", "école", "e\u{301}cole", "中文"] {
        let literal = format!("{name:?}");
        let source = format!("#[prop(name = {literal})]");
        let options = PropOptions::<true>::from_attributes(&attributes(&source)).unwrap();
        let ident = options.name.unwrap();
        assert_eq!(ident.to_string(), name);
        assert_eq!(
            ident.span().source_text().as_deref(),
            Some(literal.as_str())
        );
    }
}

#[test]
fn invalid_renamed_props_report_the_literal_instead_of_panicking() {
    for name in [
        "",
        "two names",
        "not-a-name",
        "123",
        "type",
        "_",
        "r#self",
        " renamed",
        "renamed ",
        "/* comment */renamed",
        "renamed// comment",
    ] {
        let literal = format!("{name:?}");
        let source = format!("#[prop(name = {literal})]");
        let error = PropOptions::<true>::from_attributes(&attributes(&source)).unwrap_err();
        assert_eq!(
            error.span().source_text().as_deref(),
            Some(literal.as_str())
        );
        assert!(!error.to_string().is_empty());
    }
}

#[test]
fn typed_names_keep_duplicate_and_conflict_diagnostics() {
    for source in [
        "#[prop(name = \"r#type\", name = \"école\")]",
        "#[prop(name(\"r#type\"))] #[prop(name = \"école\")]",
    ] {
        let error = PropOptions::<true>::from_attributes(&attributes(source)).unwrap_err();
        assert_eq!(error.to_string(), "`name` is specified multiple times");
        let source = error.span().source_text().unwrap();
        assert!(source.contains("r#type"));
        assert!(source.contains("école"));
    }
    let error =
        PropOptions::<true>::from_attributes(&attributes("#[prop(name = \"r#type\", marker)]"))
            .unwrap_err();
    assert_eq!(
        error.to_string(),
        "`marker` conflicts with mutually exclusive `name`"
    );
}
