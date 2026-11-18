//! Existing ordinary FILE storage identities survive replay and registered-root restoration.
use super::*;

#[test]
fn version_builds_allocate_distinct_local_identities_without_changing_the_live_base() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = engine.create_disk_root("P01", "admin", OperationId::generate().unwrap()).unwrap();
    let file = new_id("file").unwrap();
    let path = name_policy::PortablePath::parse("file.txt").unwrap();
    let source = fixture.temp.path().join("source");
    fs::write(&source, b"body").unwrap();
    let baseline = engine.state().clone();
    let mut next = baseline.clone();
    next.begin_tracking();
    let mut prepared = Vec::new();
    let build = |state: &mut State, prepared: &mut Vec<crate::engine::Prepared>| {
        engine
            .build_version(
                state,
                prepared,
                crate::business::VersionBuild {
                    history: &repo.history_tree_id,
                    file_id: &file,
                    path: &path,
                    source: &crate::engine::ReadSource::external(source.clone()),
                    primary: "P01",
                    copies: 1,
                    source_ids: Vec::new(),
                    checkpoint: true,
                },
            )
            .unwrap()
    };
    let first = build(&mut next, &mut prepared);
    let second = build(&mut next, &mut prepared);
    assert_ne!(first.storage_id, second.storage_id);
    assert_ne!(first.version_id, second.version_id);
    crate::container_storage::validate_version(&first).unwrap();
    crate::container_storage::validate_version(&second).unwrap();
    assert_eq!(next.versions.len(), 2);
    assert_eq!(engine.state(), &baseline);

    // Discarding a candidate leaves the live version set unchanged.
    drop(next);
    drop(prepared);
    let mut retry = engine.state().clone();
    retry.begin_tracking();
    let rebuilt = build(&mut retry, &mut Vec::new());
    assert_ne!(rebuilt.storage_id, first.storage_id);
    assert_ne!(rebuilt.storage_id, second.storage_id);
    assert_eq!(engine.state(), &baseline);
}

#[test]
fn purged_versions_retain_local_identity_after_reopen_and_replay() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = engine.create_disk_root("P01", "admin", OperationId::generate().unwrap()).unwrap();
    save(&mut engine, &repo.main_branch_id, "file.txt", b"first");
    let first = engine.list_tree(&repo.main_branch_id).unwrap()[0].clone();
    save(&mut engine, &repo.main_branch_id, "file.txt", b"second");
    let second = engine.list_tree(&repo.main_branch_id).unwrap()[0].clone();
    let second_id = engine.state.versions[&second.version_id].storage_id;
    let purge = engine
        .create_purge_plan(
            &repo.history_tree_id,
            vec![first.version_id.clone()],
            "admin",
        )
        .unwrap();
    engine
        .commit_plan(
            &purge.plan_id,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    let old = engine.state.versions[&first.version_id].clone();
    drop(engine);

    // The first reopen builds a recovery checkpoint; the second can consume it.
    drop(Engine::open(&fixture.root).unwrap());
    let mut engine = Engine::open(&fixture.root).unwrap();
    assert_eq!(engine.state.versions[&first.version_id], old);
    assert_eq!(
        engine.state.content_states[&first.version_id],
        ContentState::Purged
    );
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", b"third")
        .unwrap();
    let request = CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: storage_contract::OperationId::generate().unwrap(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "third".into(),
        request_digest: None,
    };
    let published = engine.commit_workspace(request.clone()).unwrap();
    let third = engine.list_tree(&repo.main_branch_id).unwrap()[0].clone();
    assert_ne!(engine.state.versions[&third.version_id].storage_id, second_id);
    let third_id = engine.state.versions[&third.version_id].storage_id;
    let count = engine.state.versions.len();
    assert_eq!(engine.commit_workspace(request.clone()).unwrap(), published);
    assert_eq!(engine.state.versions.len(), count);
    drop(engine);
    let mut engine = Engine::open(&fixture.root).unwrap();
    assert_eq!(engine.commit_workspace(request).unwrap(), published);
    assert_eq!(engine.state.versions.len(), count);
    assert_eq!(engine.state.versions[&third.version_id].storage_id, third_id);
}

#[test]
fn registered_disk_restore_preserves_ordinary_file_versions_and_local_storage_ids() {
    let f = Fixture::new();
    let mut original = f.initialize();
    let repo = original.create_disk_root("P01", "admin", OperationId::generate().unwrap()).unwrap();
    save(&mut original, &repo.main_branch_id, "body.txt", b"first");
    let before = original.list_tree(&repo.main_branch_id).unwrap()[0].clone();
    save(&mut original, &repo.main_branch_id, "body.txt", b"second");
    let after = original.list_tree(&repo.main_branch_id).unwrap()[0].clone();
    let versions = original.state().versions.clone();
    assert_ne!(versions[&before.version_id].storage_id, versions[&after.version_id].storage_id);
    let roots = original.config().disks.iter().map(|d| d.root.clone()).collect::<Vec<_>>();
    drop(original);
    fs::remove_dir_all(&f.root).unwrap();
    let restored = Engine::restore_from_disks(&f.root, &roots).unwrap();
    assert_eq!(restored.state().versions, versions);
    assert_eq!(bytes(&restored, &before.version_id), b"first");
    assert_eq!(bytes(&restored, &after.version_id), b"second");
}
