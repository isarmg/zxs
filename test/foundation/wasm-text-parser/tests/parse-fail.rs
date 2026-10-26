// The checked-in diagnostics are independent upstream expectations, never regenerated here.
mod common;

#[test]
fn parse_failure_diagnostics() {
    const CASES: &[&str] = &[
        "bad-index.wat",
        "bad-name.wat",
        "bad-name2.wat",
        "bad-name3.wat",
        "block1.wat",
        "block2.wat",
        "block3.wat",
        "confusing-block-comment0.wat",
        "confusing-block-comment1.wat",
        "confusing-block-comment2.wat",
        "confusing-block-comment3.wat",
        "confusing-block-comment4.wat",
        "confusing-block-comment5.wat",
        "confusing-block-comment6.wat",
        "confusing-block-comment7.wat",
        "confusing-block-comment8.wat",
        "confusing-line-comment0.wat",
        "confusing-line-comment1.wat",
        "confusing-line-comment2.wat",
        "confusing-line-comment3.wat",
        "confusing-line-comment4.wat",
        "confusing-line-comment5.wat",
        "confusing-line-comment6.wat",
        "confusing-line-comment7.wat",
        "confusing-line-comment8.wat",
        "confusing-string0.wat",
        "confusing-string1.wat",
        "confusing-string2.wat",
        "confusing-string3.wat",
        "confusing-string4.wat",
        "confusing-string5.wat",
        "confusing-string6.wat",
        "confusing-string7.wat",
        "confusing-string8.wat",
        "inline1.wat",
        "newline-in-string.wat",
        "string1.wat",
        "string10.wat",
        "string11.wat",
        "string12.wat",
        "string13.wat",
        "string14.wat",
        "string15.wat",
        "string16.wat",
        "string2.wat",
        "string3.wat",
        "string4.wat",
        "string5.wat",
        "string6.wat",
        "string7.wat",
        "string8.wat",
        "string9.wat",
        "unbalanced.wat",
    ];
    assert_eq!(CASES.len(), 53);
    let directory = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../test/foundation/wasm-text-parser/tests/parse-fail"
    ));
    for name in CASES {
        let text = std::fs::read_to_string(directory.join(name)).unwrap();
        let mut error = common::parse_wat(&text).expect_err(name);
        // Retain the diagnostic's original source identity independently of checkout location.
        error.set_path(&std::path::Path::new("tests/parse-fail").join(name));
        let expected = std::fs::read_to_string(directory.join(format!("{name}.err"))).unwrap();
        assert_eq!(
            format!("{error}\n"),
            expected.replace("\r\n", "\n"),
            "{name}"
        );
    }
}
