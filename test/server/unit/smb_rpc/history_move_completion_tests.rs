//! Shared completion boundaries and original evidence loss on disposable roots.
//! Object-kind coverage belongs to normal business tests, not a copied matrix.
use super::*;
use std::{fs, io::Read};
use storage_contract::file_relocation::{OBJECT_MOVE_KIND, ObjectMoveFact};

#[test]
fn object_completion_cold_replay_preserves_the_selected_operation_and_serving_scopes() {
    for (fault, expected_error) in [
        (
            "move_after_completion_binding",
            storage_contract::ErrorCode::RecoveryRequired,
        ),
        (
            "move_after_system_database",
            storage_contract::ErrorCode::CommitInDoubt,
        ),
    ] {
        let roots = roots();
        let actor = identity(&roots.shared);
        let operation = OperationId::generate().unwrap();
        let first = object_command(&roots, operation);
        let retry = object_command(&roots, operation);
        let old = original_entry(&roots);
        let before = roots
            .backend
            .inner
            .test_with_engine_locked(|engine| engine.state().clone());
        let members = roots.backend.inner.test_with_engine_locked(|engine| {
            engine
                .config()
                .disks
                .iter()
                .filter(|disk| disk.metadata_member)
                .map(|disk| disk.root.clone())
                .collect::<Vec<_>>()
        });
        roots
            .backend
            .inner
            .test_with_engine_locked(|engine| engine.inject_fault_once(fault));
        let error = roots
            .backend
            .inner
            .move_current_object(&actor, first)
            .unwrap_err();
        assert_eq!(error.code, expected_error, "{fault}");
        let active = roots._fixture.path().join("state/move-completion.json");
        let original_binding = fs::read(&active).unwrap();
        roots.backend.inner.test_with_engine_locked(|engine| {
            assert_eq!(engine.state(), &before, "{fault}");
            assert_eq!(
                engine
                    .create_workspace(&roots.source.main_branch_id, "alice")
                    .unwrap_err()
                    .code(),
                storage_contract::ErrorCode::RecoveryRequired,
                "{fault}"
            );
        });

        let Roots {
            _fixture,
            shared,
            backend,
            source,
            target,
        } = roots;
        drop(shared);
        drop(backend);
        // Keep bytes from the actual selected candidate. Model two interrupted
        // filesystem states without inventing a receipt or another binding.
        let member_active = members[0].join("move-completion.json");
        if fault == "move_after_completion_binding" {
            assert_eq!(fs::read(&member_active).unwrap(), original_binding);
            fs::remove_file(&active).unwrap();
            fs::remove_file(&member_active).unwrap();
            assert_eq!(
                fs::read(members[1].join("move-completion.json")).unwrap(),
                original_binding
            );
        } else {
            let archive = members[0].join("move-completion");
            fs::create_dir_all(&archive).unwrap();
            let retired = archive.join(format!("{operation}.json"));
            fs::rename(&member_active, &retired).unwrap();

            // Offline inspection keeps the old visible State, but must not
            // rebuild its SYSTEM view over the already installed candidate.
            let system = _fixture.path().join("state");
            let index = system.join("index.sqlite");
            let installed_index = fs::read(&index).unwrap();
            let mut preserved = Engine::open_for_recovery(&system).unwrap();
            assert_eq!(preserved.state(), &before);
            assert_eq!(
                preserved
                    .create_workspace(&source.main_branch_id, "alice")
                    .unwrap_err()
                    .code(),
                storage_contract::ErrorCode::RecoveryRequired
            );
            drop(preserved);
            assert_eq!(fs::read(index).unwrap(), installed_index);
            assert_eq!(fs::read(&active).unwrap(), original_binding);
            assert_eq!(fs::read(retired).unwrap(), original_binding);
            assert!(!member_active.exists());
        }
        let backend = reopen_backend(&_fixture);
        // Startup must finish the retained candidate before admitting current
        // reads; an absent source must not cause a new planner invocation.
        let receipt = backend.test_with_engine_locked(|engine| {
            let receipt = engine.state().operations.get(&operation).unwrap().clone();
            assert_eq!(receipt.kind, OBJECT_MOVE_KIND, "{fault}");
            assert_eq!(receipt.sequence, before.sequence + 1, "{fault}");
            assert_eq!(receipt.commit_state, CommitState::Published);
            assert_eq!(receipt.protection_at_publish, ProtectionAtPublish::TargetMet);
            assert_eq!(receipt.current_health, CurrentHealth::Healthy);
            assert_eq!(engine.state().operations.len(), before.operations.len() + 1);
            assert_eq!(
                engine
                    .state()
                    .events
                    .iter()
                    .filter(|event| event.operation_id == operation)
                    .count(),
                1
            );
            assert!(
                engine
                    .namespace_tree(&source.main_branch_id)
                    .unwrap()
                    .get("source.txt")
                    .is_none()
            );
            let tree = engine.namespace_tree(&target.main_branch_id).unwrap();
            let moved = tree.get("arrived.txt").unwrap();
            let mut body = Vec::new();
            engine
                .open_content(&moved.version_id)
                .unwrap()
                .file
                .read_to_end(&mut body)
                .unwrap();
            assert_eq!(body, b"original");
            assert!(
                matches!(engine.index_health(), storage_engine::IndexHealth::Ready { sequence }
                    if sequence == receipt.sequence)
            );
            for disk in &engine.config().disks {
                assert_eq!(
                    engine
                        .disk_catalog_observation(&disk.slot_id)
                        .unwrap()
                        .snapshot
                        .source_sequence,
                    receipt.sequence
                );
                assert_eq!(
                    engine
                        .disk_history_observation(&disk.slot_id)
                        .unwrap()
                        .snapshot
                        .header
                        .through_sequence,
                    receipt.sequence
                );
            }
            let fact: ObjectMoveFact =
                json_codec::from_value(receipt.result["movement"].clone()).unwrap();
            assert_eq!(fact.request.source_entry_id.as_str(), old.entry_id);
            assert_eq!(fact.source_file_id.as_ref().unwrap().as_str(), old.file_id);
            assert_eq!(fact.target_repository_id.as_str(), target.repository_id);
            assert_eq!(fact.target_entry_id.as_str(), moved.entry_id);
            receipt
        });
        assert!(!active.exists(), "{fault}");
        let retired = _fixture
            .path()
            .join("state/move-completion")
            .join(format!("{operation}.json"));
        assert_eq!(fs::read(retired).unwrap(), original_binding, "{fault}");
        for root in &members {
            assert!(!root.join("move-completion.json").exists(), "{fault}");
            assert_eq!(
                fs::read(root.join("move-completion").join(format!("{operation}.json"))).unwrap(),
                original_binding,
                "{fault}"
            );
        }
        let published = backend.test_with_engine_locked(|engine| engine.state().clone());
        assert_eq!(
            backend.move_current_object(&actor, retry).unwrap(),
            receipt,
            "{fault}"
        );
        backend.test_with_engine_locked(|engine| assert_eq!(engine.state(), &published, "{fault}"));
    }
}

#[test]
fn completed_object_move_needs_original_archive_after_system_loss() {
    let roots = roots();
    let actor = identity(&roots.shared);
    let operation = OperationId::generate().unwrap();
    let command = object_command(&roots, operation);
    let movement = command.movement.clone();
    let source_file = command.expected_source_file_id.clone();
    let receipt = roots
        .backend
        .inner
        .move_current_object(&actor, command)
        .unwrap();
    let (published, disks) = roots.backend.inner.test_with_engine_locked(|engine| {
        (
            engine.state().clone(),
            engine
                .config()
                .disks
                .iter()
                .map(|disk| disk.root.clone())
                .collect::<Vec<_>>(),
        )
    });
    let archives = disks
        .iter()
        .map(|root| {
            let path = root.join("move-completion").join(format!("{operation}.json"));
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect::<Vec<_>>();
    let Roots {
        _fixture,
        shared,
        backend,
        source,
        target,
    } = roots;
    drop(shared);
    drop(backend);
    fs::remove_dir_all(_fixture.path().join("state")).unwrap();

    let replacement = _fixture.path().join("replacement-system");
    let mut engine = Engine::restore_from_disks(&replacement, &disks).unwrap();
    assert_eq!(engine.state(), &published);
    assert_eq!(
        engine
            .move_captured_object(&movement, source_file.as_ref(), None, "alice", operation)
            .unwrap(),
        receipt
    );
    assert_eq!(engine.state(), &published);
    assert!(
        engine
            .namespace_tree(&source.main_branch_id)
            .unwrap()
            .get("source.txt")
            .is_none()
    );
    let tree = engine.namespace_tree(&target.main_branch_id).unwrap();
    let moved = tree.get("arrived.txt").unwrap();
    let mut body = Vec::new();
    engine
        .open_content(&moved.version_id)
        .unwrap()
        .file
        .read_to_end(&mut body)
        .unwrap();
    assert_eq!(body, b"original");
    for (path, bytes) in &archives {
        assert_eq!(&fs::read(path).unwrap(), bytes);
    }
    drop(engine);

    // A journal publication without any original completion archive does not
    // prove that FILE, disk databases and SYSTEM all finished installing.
    fs::remove_dir_all(&replacement).unwrap();
    for (path, _) in &archives {
        fs::remove_file(path).unwrap();
    }
    let rejected = _fixture.path().join("rejected-system");
    let error = Engine::restore_from_disks(&rejected, &disks).err().unwrap();
    assert_eq!(error.code(), storage_contract::ErrorCode::PreparationStale);
    for (path, _) in &archives {
        assert!(!path.exists());
    }
    for root in &disks {
        assert!(!root.join("move-completion.json").exists());
    }

    // Return the exact saved evidence, not a new successful operation. A fresh
    // restore must recover the same State and receipt without rewriting it.
    for (path, bytes) in &archives {
        fs::write(path, bytes).unwrap();
    }
    let mut engine =
        Engine::restore_from_disks(&_fixture.path().join("repaired-system"), &disks).unwrap();
    assert_eq!(engine.state(), &published);
    assert_eq!(
        engine
            .move_captured_object(&movement, source_file.as_ref(), None, "alice", operation)
            .unwrap(),
        receipt
    );
    assert_eq!(engine.state(), &published);
}

