mod common;

#[test]
fn no_stack_overflow() {
    let mut s = "(module (func \n".to_string();
    for _ in 0..10_000 {
        s.push_str("(i32.add\n");
    }
    assert!(common::parse_wat(&s).is_err());
}

#[test]
fn component_text_is_rejected() {
    let error = common::parse_wat("(component)").unwrap_err();
    assert!(error
        .to_string()
        .contains("component text is outside the core parser scope"));
}
