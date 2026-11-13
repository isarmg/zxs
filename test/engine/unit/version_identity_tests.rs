//! Local identities survive publication, replay, split, and package restoration.
use super::*;

#[test]
fn version_builds_allocate_distinct_local_identities_without_changing_the_live_base() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 1);
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
    let repo = repository(&mut engine, 3);
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
fn mixed_split_and_package_restore_preserve_local_storage_identities() {
    use std::os::unix::fs::PermissionsExt;

    let source = Fixture::new();
    fs::set_permissions(source.temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut engine = source.initialize();
    let repo = repository(&mut engine, 3);
    let cut = save(&mut engine, &repo.main_branch_id, "file.txt", b"first")
        .commit_id
        .unwrap();
    save(&mut engine, &repo.main_branch_id, "file.txt", b"second");
    let second = engine.list_tree(&repo.main_branch_id).unwrap()[0].clone();
    save(&mut engine, &repo.main_branch_id, "file.txt", b"third");
    let purge = engine
        .create_purge_plan(
            &repo.history_tree_id,
            vec![second.version_id.clone()],
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
    let old_versions = engine.state.versions.clone();
    let split = engine
        .create_split_plan(&repo.history_tree_id, &cut, "admin")
        .unwrap();
    let result = engine
        .commit_plan(
            &split.plan_id,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    let history = result.history_tree_id.unwrap();
    let branch = engine.state.histories[&history]
        .main_branch_id
        .clone()
        .unwrap();
    let mut split_ids = engine
        .state
        .versions
        .values()
        .filter(|version| version.history_tree_id == history)
        .map(|version| version.storage_id)
        .collect::<Vec<_>>();
    split_ids.sort();
    let mut original_ids = old_versions.values().map(|version| version.storage_id)
        .collect::<Vec<_>>();
    original_ids.sort();
    assert_eq!(split_ids, original_ids);
    for (id, version) in old_versions.iter() {
        assert_eq!(&engine.state.versions[id], version);
    }
    let tombstone = engine
        .state
        .versions
        .values()
        .find(|version| {
            version.history_tree_id == history
                && version.storage_id == old_versions[&second.version_id].storage_id
        })
        .unwrap()
        .clone();
    assert_eq!(
        engine.state.content_states[&tombstone.version_id],
        ContentState::Purged
    );
    assert_eq!(
        tombstone.storage_id,
        old_versions[&second.version_id].storage_id
    );
    assert_eq!(
        tombstone.created_at,
        old_versions[&second.version_id].created_at
    );
    save(&mut engine, &branch, "file.txt", b"fourth");
    let latest = engine.list_tree(&branch).unwrap()[0].clone();
    assert!(!split_ids.contains(&engine.state.versions[&latest.version_id].storage_id));

    let package = source.temp.path().join("package");
    engine.export_history_package(&history, &package).unwrap();
    let imported = engine
        .state
        .versions
        .iter()
        .filter(|(_, version)| version.history_tree_id == history)
        .map(|(id, version)| (id.clone(), version.clone()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let target = Fixture::new();
    let mut restored = target.initialize();
    restored
        .restore_history_package(
            &package,
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    for (id, version) in &imported {
        assert_eq!(&restored.state.versions[id], version);
    }
    save(&mut restored, &branch, "file.txt", b"fifth");
    let fifth = restored.list_tree(&branch).unwrap()[0].clone();
    let fifth_id = restored.state.versions[&fifth.version_id].storage_id;
    assert!(imported.values().all(|version| version.storage_id != fifth_id));
    drop(restored);
    let mut reopened = Engine::open(&target.root).unwrap();
    assert_eq!(reopened.state.versions[&tombstone.version_id], tombstone);
    save(&mut reopened, &branch, "file.txt", b"sixth");
    let sixth = reopened.list_tree(&branch).unwrap()[0].clone();
    let sixth_id = reopened.state.versions[&sixth.version_id].storage_id;
    assert_ne!(sixth_id, fifth_id);
    assert!(imported.values().all(|version| version.storage_id != sixth_id));
}
