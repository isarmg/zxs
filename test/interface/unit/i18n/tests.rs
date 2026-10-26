use super::*;
#[test]
fn format_parameters_are_literal_and_never_reparsed() {
    assert_eq!(tf("", "{0} / {1}", &["{1}", "second"]), "{1} / second");
    assert_eq!(tf("", "{0}{0} {2}", &["中文🦀"]), "中文🦀中文🦀 {2}");
    assert_eq!(
        tf("", "plain {invalid} {", &["unused"]),
        "plain {invalid} {"
    );
    assert_eq!(tf("", "{01} {1}", &["zero", "one"]), "{01} one");
}
#[test]
fn locale_admission_matches_current_contract() {
    assert_eq!(supported("zh-Hans-CN"), Some(Locale::Chinese));
    assert_eq!(supported("en-US"), Some(Locale::English));
    assert_eq!(supported("zh!"), None);
    assert_eq!(supported("en-"), None);
    assert_eq!(supported("fr"), None);
}
