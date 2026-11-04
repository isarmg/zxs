use super::*;
use json_codec::json;
fn id(prefix: &str, n: u8) -> String {
    format!("{prefix}_{n:032x}")
}
fn request() -> FileHistorySourceRequest {
    FileHistorySourceRequest::parse(&id("file", 1), &id("ver", 1), &"a".repeat(64)).unwrap()
}
#[test]
fn source_request_is_exact_read_only_and_does_not_modify_regular_history_queries() {
    let request = request();
    let url = endpoint(&request);
    assert!(url.contains("/merge-history/source?"));
    assert!(url.contains("node_id=ver_"));
    assert!(url.contains("history_root="));
}
#[test]
fn no_response_can_invent_a_different_clicked_source_or_history_root() {
    let request = request();
    let branch = json!({
        "branch_id":id("br",1),"name":"Archived source","node_id":id("ver",2),
        "head_commit_id":id("commit",1),"fork_commit_id":id("commit",1),
        "state":"archived","branch_kind":"file","generation":"1","current":false,"read_only":true,"alias_of":null,
        "content":{"version_id":id("ver",2),"source":{"kind":"owned"},"stored_state":"AVAILABLE"}
    });
    let value = json!({"file_id":id("file",1),"source_node_id":id("ver",1),"history_root":"a".repeat(64),"branch":branch});
    let selected = parse(value.clone(), &request).unwrap();
    assert_eq!(selected.expected, id("ver", 2));
    assert_eq!(selected.id, id("br", 1));
    for (field, bad) in [
        ("source_node_id", json!(id("ver", 3))),
        ("history_root", json!("b".repeat(64))),
        ("file_id", json!(id("file", 2))),
    ] {
        let mut changed = value.clone();
        changed[field] = bad;
        assert!(parse(changed, &request).is_err());
    }
}
