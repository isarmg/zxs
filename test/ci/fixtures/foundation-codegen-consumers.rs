use proc_macro2::Span;
use proc_macro2_diagnostics::SpanDiagnosticExt;

#[test]
fn displaydoc_preserves_borrowed_values_and_numeric_error_context() {
    /// invalid length: expected {expected}, got {actual}
    #[derive(displaydoc::Display)]
    struct LengthError {
        expected: usize,
        actual: usize,
    }

    #[derive(displaydoc::Display)]
    enum LookupError<'a> {
        /// unknown component `{0}`
        Unknown(&'a str),
    }

    assert_eq!(
        LengthError {
            expected: 32,
            actual: 31
        }
        .to_string(),
        "invalid length: expected 32, got 31"
    );
    assert_eq!(
        LookupError::Unknown("文件").to_string(),
        "unknown component `文件`"
    );
}

#[test]
fn diagnostics_escape_messages_as_literals_and_keep_child_context() {
    let message = "invalid \"name\"\\path\n版本";
    let joined = Span::call_site()
        .error(message)
        .help("use an existing field")
        .emit_as_item_tokens();
    let joined: rust_syntax::ItemMacro = rust_syntax::parse2(joined).unwrap();
    let joined_message = rust_syntax::parse2::<rust_syntax::LitStr>(joined.mac.tokens)
        .unwrap()
        .value();
    assert_eq!(
        joined_message,
        format!("{message}\n= help: use an existing field")
    );

    let diagnostic = Span::call_site()
        .error(message)
        .span_help(Span::call_site(), "use an existing field");
    assert_eq!(diagnostic.children().count(), 1);
    let tokens = diagnostic.emit_as_item_tokens();
    let file: rust_syntax::File = rust_syntax::parse2(tokens).unwrap();
    let messages: Vec<_> = file
        .items
        .into_iter()
        .map(|item| {
            let rust_syntax::Item::Macro(item) = item else {
                panic!("expected a compile_error item")
            };
            assert!(item.mac.path.segments.last().unwrap().ident == "compile_error");
            rust_syntax::parse2::<rust_syntax::LitStr>(item.mac.tokens)
                .unwrap()
                .value()
        })
        .collect();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0], message);
    assert!(messages[1].contains("use an existing field"));
}

#[test]
fn source_formatter_preserves_expression_precedence_and_reparses_its_output() {
    let original = rust_syntax::parse_file("fn scale(x:i32)->i32{(x+1)*2}").unwrap();
    let formatted = rust_source_format::unparse(&original);
    assert_eq!(formatted, "fn scale(x: i32) -> i32 {\n    (x + 1) * 2\n}\n");
    let reparsed = rust_syntax::parse_file(&formatted).unwrap();
    assert_eq!(rust_source_format::unparse(&reparsed), formatted);
}
