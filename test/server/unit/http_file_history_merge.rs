use super::*;
use json_codec::json;

fn decode(body: Value) -> Result<MergeFileHistory, ApiError> {
    let Value::Object(body) = crate::with_id(body, "file_id", "file".into())? else {
        unreachable!("fixture binds an object")
    };
    parse(body)
}

#[test]
fn historical_merge_envelope_keeps_sources_heads_and_new_name_distinct() {
    let operation = storage_contract::OperationId::generate().unwrap();
    let body = json!({"inputs":[{"branch_id":"branch","source_node_id":"old","expected_node_id":"new"}],"chosen_node_id":"old","name":"Café","confirmed":true,"repository_id":"repo","path":"file.txt","operation_id":operation});
    let parsed = decode(body.clone()).unwrap();
    assert_eq!(parsed.inputs[0].source_node_id, "old");
    assert_eq!(parsed.inputs[0].expected_node_id, "new");
    assert_eq!(parsed.name.key(), "café");
    assert!(parsed.confirmed);
    assert_eq!(parsed.location.unwrap().path, "file.txt");
    for (field, value) in [
        ("name", json!("../bad")),
        ("name", json!("x".repeat(129))),
        ("surviving_branch_id", json!("branch")),
        ("inputs", json!([["branch", "old", "new"]])),
        (
            "inputs",
            json!([{"branch_id":"branch","expected_node_id":"new"}]),
        ),
        (
            "inputs",
            json!([{"branch_id":"branch","source_node_id":"old","expected_node_id":"new","unknown":true}]),
        ),
        ("operation_id", json!("bad")),
        ("file_id", json!("other")),
    ] {
        let mut invalid = body.clone();
        invalid[field] = value;
        assert!(decode(invalid).is_err(), "accepted {field}");
    }
    for field in [
        "name",
        "inputs",
        "chosen_node_id",
        "operation_id",
        "path",
        "repository_id",
    ] {
        let mut invalid = body.clone();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(decode(invalid).is_err(), "accepted missing {field}");
    }
    for consent in [Value::Null, json!(false), json!("true"), json!(1)] {
        let mut invalid = body.clone();
        invalid["confirmed"] = consent;
        assert!(!decode(invalid).unwrap().confirmed);
    }
}
