use super::*;

#[test]
fn action_uses_exact_position_scope_and_label_spelling() {
    let share = "11".repeat(16);
    let id = "33".repeat(16);
    let position = "44".repeat(16);
    let scope = "0a".repeat(32);
    let input = |p, v, l| action(&share, "-", &id, p, &scope, v, l);
    let (selected, op) = input(&position, "add", "436164").unwrap();
    assert_eq!(selected.position.to_string(), position);
    assert_eq!(selected.scope, [10; 32]);
    assert_eq!(op, Action::Set { name: Label::parse("Cad").unwrap(), present: true });
    assert_eq!(input(&position, "list", "-").unwrap().1, Action::List(None));
    assert_eq!(input(&position, "remove", "636164").unwrap().1,
        Action::Set { name: Label::parse("cad").unwrap(), present: false });
    for (p, v, l) in [("0", "add", "63"), (&position, "add", "-"), (&position, "new", "63"),
        (&position, "list", "0A"), (&position, "list", "00")] {
        assert!(input(p, v, l).is_none());
    }
    assert!(action(&share, "-", &id, &position, "11", "list", "-").is_none());
}

#[test]
fn response_never_places_label_text_in_markup_or_a_query() {
    let name = Label::parse("图纸 <&>").unwrap();
    let reply = response(Labels { names: vec![name.clone()], next: Some(name.clone()) }).unwrap();
    let encoded = name.encoded();
    let json = format!(r#"{{"labels":["{encoded}"],"next":"{encoded}"}}"#);
    assert_eq!(reply.body.as_ref(), json.as_bytes());
    assert_eq!(reply.ty, "application/json; charset=utf-8");
}
