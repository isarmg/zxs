//! The oracle digest was generated from cexpr 0.6.0 + nom 7.1.3 at aa7ffc46.
//! It includes Debug-rendered values, remaining slices, and error categories.
#[cfg(parser_reference)]
extern crate cexpr;
#[cfg(not(parser_reference))]
#[path = "../../../src/foundation/tooling/bindgen/src/cexpr/mod.rs"]
mod cexpr;
#[allow(dead_code)]
#[path = "../../../src/foundation/tooling/bindgen/src/cexpr/parser.rs"]
mod parser;
use std::cell::Cell;
use std::collections::HashMap;
thread_local! { static HASH: Cell<u64> = const { Cell::new(14695981039346656037) }; }
fn record(value: &str) {
    HASH.with(|hash| {
        let mut h = hash.get();
        for b in value.bytes().chain([0xff]) {
            h = (h ^ u64::from(b)).wrapping_mul(1099511628211);
        }
        hash.set(h);
    });
}
fn literal(input: &[u8]) {
    record(&format!("{:?}", cexpr::literal::parse(input)));
}
fn tokens(input: &[(u8, &[u8])]) {
    let tokens: Vec<_> = input
        .iter()
        .map(|(k, s)| {
            (
                match k {
                    0 => cexpr::token::Kind::Punctuation,
                    1 => cexpr::token::Kind::Literal,
                    _ => cexpr::token::Kind::Identifier,
                },
                *s,
            )
                .into()
        })
        .collect();
    record(&format!("{:?}", cexpr::expr::expr(&tokens)));
    record(&format!(
        "{:?}",
        cexpr::expr::IdentifierParser::new(&HashMap::new()).macro_definition(&tokens)
    ));
    record(&format!("{:?}", cexpr::expr::fn_macro_declaration(&tokens)));
}
#[test]
fn baseline_differential_corpus() {
    let explicit = [
        "",
        "0",
        "0x",
        "0b",
        "0xFFFFFFFFFFFFFFFFULL",
        "18446744073709551615",
        "18446744073709551616",
        "077",
        "09",
        "1.0e3",
        "1e3",
        ".1e3",
        "1.e3",
        "2.5",
        "2f",
        "0x1p1",
        "'\\uD800'",
        "'\\U00110000'",
        "'\\777'",
        "\"a\\xFFb\"",
        "\"abc",
        "'",
        "'a",
        "u8\"x\"",
        "L'a'",
    ];
    for s in explicit {
        literal(s.as_bytes());
    }
    for n in 0..=1024u64 {
        for s in [
            format!("{n}"),
            format!("{n}ULL"),
            format!("0x{n:x}"),
            format!("0{n:o}"),
            format!("0b{n:b}"),
            format!("{n}.5"),
            format!("{n}e+3"),
            format!("'\\{n:o}'"),
            format!("\"\\u{n:04x}\""),
        ] {
            literal(s.as_bytes());
        }
    }
    let alphabet = b"01xbuULfF.eE+-\\'\"a789";
    let mut seed = 0x123456789abcdefu64;
    for _ in 0..100000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let len = (seed % 20) as usize;
        let mut v = Vec::new();
        for _ in 0..len {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            v.push(alphabet[(seed >> 32) as usize % alphabet.len()]);
        }
        literal(&v);
    }
    let atoms: [(u8, &[u8]); 17] = [
        (1, b"0"),
        (1, b"2"),
        (1, b"2.5"),
        (1, b"\"a\""),
        (1, b"'x'"),
        (2, b"NAME"),
        (0, b"+"),
        (0, b"-"),
        (0, b"*"),
        (0, b"&"),
        (0, b"<<"),
        (0, b"("),
        (0, b")"),
        (0, b","),
        (0, b"~"),
        (0, b"^"),
        (0, b"|"),
    ];
    for _ in 0..50000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let len = (seed % 12) as usize;
        let mut v = Vec::new();
        for _ in 0..len {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            v.push(atoms[(seed >> 32) as usize % atoms.len()]);
        }
        tokens(&v);
    }
    println!("oracle: {:016x}", HASH.with(Cell::get));
    assert_eq!(HASH.with(Cell::get), 0xf6f717d146a860bd);
}

fn expression(parts: &[&str]) -> cexpr::expr::EvalResult {
    let tokens: Vec<_> = parts
        .iter()
        .map(|text| {
            let kind = if text.as_bytes()[0].is_ascii_digit() {
                cexpr::token::Kind::Literal
            } else {
                cexpr::token::Kind::Punctuation
            };
            (kind, text.as_bytes()).into()
        })
        .collect();
    cexpr::assert_full_parse(cexpr::expr::expr(&tokens))
        .unwrap()
        .1
}

#[test]
fn arithmetic_precedence_wrapping_and_float_promotion() {
    use cexpr::expr::EvalResult::{Float, Int};
    use std::num::Wrapping;
    for (parts, expected) in [
        (vec!["2", "+", "3", "*", "4"], 14),
        (vec!["(", "2", "+", "3", ")", "*", "4"], 20),
        (vec!["20", "/", "3", "%", "4"], 2),
        (vec!["10", "-", "3", "-", "2"], 5),
        (vec!["1", "<<", "5", ">>", "2"], 8),
        (vec!["6", "&", "3", "^", "1", "|", "8"], 11),
        (vec!["~", "0"], -1),
        (vec!["-", "8", ">>", "1"], -4),
        (vec!["+", "8"], 8),
        (vec!["18446744073709551615ULL", "+", "1"], 0),
        (vec!["9223372036854775807", "+", "1"], i64::MIN),
        (vec!["9223372036854775808", "/", "-", "1"], i64::MIN),
        (vec!["9223372036854775808", "%", "-", "1"], 0),
    ] {
        assert_eq!(expression(&parts), Int(Wrapping(expected)), "{parts:?}");
    }
    assert_eq!(expression(&["2", "+", "0.5"]), Float(2.5));
    assert_eq!(expression(&["7.5", "%", "2"]), Float(1.5));
    assert_eq!(expression(&["7.5", "/", "2"]), Float(3.75));
}

fn integer_zero_divisor_macro(operator: &[u8]) -> Vec<cexpr::token::Token> {
    use cexpr::token::Kind::{Identifier, Literal, Punctuation};
    vec![
        (Identifier, b"BAD".as_slice()).into(),
        (Literal, b"1".as_slice()).into(),
        (Punctuation, operator).into(),
        (Literal, b"0".as_slice()).into(),
    ]
}

#[test]
fn integer_division_by_zero_is_a_macro_parse_error() {
    let tokens = integer_zero_divisor_macro(b"/");
    assert!(cexpr::expr::macro_definition(&tokens).is_err());
}

#[test]
fn integer_remainder_by_zero_is_a_macro_parse_error() {
    let tokens = integer_zero_divisor_macro(b"%");
    assert!(cexpr::expr::macro_definition(&tokens).is_err());
}

#[test]
fn identifiers_function_macros_and_string_concatenation() {
    use cexpr::expr::{EvalResult, IdentifierParser};
    use cexpr::token::Kind::{Identifier, Literal, Punctuation};
    use std::num::Wrapping;
    let names = HashMap::from([
        (b"VALUE".to_vec(), EvalResult::Int(Wrapping(7))),
        (b"PREFIX".to_vec(), EvalResult::Str(b"pre".to_vec())),
    ]);
    let parser = IdentifierParser::new(&names);
    let tokens: Vec<_> = [
        (Identifier, &b"ANSWER"[..]),
        (Identifier, b"VALUE"),
        (Punctuation, b"*"),
        (Literal, b"6"),
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    assert_eq!(
        parser.macro_definition(&tokens).unwrap().1,
        (b"ANSWER".as_slice(), EvalResult::Int(Wrapping(42)))
    );
    let tokens: Vec<_> = [
        (Identifier, &b"TEXT"[..]),
        (Identifier, b"PREFIX"),
        (Literal, b"\"fix\""),
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    assert_eq!(
        parser.macro_definition(&tokens).unwrap().1 .1,
        EvalResult::Str(b"prefix".to_vec())
    );
    let tokens: Vec<_> = [
        (Identifier, &b"FUNC"[..]),
        (Punctuation, b"("),
        (Identifier, b"a"),
        (Punctuation, b","),
        (Identifier, b"b"),
        (Punctuation, b")"),
        (Identifier, b"a"),
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    let (rest, (name, args)) = cexpr::expr::fn_macro_declaration(&tokens).unwrap();
    assert_eq!(name, b"FUNC");
    assert_eq!(args, vec![b"a", b"b"]);
    assert_eq!(rest, &tokens[6..]);
}

#[test]
fn literal_widths_escapes_and_rejections() {
    use cexpr::expr::EvalResult::{Char, Float, Int, Str};
    use cexpr::literal::CChar;
    use std::num::Wrapping;
    for (raw, expected) in [
        ("0xffUL", Int(Wrapping(255))),
        ("077", Int(Wrapping(63))),
        ("0b101LL", Int(Wrapping(5))),
        ("18446744073709551615ULL", Int(Wrapping(-1))),
        ("1e3", Float(1000.0)),
        ("2.5f", Float(2.5)),
        ("L'\\777'", Char(CChar::Raw(511))),
        ("'\\n'", Char(CChar::Char('\n'))),
        ("u8\"\\u03a9\\xff\"", Str(vec![0xce, 0xa9, 0xff])),
    ] {
        assert_eq!(
            cexpr::literal::parse(raw.as_bytes()).unwrap().1,
            expected,
            "{raw}"
        );
    }
    for raw in [
        "18446744073709551616",
        "'\\uD800'",
        "'\\U00110000'",
        "garbage",
        "\"unterminated",
        "0x",
        "1garbage",
    ] {
        assert!(cexpr::literal::parse(raw.as_bytes()).is_err(), "{raw}");
    }
}

#[test]
fn callback_token_shape_comments_and_invalid_kinds() {
    use cexpr::token::{Kind, Token};
    let mut tokens = vec![
        Token {
            kind: Kind::Comment,
            raw: b"/*x*/".to_vec().into_boxed_slice(),
        },
        (Kind::Literal, &b"42"[..]).into(),
    ];
    assert!(cexpr::expr::expr(&tokens).is_err());
    cexpr::token::remove_comments(&mut tokens);
    assert_eq!(tokens.len(), 1);
    assert!(cexpr::expr::expr(&tokens).is_ok());
    tokens[0].kind = Kind::Keyword;
    assert!(cexpr::expr::expr(&tokens).is_err());
}

#[test]
fn incomplete_errors_backtracking_and_empty_progress() {
    use parser::error::{Error, ErrorKind};
    use parser::{Err, IResult, Needed};
    fn incomplete(i: &[u8]) -> IResult<&[u8], u8> {
        let _ = i;
        Err(Err::Incomplete(Needed::new(1)))
    }
    fn fail(i: &[u8]) -> IResult<&[u8], u8> {
        Err(Err::Failure(Error::new(i, ErrorKind::Fix)))
    }
    fn empty(i: &[u8]) -> IResult<&[u8], u8> {
        Ok((i, 0))
    }
    let input = b"x".as_slice();
    assert!(matches!(
        parser::branch::alt((incomplete, empty))(input),
        Err(Err::Incomplete(_))
    ));
    assert!(matches!(
        parser::branch::alt((fail, empty))(input),
        Err(Err::Failure(_))
    ));
    assert!(matches!(
        parser::combinator::complete(incomplete)(input),
        Err(Err::Error(_))
    ));
    assert!(parser::multi::many0(empty)(input).is_err());
    assert!(parser::multi::many1(empty)(input).is_err());
    assert!(parser::multi::many_m_n(1, 3, empty)(input).is_err());
    assert!(parser::multi::separated_list0(empty, empty)(input).is_err());
    assert_eq!(
        parser::combinator::opt(parser::bytes::complete::tag("z"))(input),
        Ok((input, None))
    );
}
