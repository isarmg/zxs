use wasm_text_parser::{token::Span, Error};

fn assert_column(prefix: &str, rendered_prefix: &str, width: usize) {
    let text = format!("{prefix}?");
    let mut error = Error::new(Span::from_offset(prefix.len()), "unexpected token".into());
    error.set_text(&text);
    error.set_path(std::path::Path::new("diagnostic.wat"));
    assert_eq!(
        error.to_string(),
        format!("unexpected token\n     --> diagnostic.wat:1:{}\n      |\n    1 | {rendered_prefix}?\n      | {}^", width + 1, " ".repeat(width))
    );
}

#[test]
fn unicode_display_columns() {
    assert_column("é", "é", 1);
    assert_column("e\u{301}", "e\u{301}", 1);
    assert_column("界", "界", 2);
    assert_column("👩‍🔬", "👩‍🔬", 2);
}

#[test]
fn expanded_tab_columns() {
    assert_column("\t界", "    界", 6);
}

#[test]
fn removed_direction_controls_do_not_shift_columns() {
    assert_column("a\u{202e}界", "a界", 3);
}

#[test]
fn rendered_end_of_line_column() {
    let mut error = Error::new(Span::from_offset("\t界".len()), "missing token".into());
    error.set_text("\t界");
    error.set_path(std::path::Path::new("diagnostic.wat"));
    assert_eq!(
        error.to_string(),
        "missing token\n     --> diagnostic.wat:1:7\n      |\n    1 |     界\n      |       ^"
    );
}

#[test]
fn arbitrary_public_span_keeps_numeric_column_fallback() {
    let mut error = Error::new(Span::from_offset(1), "inside a UTF-8 character".into());
    error.set_text("é");
    error.set_path(std::path::Path::new("diagnostic.wat"));
    assert_eq!(
        error.to_string(),
        "inside a UTF-8 character\n     --> diagnostic.wat:1:2\n      |\n    1 | é\n      |  ^"
    );
}
