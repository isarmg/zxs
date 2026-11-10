//! Two shared completion boundaries, exercised once through a real SMB backend.
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
        let first = command(&roots, operation);
        let retry = command(&roots, operation);
        let old = original_entry(&roots);
        let before = roots
            .backend
            .inner
            .test_with_engine_locked(|engine| engine.state().clone());
        roots
            .backend
            .inner
            .test_with_engine_locked(|engine| engine.inject_fault_once(fault));
        let error = roots
            .backend
            .inner
            .move_current_file(&actor, first)
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
        let published = backend.test_with_engine_locked(|engine| engine.state().clone());
        assert_eq!(
            backend.move_current_file(&actor, retry).unwrap(),
            receipt,
            "{fault}"
        );
        backend.test_with_engine_locked(|engine| assert_eq!(engine.state(), &published, "{fault}"));
    }
}
