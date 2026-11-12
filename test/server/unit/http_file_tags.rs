use super::Body;
use crate::http_boundary::JsonBody;
use json_codec::json;

fn body() -> json_codec::Value {
    json!({
        "repository_id": storage_contract::RepositoryId::from_random([2; 16]),
        "path": "file.txt",
        "expected_generation": "7",
        "tags": ["review", "审核"],
        "operation_id": storage_contract::OperationId::from_random([3; 16]),
    })
}

#[test]
fn replacement_body_retains_typed_context_generation_and_exact_labels() {
    let parsed = Body::from_json(body()).unwrap();
    assert_eq!(parsed.context.path.as_str(), "file.txt");
    assert_eq!(parsed.expected_generation.get(), 7);
    assert_eq!(
        parsed.tags.iter().map(|tag| tag.as_str()).collect::<Vec<_>>(),
        ["review", "审核"]
    );
}

#[test]
fn replacement_body_rejects_node_targets_and_invalid_tag_sets() {
    let mut legacy = body();
    legacy["targets"] = json!([]);
    assert!(Body::from_json(legacy).is_err());
    for tags in [json!(["duplicate", "duplicate"]), json!([" edge"])] {
        let mut candidate = body();
        candidate["tags"] = tags;
        let error = Body::from_json(candidate).err().unwrap();
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidTag);
    }
    let mut excess = body();
    excess["tags"] = json!((0..17).map(|i| format!("tag-{i}")).collect::<Vec<_>>());
    assert_eq!(
        Body::from_json(excess).err().unwrap().code,
        storage_contract::ErrorCode::TagLimitExceeded
    );
}
