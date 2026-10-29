use super::*;

#[test]
fn cancel_parses_once_but_preserves_operation_error_for_plan_admission() {
    let operation_id = storage_contract::OperationId::generate();
    let body = json_codec::json!({"plan_id":"plan","operation_id":operation_id});
    let parsed = parse(body.as_object().unwrap().clone()).unwrap();
    assert_eq!(parsed.plan_id, "plan");
    assert_eq!(parsed.operation_id.unwrap(), operation_id);
    for (operation, code) in [
        (Value::Null, storage_contract::ErrorCode::InvalidRequest),
        (
            Value::Bool(true),
            storage_contract::ErrorCode::InvalidRequest,
        ),
        (
            Value::String(String::new()),
            storage_contract::ErrorCode::InvalidRequest,
        ),
        (
            Value::String("bad".into()),
            storage_contract::ErrorCode::InvalidId,
        ),
    ] {
        let mut wrong = body.clone();
        wrong["operation_id"] = operation;
        assert_eq!(
            parse(wrong.as_object().unwrap().clone())
                .unwrap()
                .operation_id
                .unwrap_err()
                .code,
            code
        );
    }
    let missing = json_codec::json!({"plan_id":"plan"});
    assert_eq!(
        parse(missing.as_object().unwrap().clone())
            .unwrap()
            .operation_id
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::InvalidRequest
    );
    let mut unknown = body;
    unknown["unexpected"] = Value::Null;
    assert_eq!(
        parse(unknown.as_object().unwrap().clone())
            .err()
            .unwrap()
            .code,
        storage_contract::ErrorCode::InvalidRequest
    );
}

#[test]
fn plan_projection_preserves_raw_wire_and_large_sequence() {
    let plan = storage_contract::Plan {
        plan_id: "plan".into(),
        kind: storage_contract::PlanKind::Purge,
        principal: "admin".into(),
        expected_sequence: u64::MAX,
        digest: "digest".into(),
        input: json_codec::json!({"history_tree_id":"history","opaque":{"items":[1,null]}}),
        state: storage_contract::PlanState::Ready,
    };
    let response = PlanResponse::from(plan.clone());
    let bytes = json_codec::to_vec(&response).unwrap();
    let parsed: Value = storage_contract::strict_json(&bytes).unwrap();
    assert_eq!(parsed, json_codec::to_value(&plan).unwrap());
    assert_eq!(parsed["expected_sequence"], u64::MAX.to_string());
    assert_eq!(parsed["state"], "ready");
}

#[test]
fn commit_required_text_precedes_unknown_fields_and_operation_format_is_deferred() {
    let operation = storage_contract::OperationId::generate();
    let body =
        json_codec::json!({"plan_id":"plan","operation_id":operation,"plan_digest":"digest"});
    let parsed = parse_commit(body.as_object().unwrap().clone()).unwrap();
    assert_eq!(parsed.plan_digest, "digest");
    assert_eq!(parsed.operation_id.unwrap(), operation);
    for value in [Value::Null, Value::Bool(true), Value::String(String::new())] {
        let mut wrong = body.clone();
        wrong["operation_id"] = value;
        wrong["confirmed"] = true.into();
        let error = parse_commit(wrong.as_object().unwrap().clone())
            .err()
            .unwrap();
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
        assert_eq!(error.message, "missing string field operation_id");
    }
    for confirmed in [false, true] {
        let mut wrong = body.clone();
        wrong["confirmed"] = confirmed.into();
        assert_eq!(
            parse_commit(wrong.as_object().unwrap().clone())
                .err()
                .unwrap()
                .code,
            storage_contract::ErrorCode::InvalidRequest
        );
    }
    let mut invalid = body;
    invalid["operation_id"] = "bad".into();
    assert_eq!(
        parse_commit(invalid.as_object().unwrap().clone())
            .unwrap()
            .operation_id
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::InvalidId
    );
    invalid["plan_digest"] = "".into();
    assert_eq!(
        parse_commit(invalid.as_object().unwrap().clone())
            .err()
            .unwrap()
            .message,
        "missing string field plan_digest"
    );
}
