use wasm_text_parser::parser::{self, Parse, ParseBuffer, Parser, Peek};

mod without_result_import {
    wasm_text_parser::custom_reserved!(spaceship = "<=>");
    wasm_text_parser::custom_reserved!(Uppercase);

    #[test]
    fn both_forms_need_no_parser_result_import() {
        let symbol = super::parse_with_peek::<spaceship>("  <=>");
        assert_eq!(symbol.0.offset(), 2);

        let symbol = super::parse_with_peek::<Uppercase>("Uppercase");
        assert_eq!(symbol.0.offset(), 0);
    }
}

mod with_unrelated_result_alias {
    type Result<T> = std::result::Result<T, &'static str>;

    wasm_text_parser::custom_reserved!(spaceship = "<=>");
    wasm_text_parser::custom_reserved!(Uppercase);

    #[test]
    fn both_forms_ignore_the_callers_result_alias() -> Result<()> {
        let symbol = super::parse_with_peek::<spaceship>("  <=>");
        assert_eq!(symbol.0.offset(), 2);

        let symbol = super::parse_with_peek::<Uppercase>("Uppercase");
        assert_eq!(symbol.0.offset(), 0);
        Ok(())
    }
}

struct Peeked<T>(T);

impl<'a, T: Parse<'a> + Peek> Parse<'a> for Peeked<T> {
    fn parse(parser: Parser<'a>) -> parser::Result<Self> {
        assert!(parser.peek::<T>()?);
        assert!(parser.peek::<T>()?);
        Ok(Self(parser.parse()?))
    }
}

fn parse_with_peek<T: for<'a> Parse<'a> + Peek>(input: &str) -> T {
    let buffer = ParseBuffer::new(input).unwrap();
    parser::parse::<Peeked<T>>(&buffer).unwrap().0
}

#[test]
fn parse_rejects_mismatches_and_empty_input() {
    for input in ["=>", "module", "42", ""] {
        let buffer = ParseBuffer::new(input).unwrap();
        let error = parser::parse::<without_result_import::spaceship>(&buffer).unwrap_err();
        assert_eq!(error.message(), "expected reserved symbol `<=>`");
    }
}

#[test]
fn peek_leaves_mismatches_unconsumed_and_accepts_empty_input() {
    for input in ["=>", "module", "42"] {
        let buffer = ParseBuffer::new(input).unwrap();
        let error = parser::parse::<Option<without_result_import::spaceship>>(&buffer).unwrap_err();
        assert_eq!(error.message(), "extra tokens remaining after parse");
        assert_eq!(error.span().offset(), 0);
    }

    let buffer = ParseBuffer::new(" (; comment ;) ").unwrap();
    assert!(
        parser::parse::<Option<without_result_import::spaceship>>(&buffer)
            .unwrap()
            .is_none()
    );
}

#[test]
fn peek_preserves_lexer_errors() {
    let buffer = ParseBuffer::new("\"unterminated").unwrap();
    let error = parser::parse::<Option<without_result_import::spaceship>>(&buffer).unwrap_err();
    assert!(matches!(
        error.lex_error(),
        Some(wasm_text_parser::lexer::LexError::UnexpectedEof)
    ));
}
