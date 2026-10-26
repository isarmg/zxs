mod common;

use std::error::Error as _;
use wasm_text_printer::Error;

const ADD: &str = "(module (func (param i32 i32) (result i32) local.get 0 local.get 1 i32.add))";

#[test]
fn plain_printing_is_uncolored_and_unfolded() {
    let bytes = common::parse_wat(ADD).unwrap();
    assert_eq!(
        wasm_text_printer::print_bytes(&bytes).unwrap(),
        concat!(
            "(module\n",
            "  (type (;0;) (func (param i32 i32) (result i32)))\n",
            "  (func (;0;) (type 0) (param i32 i32) (result i32)\n",
            "    local.get 0\n    local.get 1\n    i32.add\n  )\n)\n"
        )
    );
}

#[test]
fn plain_printing_retains_legacy_exception_handling() {
    let bytes = common::parse_wat("(module (func try nop catch_all nop end))").unwrap();
    let text = wasm_text_printer::print_bytes(&bytes).unwrap();
    assert!(text
        .lines()
        .any(|line| line.trim_start().starts_with("try ")));
    assert!(text.contains("catch_all\n"));
    assert!(!text.contains('\u{1b}'));
}

#[test]
fn malformed_binary_is_rejected() {
    let error = wasm_text_printer::print_bytes(&[0, 97, 115, 109]).unwrap_err();
    let Error::Parse(ref parse) = error else {
        panic!("expected binary reader error")
    };
    let source = error
        .source()
        .unwrap()
        .downcast_ref::<wasmparser::BinaryReaderError>()
        .unwrap();
    assert!(std::ptr::eq(source, parse));
    assert_eq!(error.to_string(), parse.to_string());
}

#[test]
fn component_binary_is_rejected() {
    // Core module version is 01 00 00 00; 0d 00 01 00 is the component encoding.
    let error = wasm_text_printer::print_bytes(&[0, 97, 115, 109, 13, 0, 1, 0]).unwrap_err();
    assert!(matches!(error, Error::UnsupportedComponent));
    assert!(error.source().is_none());
}

#[test]
fn function_count_limit_is_typed() {
    // Function section declares 1,000,001 entries. Rejection precedes allocation.
    let error = wasm_text_printer::print_bytes(&[
        0, 97, 115, 109, 1, 0, 0, 0, 3, 3, 0xc1, 0x84, 0x3d, 10, 3, 0xc1, 0x84, 0x3d,
    ])
    .unwrap_err();
    assert!(
        matches!(error, Error::TooManyFunctions(1_000_001)),
        "{error:?}"
    );
    assert!(error.source().is_none());
}

#[test]
fn local_count_limit_is_typed() {
    let wat = format!("(module (func (local {})))", "i32 ".repeat(50_001));
    let bytes = common::parse_wat(&wat).unwrap();
    let error = wasm_text_printer::print_bytes(&bytes).unwrap_err();
    assert!(matches!(error, Error::TooManyLocals));
    assert!(error.source().is_none());
}

#[test]
fn code_without_function_declaration_is_rejected() {
    let bytes = [0, 97, 115, 109, 1, 0, 0, 0, 10, 4, 1, 2, 0, 11];
    let error = wasm_text_printer::print_bytes(&bytes).unwrap_err();
    let Error::Parse(ref source) = error else {
        panic!("expected the parser's function/code count rejection")
    };
    assert!(source.message().contains("function section is absent"));
    assert!(error.source().is_some());
}

#[test]
fn try_table_catch_targets_enclosing_function() {
    let bytes = common::parse_wat("(module (func try_table (catch_all 0) nop end))").unwrap();
    let text = wasm_text_printer::print_bytes(&bytes).unwrap();
    assert!(text.contains("(catch_all 0)"));
}

#[test]
fn truncated_branch_table_preserves_parser_error() {
    // Complete section/body lengths, but br_table lacks its default target.
    let bytes = [
        0, 97, 115, 109, 1, 0, 0, 0, 1, 4, 1, 96, 0, 0, 3, 2, 1, 0, 10, 6, 1, 4, 0, 14, 1, 0,
    ];
    let error = wasm_text_printer::print_bytes(&bytes).unwrap_err();
    assert!(matches!(error, Error::Parse(_)));
    assert!(error.source().is_some());
}
