use super::*;
use json_codec::json;

fn parse(value: json_codec::Value) -> Result<ResolveBody, ApiError> {
    ResolveBody::parse(value.as_object().unwrap())
}
#[test]
fn resolve_body_keeps_error_priority_and_caches_unicode_lookup_once() {
    for value in [
        json!({}),
        json!({"path":null}),
        json!({"path":""}),
        json!({"path":7}),
    ] {
        assert_eq!(
            parse(value).err().unwrap().message,
            "missing string field path"
        );
    }
    assert_eq!(
        parse(json!({"unknown":true,"path":"../bad","commit_id":null}))
            .err()
            .unwrap()
            .message,
        "unknown field unknown"
    );
    let malformed = parse(json!({"path":"../bad","commit_id":null}))
        .err()
        .unwrap();
    assert!(std::error::Error::source(&malformed).is_some());
    for identity in [
        json!(null),
        json!(false),
        json!(1),
        json!({}),
        json!([]),
        json!(""),
    ] {
        assert_eq!(
            parse(json!({"path":"file.txt","commit_id":identity}))
                .err()
                .unwrap()
                .message,
            "commit_id must be a nonempty string when supplied"
        );
    }
    let absent = parse(json!({"branch_id":"branch","path":"CAFE\u{301}.TXT"})).unwrap();
    assert_eq!(absent.path.key(), "café.txt");
    assert!(absent.commit_id.is_none());
    assert_eq!(
        parse(json!({"path":"file.txt","commit_id":"historical"}))
            .unwrap()
            .commit_id
            .as_deref(),
        Some("historical")
    );
}
