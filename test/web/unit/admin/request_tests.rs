use super::*;
#[test]
fn new_repository_defaults_to_one_actual_copy_and_preserves_explicit_choices() {
    let mut fields = initial_fields(Kind::Repository);
    fields.insert("name".into(), "one disk".into());
    fields.insert("slot".into(), "P01".into());
    let (_, path, body) = build(Kind::Repository, &fields, "operation").unwrap();
    assert_eq!(path, "/api/repositories");
    assert_eq!(body["target_copies"], 1);
    fields.insert("copies".into(), "3".into());
    assert_eq!(
        build(Kind::Repository, &fields, "operation").unwrap().2["target_copies"],
        3
    );
    fields.insert("copies".into(), "0".into());
    assert!(build(Kind::Repository, &fields, "operation").is_err());
    assert_eq!(initial_fields(Kind::Policy)["copies"], "1");
    assert!(
        initial_fields(Kind::Save).is_empty(),
        "historical saves retain inherited policy unless explicitly overridden"
    );
}

#[test]
fn inspection_cannot_build_a_mutation_request() {
    assert!(build(Kind::Inspect, &Fields::new(), "operation").is_err());
}

#[test]
fn invalid_numeric_fields_retain_their_parser_source() {
    use std::error::Error as _;
    let fields = [
        ("name".into(), "repo".into()),
        ("slot".into(), "P01".into()),
        ("copies".into(), "NaN".into()),
    ]
    .into_iter()
    .collect();
    assert!(
        build(Kind::Repository, &fields, "operation")
            .unwrap_err()
            .source()
            .is_some()
    );
    let fields = [
        ("repository".into(), "repo".into()),
        ("branch".into(), "b".into()),
        ("generation".into(), "18446744073709551616".into()),
    ]
    .into_iter()
    .collect();
    assert!(
        build(Kind::Smb, &fields, "operation")
            .unwrap_err()
            .source()
            .is_some()
    );
}
