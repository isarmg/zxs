use super::*;

#[test]
fn admitted_directory_projection_reuses_facts_only_for_its_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = initialize(temp.path());
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("directory facts").unwrap(),
            "P01",
            3,
            "alice",
            OperationId::generate(),
        )
        .unwrap();
    save_path(
        &mut engine,
        &repo.main_branch_id,
        "parent/child/file.txt",
        b"body",
    );
    let members = engine.members();
    let candidate = latest_candidate(&engine);
    let admitted =
        crate::publication_evidence::verified_snapshot(&members, &candidate.snapshot_hash).unwrap();
    let mut reads = BTreeMap::new();
    let objects = bound_history_objects_with(&candidate, Some(&admitted), |kind, hash| {
        *reads.entry(kind).or_insert(0_usize) += 1;
        if matches!(kind, "namespace-page" | "namespace-index") {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "binding must reuse this snapshot's admitted directory identities",
            ));
        }
        crate::publication_evidence::verified_graph_object(&members, kind, hash)
    })
    .unwrap();
    assert!(!objects.is_empty());
    assert!(reads.get("history-manifest").is_some_and(|count| *count > 0));
    assert!(reads.get("state-page").is_some_and(|count| *count > 0));

    // Another admission must independently read and validate the namespace;
    // the previous snapshot's directory facts cannot authorize it.
    let snapshot = load_object(&members[0], "snapshot", &candidate.snapshot_hash).unwrap();
    let mut namespace_reads = 0;
    let repeated = crate::journal::resolve_admitted_snapshot(snapshot, |kind, hash| {
        if matches!(kind, "namespace-page" | "namespace-index") {
            namespace_reads += 1;
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "new admission cannot read its namespace",
            ));
        }
        crate::publication_evidence::verified_graph_object(&members, kind, hash)
    });
    assert!(repeated.is_err());
    assert!(namespace_reads > 0);
}
