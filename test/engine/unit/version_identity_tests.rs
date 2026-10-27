//! Per-file numbering consumes admitted records and transaction-local builders.
use super::*;

fn number(version: &FileVersion) -> u64 {
    version
        .storage_name
        .split('_')
        .next()
        .unwrap()
        .strip_prefix('v')
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn version_builds_advance_inside_one_candidate_and_discard_with_that_candidate() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 1);
    let file = new_id("file");
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
    assert_eq!((number(&first), number(&second)), (1, 2));
    assert_ne!(first.version_id, second.version_id);
    crate::container_storage::validate_version(&first).unwrap();
    crate::container_storage::validate_version(&second).unwrap();
    assert_eq!(next.versions.len(), 2);
    assert_eq!(engine.state(), &baseline);

    // A discarded unpublished candidate cannot reserve numbers in the live base.
    drop(next);
    drop(prepared);
    let mut retry = engine.state().clone();
    retry.begin_tracking();
    assert_eq!(number(&build(&mut retry, &mut Vec::new())), 1);
    assert_eq!(engine.state(), &baseline);
}

#[test]
fn purged_versions_remain_numbered_after_reopen_and_replay() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 3);
    save(&mut engine, &repo.main_branch_id, "file.txt", b"first");
    let first = engine.list_tree(&repo.main_branch_id).unwrap()[0].clone();
    save(&mut engine, &repo.main_branch_id, "file.txt", b"second");
    let second = engine.list_tree(&repo.main_branch_id).unwrap()[0].clone();
    assert_eq!(number(&engine.state.versions[&second.version_id]), 2);
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
            storage_contract::OperationId::generate(),
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
        operation_id: storage_contract::OperationId::generate(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "third".into(),
        request_digest: None,
    };
    let published = engine.commit_workspace(request.clone()).unwrap();
    let third = engine.list_tree(&repo.main_branch_id).unwrap()[0].clone();
    assert_eq!(number(&engine.state.versions[&third.version_id]), 3);
    let count = engine.state.versions.len();
    assert_eq!(engine.commit_workspace(request.clone()).unwrap(), published);
    assert_eq!(engine.state.versions.len(), count);
    drop(engine);
    let mut engine = Engine::open(&fixture.root).unwrap();
    assert_eq!(engine.commit_workspace(request).unwrap(), published);
    assert_eq!(engine.state.versions.len(), count);
    assert_eq!(number(&engine.state.versions[&third.version_id]), 3);
}

#[test]
fn mixed_split_and_package_restore_preserve_the_numbering_basis() {
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
            storage_contract::OperationId::generate(),
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
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let history = result.history_tree_id.unwrap();
    let branch = engine.state.histories[&history]
        .main_branch_id
        .clone()
        .unwrap();
    let mut split_numbers = engine
        .state
        .versions
        .values()
        .filter(|version| version.history_tree_id == history)
        .map(number)
        .collect::<Vec<_>>();
    split_numbers.sort();
    assert_eq!(split_numbers, [1, 2, 3]);
    for (id, version) in old_versions.iter() {
        assert_eq!(&engine.state.versions[id], version);
    }
    let tombstone = engine
        .state
        .versions
        .values()
        .find(|version| version.history_tree_id == history && number(version) == 2)
        .unwrap()
        .clone();
    assert_eq!(
        engine.state.content_states[&tombstone.version_id],
        ContentState::Purged
    );
    assert_eq!(
        tombstone.storage_name,
        old_versions[&second.version_id].storage_name
    );
    assert_eq!(
        tombstone.created_at,
        old_versions[&second.version_id].created_at
    );
    save(&mut engine, &branch, "file.txt", b"fourth");
    let latest = engine.list_tree(&branch).unwrap()[0].clone();
    assert_eq!(number(&engine.state.versions[&latest.version_id]), 4);

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
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    for (id, version) in &imported {
        assert_eq!(&restored.state.versions[id], version);
    }
    save(&mut restored, &branch, "file.txt", b"fifth");
    let fifth = restored.list_tree(&branch).unwrap()[0].clone();
    assert_eq!(number(&restored.state.versions[&fifth.version_id]), 5);
    drop(restored);
    let mut reopened = Engine::open(&target.root).unwrap();
    assert_eq!(reopened.state.versions[&tombstone.version_id], tombstone);
    save(&mut reopened, &branch, "file.txt", b"sixth");
    let sixth = reopened.list_tree(&branch).unwrap()[0].clone();
    assert_eq!(number(&reopened.state.versions[&sixth.version_id]), 6);
}
