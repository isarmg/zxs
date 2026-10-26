use super::*;

#[test]
fn ws_smoke() {
    fn get_whitespace(input: &str) -> &str {
        let token = get_token(input);
        match token.kind {
            TokenKind::Whitespace => token.src(input),
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(get_whitespace(" "), " ");
    assert_eq!(get_whitespace("  "), "  ");
    assert_eq!(get_whitespace("  \n "), "  \n ");
    assert_eq!(get_whitespace("  x"), "  ");
    assert_eq!(get_whitespace("  ;"), "  ");
}

#[test]
fn line_comment_smoke() {
    fn get_line_comment(input: &str) -> &str {
        let token = get_token(input);
        match token.kind {
            TokenKind::LineComment => token.src(input),
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(get_line_comment(";;"), ";;");
    assert_eq!(get_line_comment(";; xyz"), ";; xyz");
    assert_eq!(get_line_comment(";; xyz\nabc"), ";; xyz");
    assert_eq!(get_line_comment(";;\nabc"), ";;");
    assert_eq!(get_line_comment(";;   \nabc"), ";;   ");
    assert_eq!(get_line_comment(";;   \rabc"), ";;   ");
    assert_eq!(get_line_comment(";;   \r\nabc"), ";;   ");
}

#[test]
fn block_comment_smoke() {
    fn get_block_comment(input: &str) -> &str {
        let token = get_token(input);
        match token.kind {
            TokenKind::BlockComment => token.src(input),
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(get_block_comment("(;;)"), "(;;)");
    assert_eq!(get_block_comment("(; ;)"), "(; ;)");
    assert_eq!(get_block_comment("(; (;;) ;)"), "(; (;;) ;)");
}

fn get_token(input: &str) -> Token {
    Lexer::new(input)
        .parse(&mut 0)
        .expect("no first token")
        .expect("no token")
}

#[test]
fn lparen() {
    assert_eq!(get_token("((").kind, TokenKind::LParen);
}

#[test]
fn rparen() {
    assert_eq!(get_token(")(").kind, TokenKind::RParen);
}

#[test]
fn strings() {
    fn get_string(input: &str) -> Vec<u8> {
        let token = get_token(input);
        match token.kind {
            TokenKind::String => token.string(input).to_vec(),
            other => panic!("not keyword {other:?}"),
        }
    }
    assert_eq!(&*get_string("\"\""), b"");
    assert_eq!(&*get_string("\"a\""), b"a");
    assert_eq!(&*get_string("\"a b c d\""), b"a b c d");
    assert_eq!(&*get_string("\"\\\"\""), b"\"");
    assert_eq!(&*get_string("\"\\'\""), b"'");
    assert_eq!(&*get_string("\"\\n\""), b"\n");
    assert_eq!(&*get_string("\"\\t\""), b"\t");
    assert_eq!(&*get_string("\"\\r\""), b"\r");
    assert_eq!(&*get_string("\"\\\\\""), b"\\");
    assert_eq!(&*get_string("\"\\01\""), &[1]);
    assert_eq!(&*get_string("\"\\u{1}\""), &[1]);
    assert_eq!(
        &*get_string("\"\\u{0f3}\""),
        '\u{0f3}'.encode_utf8(&mut [0; 4]).as_bytes()
    );
    assert_eq!(
        &*get_string("\"\\u{0_f_3}\""),
        '\u{0f3}'.encode_utf8(&mut [0; 4]).as_bytes()
    );

    for i in 0..=255i32 {
        let s = format!("\"\\{i:02x}\"");
        assert_eq!(&*get_string(&s), &[i as u8]);
    }
}

#[test]
fn id() {
    fn get_id(input: &str) -> String {
        let token = get_token(input);
        match token.kind {
            TokenKind::Id => token.id(input).unwrap().to_string(),
            other => panic!("not id {other:?}"),
        }
    }
    assert_eq!(get_id("$x"), "x");
    assert_eq!(get_id("$xyz"), "xyz");
    assert_eq!(get_id("$x_z"), "x_z");
    assert_eq!(get_id("$0^"), "0^");
    assert_eq!(get_id("$0^;;"), "0^");
    assert_eq!(get_id("$0^ ;;"), "0^");
    assert_eq!(get_id("$\"x\" ;;"), "x");
}

#[test]
fn annotation() {
    fn get_annotation(input: &str) -> String {
        let token = get_token(input);
        match token.kind {
            TokenKind::Annotation => token.annotation(input).unwrap().to_string(),
            other => panic!("not annotation {other:?}"),
        }
    }
    assert_eq!(get_annotation("@foo"), "foo");
    assert_eq!(get_annotation("@foo "), "foo");
    assert_eq!(get_annotation("@f "), "f");
    assert_eq!(get_annotation("@\"x\" "), "x");
    assert_eq!(get_annotation("@0 "), "0");
}

#[test]
fn keyword() {
    fn get_keyword(input: &str) -> &str {
        let token = get_token(input);
        match token.kind {
            TokenKind::Keyword => token.keyword(input),
            other => panic!("not keyword {other:?}"),
        }
    }
    assert_eq!(get_keyword("x"), "x");
    assert_eq!(get_keyword("xyz"), "xyz");
    assert_eq!(get_keyword("x_z"), "x_z");
    assert_eq!(get_keyword("x_z "), "x_z");
    assert_eq!(get_keyword("x_z "), "x_z");
}

#[test]
fn reserved() {
    fn get_reserved(input: &str) -> &str {
        let token = get_token(input);
        match token.kind {
            TokenKind::Reserved => token.reserved(input),
            other => panic!("not reserved {other:?}"),
        }
    }
    assert_eq!(get_reserved("^_x "), "^_x");
}

#[test]
fn integer() {
    fn get_integer(input: &str) -> String {
        let token = get_token(input);
        match token.kind {
            TokenKind::Integer(i) => token.integer(input, i).val.to_string(),
            other => panic!("not integer {other:?}"),
        }
    }
    assert_eq!(get_integer("1"), "1");
    assert_eq!(get_integer("0"), "0");
    assert_eq!(get_integer("-1"), "-1");
    assert_eq!(get_integer("+1"), "1");
    assert_eq!(get_integer("+1_000"), "1000");
    assert_eq!(get_integer("+1_0_0_0"), "1000");
    assert_eq!(get_integer("+0x10"), "10");
    assert_eq!(get_integer("-0x10"), "-10");
    assert_eq!(get_integer("0x10"), "10");
}

#[test]
fn float() {
    fn get_float(input: &str) -> Float<'_> {
        let token = get_token(input);
        match token.kind {
            TokenKind::Float(f) => token.float(input, f),
            other => panic!("not float {other:?}"),
        }
    }
    assert_eq!(
        get_float("nan"),
        Float::Nan {
            val: None,
            negative: false
        },
    );
    assert_eq!(
        get_float("-nan"),
        Float::Nan {
            val: None,
            negative: true,
        },
    );
    assert_eq!(
        get_float("+nan"),
        Float::Nan {
            val: None,
            negative: false,
        },
    );
    assert_eq!(
        get_float("+nan:0x1"),
        Float::Nan {
            val: Some("1".into()),
            negative: false,
        },
    );
    assert_eq!(
        get_float("nan:0x7f_ffff"),
        Float::Nan {
            val: Some("7fffff".into()),
            negative: false,
        },
    );
    assert_eq!(get_float("inf"), Float::Inf { negative: false });
    assert_eq!(get_float("-inf"), Float::Inf { negative: true });
    assert_eq!(get_float("+inf"), Float::Inf { negative: false });

    assert_eq!(
        get_float("1.2"),
        Float::Val {
            integral: "1".into(),
            fractional: Some("2".into()),
            exponent: None,
            hex: false,
        },
    );
    assert_eq!(
        get_float("1.2e3"),
        Float::Val {
            integral: "1".into(),
            fractional: Some("2".into()),
            exponent: Some("3".into()),
            hex: false,
        },
    );
    assert_eq!(
        get_float("-1_2.1_1E+0_1"),
        Float::Val {
            integral: "-12".into(),
            fractional: Some("11".into()),
            exponent: Some("01".into()),
            hex: false,
        },
    );
    assert_eq!(
        get_float("+1_2.1_1E-0_1"),
        Float::Val {
            integral: "12".into(),
            fractional: Some("11".into()),
            exponent: Some("-01".into()),
            hex: false,
        },
    );
    assert_eq!(
        get_float("0x1_2.3_4p5_6"),
        Float::Val {
            integral: "12".into(),
            fractional: Some("34".into()),
            exponent: Some("56".into()),
            hex: true,
        },
    );
    assert_eq!(
        get_float("+0x1_2.3_4P-5_6"),
        Float::Val {
            integral: "12".into(),
            fractional: Some("34".into()),
            exponent: Some("-56".into()),
            hex: true,
        },
    );
    assert_eq!(
        get_float("1."),
        Float::Val {
            integral: "1".into(),
            fractional: None,
            exponent: None,
            hex: false,
        },
    );
    assert_eq!(
        get_float("0x1p-24"),
        Float::Val {
            integral: "1".into(),
            fractional: None,
            exponent: Some("-24".into()),
            hex: true,
        },
    );
}
