use super::*;
use json_codec::json;
use std::sync::Arc;
fn commit(id: &str, entries: u64) -> Arc<storage_contract::Commit> {
    Arc::new(storage_contract::Commit {
        commit_id: id.into(),
        history_tree_id: "history".into(),
        kind: "open-kind".into(),
        parent_commit_ids: vec![],
        tree_id: "tree".into(),
        sequence: 1,
        operation_id: storage_contract::OperationId::generate(),
        author: "owner".into(),
        message: "".into(),
        namespace: crate::fixture_commit_namespace(entries),
        pending_file_versions: Default::default(),
        namespace_deletions: None,
        provenance: None,
    })
}
fn entry(path: &str, version: &str) -> Entry {
    Entry {
        entry_id: format!("entry-{path}"),
        path: name_policy::PortablePath::parse(path).unwrap(),
        kind: storage_contract::NamespaceEntryKind::File,
        file_id: "file".into(),
        version_id: version.into(),
        size: u64::MAX,
    }
}
#[test]
fn comparison_wire_preserves_change_variants_omission_order_and_entry_fields() {
    let left_entries = vec![entry("keep.txt", "version"), entry("remove.txt", "old")];
    let right_entries = vec![entry("KEEP.txt", "version"), entry("add.txt", "new")];
    let expected = json!({"left":"left","right":"right","changes":[
        {"path":"KEEP.txt","change":"modified","before":left_entries[0],"after":right_entries[0]},
        {"path":"remove.txt","change":"removed","before":left_entries[1]},
        {"path":"add.txt","change":"added","after":right_entries[1]}
    ]});
    let response = ComparisonSnapshot::encode(
        &CommitComparison::between(
            commit("left", left_entries.len() as u64),
            left_entries,
            commit("right", right_entries.len() as u64),
            right_entries,
        ),
        &crate::tests::history_read_control(),
    )
    .unwrap();
    let actual: json_codec::Value = json_codec::from_slice(response.body()).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(
        storage_contract::canonical_json(&actual).unwrap(),
        storage_contract::canonical_json(&expected).unwrap()
    );
    assert_eq!(
        expected["changes"][0]["before"]["size"],
        u64::MAX.to_string()
    );
    let empty = ComparisonSnapshot::encode(
        &CommitComparison::between(commit("left", 0), vec![], commit("right", 0), vec![]),
        &crate::tests::history_read_control(),
    )
    .unwrap();
    assert_eq!(
        storage_contract::canonical_json(
            &json_codec::from_slice::<json_codec::Value>(empty.body()).unwrap()
        )
        .unwrap(),
        br#"{"changes":[],"left":"left","right":"right"}"#
    );
}

#[test]
fn comparison_budget_counts_escaped_ids_records_and_complete_envelope() {
    let comparison = CommitComparison::between(
        commit("left\"\\\n", 1),
        vec![entry("old.txt", "old\"\\")],
        commit("right\t\u{1}é", 1),
        vec![entry("new.txt", "new\n")],
    );
    let control = crate::tests::history_read_control();
    let snapshot = ComparisonSnapshot::encode(&comparison, &control).unwrap();
    let value: json_codec::Value = json_codec::from_slice(snapshot.body()).unwrap();
    assert_eq!(value["left"], comparison.left_id());
    assert_eq!(value["right"], comparison.right_id());
    assert_eq!(value["changes"][0]["before"]["version_id"], "old\"\\");
    let exact = ComparisonSnapshot::encode_with_limit(&comparison, &control, snapshot.body().len())
        .unwrap();
    assert_eq!(exact.body(), snapshot.body());
    assert_eq!(
        ComparisonSnapshot::encode_with_limit(&comparison, &control, snapshot.body().len() - 1)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::ResourceLimitExceeded
    );
    let expired = HistoryReadControl::until(std::time::Instant::now());
    assert_eq!(
        ComparisonSnapshot::encode(&comparison, &expired)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::RequestTimeout
    );
}

#[test]
fn comparison_protocol_budget_accepts_exact_limit_and_rejects_next_byte() {
    use storage_contract::client_protocol::MAX_RESPONSE_BYTES;
    let control = crate::tests::history_read_control();
    let shell = CommitComparison::between(commit("", 0), vec![], commit("right", 0), vec![]);
    let length = ComparisonSnapshot::encode(&shell, &control)
        .unwrap()
        .body()
        .len();
    let id = "x".repeat(MAX_RESPONSE_BYTES - length);
    let exact = CommitComparison::between(commit(&id, 0), vec![], commit("right", 0), vec![]);
    assert_eq!(
        ComparisonSnapshot::encode(&exact, &control)
            .unwrap()
            .body()
            .len(),
        MAX_RESPONSE_BYTES
    );
    let oversized =
        CommitComparison::between(commit(&(id + "x"), 0), vec![], commit("right", 0), vec![]);
    assert_eq!(
        ComparisonSnapshot::encode(&oversized, &control)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::ResourceLimitExceeded
    );
}

#[tokio::test]
async fn comparison_bytes_move_once_and_last_transport_chunk_owns_permit() {
    use futures::StreamExt;
    let comparison =
        CommitComparison::between(commit("left", 0), vec![], commit("right", 0), vec![]);
    let snapshot =
        ComparisonSnapshot::encode(&comparison, &crate::tests::history_read_control()).unwrap();
    let allocation = snapshot.body().as_ptr();
    let encoded = snapshot.into_encoded();
    assert_eq!(encoded.body().as_ptr(), allocation);
    let slots = Arc::new(tokio::sync::Semaphore::new(1));
    let limits = crate::TransferLimits::default();
    let response = encoded.into_leased_response(
        slots.clone().try_acquire_owned().unwrap(),
        &limits,
        tokio::time::Instant::now() + limits.total_timeout,
        None,
    );
    let mut stream = response.into_body().into_data_stream();
    let chunk = stream.next().await.unwrap().unwrap();
    assert!(stream.next().await.is_none());
    drop(stream);
    assert_eq!(slots.available_permits(), 0);
    assert_eq!(
        json_codec::from_slice::<json_codec::Value>(&chunk).unwrap(),
        json!({"left":"left","right":"right","changes":[]})
    );
    drop(chunk);
    assert_eq!(slots.available_permits(), 1);
}
