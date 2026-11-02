use super::*;
use json_codec::json;

#[test]
fn note_body_preserves_plain_text_and_requires_explicit_cas_fields() {
    let request = json!({"expected_disk_id":"disk_id", "expected_note":"old\nnote", "note":"  plain\ntext\t\u{0000}  "});
    assert_eq!(
        parse(request.clone()).unwrap().note,
        "  plain\ntext\t\u{0000}  "
    );
    for field in ["expected_disk_id", "expected_note", "note"] {
        let mut value = request.clone();
        value.as_object_mut().unwrap().remove(field);
        assert_eq!(
            parse(value).unwrap_err().status,
            http::StatusCode::BAD_REQUEST
        );
        let mut value = request.clone();
        value[field] = json!(null);
        assert_eq!(
            parse(value).unwrap_err().status,
            http::StatusCode::BAD_REQUEST
        );
    }
    for value in [
        json!(["disk_id", "old", "new"]),
        json!(null),
        json!("note"),
        json!({"expected_disk_id":"disk_id","expected_note":"","note":"","physical_slot":"01"}),
    ] {
        assert_eq!(
            parse(value).unwrap_err().status,
            http::StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        parse(json!({"expected_disk_id":"disk_id", "expected_note":"", "note":""}))
            .unwrap()
            .note,
        ""
    );
}
