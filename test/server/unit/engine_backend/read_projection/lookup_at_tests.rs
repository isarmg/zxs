use super::*;
use name_policy::LookupPath;

#[test]
fn snapshot_lookup_preserves_frozen_identity_equivalent_spelling_and_absence() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "Café.txt", b"first");
    let commit = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let expected = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    save_projection_file(&mut engine, &repo.main_branch_id, "Café.txt", b"second");
    let other = engine
        .create_repository(
            "Other",
            "P01",
            3,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let foreign = engine.state().branches[&other.main_branch_id]
        .head_commit_id
        .clone();
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let path = LookupPath::parse("CAFE\u{301}.TXT").unwrap();
    assert_eq!(
        backend
            .lookup_at(&identity, &repo.main_branch_id, &commit, &path)
            .unwrap(),
        Some(expected)
    );
    assert_eq!(
        backend
            .lookup_at(
                &identity,
                &repo.main_branch_id,
                &commit,
                &LookupPath::parse("missing").unwrap()
            )
            .unwrap(),
        None
    );
    for invalid in [foreign.as_str(), "missing-commit"] {
        let old = backend
            .tree_snapshot(
                &identity,
                crate::tests::tree_request(
                    &(json!({"branch_id":repo.main_branch_id,"query":{"commit_id":invalid}})),
                ),
            )
            .map(|snapshot| json_codec::to_value(snapshot).unwrap())
            .unwrap_err();
        let new = backend
            .lookup_at(&identity, &repo.main_branch_id, invalid, &path)
            .unwrap_err();
        assert_eq!((new.status, new.code), (old.status, old.code));
    }
    auth.logout(&token).unwrap();
    assert_eq!(
        backend
            .lookup_at(&identity, &repo.main_branch_id, &commit, &path)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
}

#[test]
fn snapshot_lookup_rejects_changed_smb_binding() {
    let (_root, engine, repo, identity) = fixture();
    let commit = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let mut identity = identity;
    identity.smb_binding = Some(crate::auth::SmbBinding {
        repository_id: repo.repository_id.clone(),
        branch_id: repo.main_branch_id.clone(),
        generation: repo.smb_binding_generation + 1,
    });
    let backend = EngineBackend::new(engine);
    assert_eq!(
        backend
            .lookup_at(
                &identity,
                &repo.main_branch_id,
                &commit,
                &LookupPath::parse("missing").unwrap()
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::SmbRebindRequired
    );
}

// Run explicitly in release mode; fixture publication is outside the timing.
#[test]
#[ignore = "comparative baseline-query latency measurement"]
fn measure_snapshot_lookup_against_tree_round_trip() {
    use std::{hint::black_box, time::Instant};
    for count in [10, 100, 1_000] {
        let (_root, mut engine, repo, identity) = fixture();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "owner")
            .unwrap();
        for index in 0..count - 1 {
            engine
                .namespace_operation(
                    &workspace.workspace_id,
                    storage_contract::NamespaceOperation::parse(
                        storage_contract::NamespaceOperationKind::Mkdir,
                        &format!("directory-{index:05}"),
                        None,
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        engine
            .stage_bytes(&workspace.workspace_id, "target.txt", b"baseline")
            .unwrap();
        engine
            .commit_workspace(storage_engine::CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: storage_contract::OperationId::generate(),
                principal: "owner".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "lookup fixture".into(),
                request_digest: None,
            })
            .unwrap();
        let commit = engine.state().branches[&repo.main_branch_id]
            .head_commit_id
            .clone();
        let backend = EngineBackend::new(engine);
        let path = LookupPath::parse("TARGET.TXT").unwrap();
        let old = || {
            let snapshot = backend
                .tree_snapshot(
                    &identity,
                    crate::tests::tree_request(
                        &(json!({"branch_id":repo.main_branch_id,"query":{"commit_id":commit}})),
                    ),
                )
                .map(|snapshot| json_codec::to_value(snapshot).unwrap())
                .unwrap();
            let entries: Vec<storage_contract::Entry> =
                json_codec::from_value(snapshot["entries"].clone()).unwrap();
            entries
                .into_iter()
                .find(|entry| entry.lookup_key() == name_policy::path_key(path.as_str()))
        };
        let new = || {
            backend
                .lookup_at(&identity, &repo.main_branch_id, &commit, &path)
                .unwrap()
        };
        TREE_PROJECTION_WORK.with(|work| work.set((0, 0)));
        let expected = old();
        let old_work = TREE_PROJECTION_WORK.with(|work| work.replace((0, 0)));
        assert_eq!(expected, new());
        let typed_work = TREE_PROJECTION_WORK.with(|work| work.replace((0, 0)));
        println!(
            "entries={count} old_tree_projections={} old_copied_tree_entries={} typed_tree_projections={} typed_copied_tree_entries={}",
            old_work.0, old_work.1, typed_work.0, typed_work.1
        );
        let mut legacy = Vec::new();
        let mut typed = Vec::new();
        for _ in 0..101 {
            let start = Instant::now();
            black_box(old());
            legacy.push(start.elapsed().as_nanos());
            let start = Instant::now();
            black_box(new());
            typed.push(start.elapsed().as_nanos());
        }
        legacy.sort_unstable();
        typed.sort_unstable();
        println!(
            "entries={count} samples=101 old_p50_ns={} old_p95_ns={} typed_p50_ns={} typed_p95_ns={}",
            legacy[50], legacy[95], typed[50], typed[95]
        );
    }
}
