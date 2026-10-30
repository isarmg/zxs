use super::*;
use json_codec::json;

#[test]
fn request_budget_includes_operation_identity_and_exact_json_escaping() {
    let empty = json!({"path":"","note":""});
    let overhead = json_codec::to_vec(&FinalAction {
        body: &empty,
        operation_id: RESERVED_OPERATION_ID,
    })
    .unwrap()
    .len();
    let mut action = json!({"path":"","note":"a".repeat(MAX_ACTION_REQUEST_BYTES - overhead)});
    check_request(&action, 1, 1).unwrap();
    action["note"] = format!("{}a", action["note"].as_str().unwrap()).into();
    assert!(matches!(
        check_request(&action, 1, 1),
        Err(LimitError::Bytes { .. })
    ));

    let mut escaped = json!({"path":"文/\"quoted\"","note":"\\\n\t文"});
    let actual = json_codec::to_vec(&FinalAction {
        body: &escaped,
        operation_id: RESERVED_OPERATION_ID,
    })
    .unwrap()
    .len();
    escaped["padding"] = "x".repeat(MAX_ACTION_REQUEST_BYTES - actual - 13).into();
    check_request(&escaped, 1, 1).unwrap();
    escaped["padding"] = format!("{}x", escaped["padding"].as_str().unwrap()).into();
    assert!(matches!(
        check_request(&escaped, 1, 1),
        Err(LimitError::Bytes { .. })
    ));
}

#[test]
fn entry_budget_uses_the_supplied_core_capability() {
    let action = json!({"inputs":[]});
    check_request(&action, 5_001, 5_001).unwrap();
    assert!(matches!(
        check_request(&action, 5_002, 5_001),
        Err(LimitError::Entries { maximum: 5_001 })
    ));
}
