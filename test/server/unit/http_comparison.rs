use super::*;
use json_codec::json;
#[test]
fn comparison_body_preserves_shape_unknown_and_identity_error_order() {
    for value in [json!(null), json!([]), json!(7)] {
        assert_eq!(
            CompareBody::parse(value).err().unwrap().message,
            "expected object"
        );
    }
    assert_eq!(
        CompareBody::parse(json!({"unexpected":true}))
            .err()
            .unwrap()
            .message,
        "unknown field unexpected"
    );
    assert_eq!(
        CompareBody::parse(json!({"right_commit_id":"right"}))
            .err()
            .unwrap()
            .message,
        "missing string field left_commit_id"
    );
    for value in [
        json!(null),
        json!(false),
        json!(7),
        json!(""),
        json!([]),
        json!({}),
    ] {
        assert_eq!(
            CompareBody::parse(json!({"left_commit_id":"left","right_commit_id":value}))
                .err()
                .unwrap()
                .message,
            "missing string field right_commit_id"
        );
    }
    let body =
        CompareBody::parse(json!({"left_commit_id":"left","right_commit_id":"right"})).unwrap();
    assert_eq!(body.left, "left");
    assert_eq!(body.right, "right");
}
