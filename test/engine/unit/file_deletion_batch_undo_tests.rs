//! Production publication and cold evidence. These tests use real Engine
//! fixtures; malformed clones only exercise admission, never authorize writes.
use super::*;
use crate::{DirectoryNamespaceBatch, ErrorCode};
use json_codec::json;
use storage_contract::history_window::HistoryFileId;
use crate::{
    CommitRequest, Config, DirectoryBatchKind, DirectoryNamespaceItem, DirectoryNamespaceSource,
    Entry, Repository, UndoFileDeletionRequest, canonical_json,
};
use std::{io::Read, path::PathBuf};

fn op() -> OperationId {
    OperationId::generate().unwrap()
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    engine: Engine,
    repository: Repository,
    entries: Vec<Entry>,
    scope: String,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("system");
        let config =
            Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
        let mut engine = Engine::initialize(&root, config).unwrap();
        let repository = engine.create_disk_root("P01", "admin", op()).unwrap();
        let workspace = engine
            .create_workspace(&repository.main_branch_id, "admin")
            .unwrap();
        for path in ["dir/group/a", "dir/group/b", "dir/stay"] {
            engine
                .stage_bytes(&workspace.workspace_id, path, path.as_bytes())
                .unwrap();
        }
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: op(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "batch undo fixture".into(),
                request_digest: None,
            })
            .unwrap();
        let scope = crate::directory_git_fixture::checkpoint(&mut engine, &repository, "dir")
            .directory_branch_id
            .unwrap();
        let tree = engine.namespace_tree(&repository.main_branch_id).unwrap();
        let entries = ["dir/group/a", "dir/group/b"]
            .map(|path| tree.get(path).unwrap().clone())
            .into();
        Self {
            _temp: temp,
            root,
            engine,
            repository,
            entries,
            scope,
        }
    }

    fn delete(&mut self) -> OperationResult {
        let request = DirectoryNamespaceBatch {
            kind: DirectoryBatchKind::Delete,
            items: self
                .entries
                .iter()
                .map(|entry| DirectoryNamespaceItem {
                    source: DirectoryNamespaceSource {
                        directory_branch_id: self.scope.clone(),
                        expected_tree_id: self.engine.state().directory_branches[&self.scope]
                            .head_tree_id
                            .clone(),
                        path: entry.path().into(),
                        entry_id: entry.entry_id.clone(),
                    },
                    target: None,
                })
                .collect(),
        };
        self.engine
            .directory_namespace_batch(&self.repository.repository_id, &request, "admin", op())
            .unwrap()
    }

    fn undo(
        &mut self,
        original: OperationId,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        self.engine
            .undo_file_deletion_batch(UndoFileDeletionBatchRequest {
                deletion_operation_id: original,
                principal: BatchUndoActor::parse(principal).unwrap(),
                operation_id: operation,
            })
    }

    fn assert_visibility(&self, visible: bool) {
        let tree = self
            .engine
            .namespace_tree(&self.repository.main_branch_id)
            .unwrap();
        for entry in &self.entries {
            assert_eq!(tree.get(entry.path()), visible.then_some(entry));
            assert_eq!(self.engine.file_deletion(&entry.file_id).is_none(), visible);
            let mut bytes = Vec::new();
            self.engine
                .open_content(&entry.version_id)
                .unwrap()
                .file
                .read_to_end(&mut bytes)
                .unwrap();
            assert_eq!(bytes, entry.path().as_bytes());
        }
        assert!(tree.get("dir/stay").is_some());
    }
}

#[test]
fn complete_original_batch_restores_once_without_rewriting_history_and_survives_redelete() {
    let mut fixture = Fixture::new();
    let historical = fixture.engine.state().clone();
    let deleted = fixture.delete();
    let frozen = fixture.engine.state().clone();
    fixture.assert_visibility(false);
    let preview = fixture
        .engine
        .preflight_file_deletion_batch_undo(deleted.operation_id, BatchUndoActor::parse("other-admin").unwrap())
        .unwrap();
    assert_eq!(preview.deletion_operation_id, deleted.operation_id);
    assert_eq!(
        json_codec::to_value(&preview.deletion_nodes).unwrap(),
        deleted.result["deletion_nodes"]
    );
    assert_eq!(fixture.engine.state(), &frozen);
    let operation = op();
    // Trusted callers authenticate their current actor. As with single-file
    // undo, another authorized actor is not silently forbidden by D.author.
    let restored = fixture
        .undo(deleted.operation_id, "other-admin", operation)
        .unwrap();
    assert_eq!(restored.kind, UNDO);
    assert_eq!(restored.sequence, frozen.sequence + 1);
    assert_eq!(
        restored.result["deletion_operation_id"],
        json!(deleted.operation_id)
    );
    assert_eq!(
        restored.result["removed_deletion_nodes"],
        deleted.result["deletion_nodes"]
    );
    assert_eq!(
        fixture.engine.state().operations.len(),
        frozen.operations.len() + 1
    );
    assert_eq!(fixture.engine.state().events.len(), frozen.events.len() + 1);
    assert_eq!(
        fixture.engine.state().operation_principals[&operation],
        "other-admin"
    );
    assert_eq!(fixture.engine.state().branches, historical.branches);
    assert_eq!(fixture.engine.state().commits, historical.commits);
    assert_eq!(
        fixture.engine.state().directory_branches,
        historical.directory_branches
    );
    assert_eq!(
        fixture.engine.state().directory_trees,
        historical.directory_trees
    );
    assert_eq!(
        fixture.engine.state().file_contexts,
        historical.file_contexts
    );
    assert_eq!(fixture.engine.state().versions, historical.versions);
    let projection = crate::state_projection::ProjectionSnapshot::new(fixture.engine.state());
    for entry in &fixture.entries {
        let rows =
            crate::file_deletion::retained::events(projection.view(), &entry.file_id).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].operation_id, deleted.operation_id);
        assert_eq!(rows[1].operation_id, operation);
        assert_eq!(rows[0].node, rows[1].node);
        assert_eq!(
            rows[0].action,
            crate::file_deletion::retained::DeletionAction::Delete
        );
        assert_eq!(
            rows[1].action,
            crate::file_deletion::retained::DeletionAction::Undo
        );
        assert_eq!(rows[0].author, "admin");
        assert_eq!(rows[1].author, "other-admin");
    }
    crate::file_deletion::publication::verify(&frozen, fixture.engine.state(), None).unwrap();
    crate::file_deletion::publication::admit_current(fixture.engine.state()).unwrap();
    fixture.assert_visibility(true);
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        fixture
            .undo(deleted.operation_id, "other-admin", operation)
            .unwrap(),
        restored
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    assert!(
        fixture
            .undo(deleted.operation_id, "admin", operation)
            .is_err()
    );
    fixture.engine.blocked = true;
    assert_eq!(
        fixture
            .engine
            .preflight_file_deletion_batch_undo(deleted.operation_id, BatchUndoActor::parse("admin").unwrap())
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        fixture
            .undo(deleted.operation_id, "other-admin", operation)
            .unwrap(),
        restored
    );
    fixture.engine.blocked = false;
    assert!(fixture.undo(op(), "other-admin", operation).is_err());
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    drop(fixture.engine);
    fixture.engine = Engine::open(&fixture.root).unwrap();
    fixture.assert_visibility(true);
    assert_eq!(
        fixture
            .undo(deleted.operation_id, "other-admin", operation)
            .unwrap(),
        restored
    );
    let newer = fixture.delete();
    let after = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        fixture
            .undo(deleted.operation_id, "other-admin", operation)
            .unwrap(),
        restored
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), after);
    assert_eq!(
        fixture
            .undo(deleted.operation_id, "admin", op())
            .unwrap_err()
            .code(),
        ErrorCode::FileNodeNotFound
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), after);
    fixture.assert_visibility(false);
    fixture.undo(newer.operation_id, "admin", op()).unwrap();
    fixture.assert_visibility(true);
}

#[test]
fn any_missing_original_member_prevents_the_entire_undo_publication() {
    let mut fixture = Fixture::new();
    let deleted = fixture.delete();
    let preview = fixture
        .engine
        .preflight_file_deletion_batch_undo(deleted.operation_id, BatchUndoActor::parse("admin").unwrap())
        .unwrap();
    assert_eq!(preview.deletion_nodes.len(), 2);
    let node = fixture
        .engine
        .file_deletion(&fixture.entries[1].file_id)
        .unwrap()
        .clone();
    fixture
        .engine
        .undo_file_deletion(UndoFileDeletionRequest {
            file_id: &HistoryFileId::parse(&node.file_id).unwrap(),
            deletion_node_id: &node.node_id,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        fixture
            .engine
            .preflight_file_deletion_batch_undo(deleted.operation_id, BatchUndoActor::parse("admin").unwrap())
            .unwrap_err()
            .code(),
        ErrorCode::FileNodeNotFound
    );
    assert_eq!(
        fixture
            .undo(deleted.operation_id, "admin", op())
            .unwrap_err()
            .code(),
        ErrorCode::FileNodeNotFound
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    assert!(
        fixture
            .engine
            .file_deletion(&fixture.entries[0].file_id)
            .is_some()
    );
    assert!(
        fixture
            .engine
            .file_deletion(&fixture.entries[1].file_id)
            .is_none()
    );
    drop(fixture.engine);
    fixture.engine = Engine::open(&fixture.root).unwrap();
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
}

#[test]
fn unknown_and_non_batch_operations_cannot_select_a_deletion_set() {
    let mut fixture = Fixture::new();
    let non_batch = *fixture.engine.state().operations.keys().next().unwrap();
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        BatchUndoActor::parse("").unwrap_err().code(),
        ErrorCode::Unauthenticated
    );
    assert_eq!(
        fixture.undo(op(), "admin", op()).unwrap_err().code(),
        ErrorCode::OperationNotFound
    );
    assert_eq!(
        fixture.undo(non_batch, "admin", op()).unwrap_err().code(),
        ErrorCode::InvalidRequest
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
}

#[test]
fn an_original_batch_with_one_distinct_file_is_still_one_complete_unit() {
    let mut fixture = Fixture::new();
    fixture.entries.truncate(1);
    let deleted = fixture.delete();
    assert_eq!(
        deleted.result["deletion_nodes"].as_array().unwrap().len(),
        1
    );
    let restored = fixture.undo(deleted.operation_id, "admin", op()).unwrap();
    assert_eq!(
        restored.result["removed_deletion_nodes"],
        deleted.result["deletion_nodes"]
    );
    fixture.assert_visibility(true);
    drop(fixture.engine);
    fixture.engine = Engine::open(&fixture.root).unwrap();
    fixture.assert_visibility(true);
}

#[test]
fn an_active_directory_marker_cannot_be_bypassed_by_original_file_batch_undo() {
    let mut fixture = Fixture::new();
    let deleted = fixture.delete();
    let group = fixture
        .engine
        .namespace_tree(&fixture.repository.main_branch_id)
        .unwrap()
        .get("dir/group")
        .unwrap()
        .clone();
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Delete,
        items: vec![DirectoryNamespaceItem {
            source: DirectoryNamespaceSource {
                directory_branch_id: fixture.scope.clone(),
                expected_tree_id: fixture.engine.state().directory_branches[&fixture.scope]
                    .head_tree_id
                    .clone(),
                path: group.path().into(),
                entry_id: group.entry_id,
            },
            target: None,
        }],
    };
    let directory = fixture
        .engine
        .directory_namespace_batch(&fixture.repository.repository_id, &request, "admin", op())
        .unwrap();
    let marker: storage_contract::directory_git::DirectoryDeletionNode =
        json_codec::from_value(directory.result["directory_deletion"].clone()).unwrap();
    assert_eq!(marker.files.len(), 2);
    assert!(
        marker
            .files
            .values()
            .all(|member| !member.created_by_directory)
    );
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        fixture
            .undo(deleted.operation_id, "admin", op())
            .unwrap_err()
            .code(),
        ErrorCode::BranchNotActive
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    fixture
        .engine
        .undo_directory_deletion(
            &fixture.repository.repository_id,
            &storage_contract::directory_git::UndoDirectoryDeletionBody {
                owner_directory_id: marker.owner_directory_id,
                directory_id: marker.directory_id,
                deletion_node_id: marker.node_id,
                operation_id: op(),
            },
            "admin",
        )
        .unwrap();
    fixture.assert_visibility(false);
    fixture.undo(deleted.operation_id, "admin", op()).unwrap();
    fixture.assert_visibility(true);
    drop(fixture.engine);
    fixture.engine = Engine::open(&fixture.root).unwrap();
    fixture.assert_visibility(true);
}

#[test]
fn batch_undo_cold_and_delta_proofs_reject_incomplete_rebound_or_reordered_evidence() {
    let mut fixture = Fixture::new();
    let deleted = fixture.delete();
    let frozen = fixture.engine.state().clone();
    let restored = fixture
        .undo(deleted.operation_id, "other-admin", op())
        .unwrap();
    let valid = fixture.engine.state().clone();
    for case in 0..15 {
        let mut forged = valid.clone();
        match case {
            0 => {
                forged
                    .operations
                    .get_mut(&restored.operation_id)
                    .unwrap()
                    .result["removed_deletion_nodes"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            }
            1 => {
                forged
                    .operations
                    .get_mut(&restored.operation_id)
                    .unwrap()
                    .result["removed_deletion_nodes"]
                    .as_array_mut()
                    .unwrap()
                    .reverse();
            }
            2 => {
                let nodes = forged
                    .operations
                    .get_mut(&restored.operation_id)
                    .unwrap()
                    .result["removed_deletion_nodes"]
                    .as_array_mut()
                    .unwrap();
                nodes[1] = nodes[0].clone();
            }
            3 => {
                forged
                    .operations
                    .get_mut(&restored.operation_id)
                    .unwrap()
                    .result["removed_deletion_nodes"][0]["author"] = json!("other-admin");
            }
            4 => {
                forged
                    .operations
                    .get_mut(&restored.operation_id)
                    .unwrap()
                    .result["deletion_operation_id"] = json!(op());
            }
            5 => {
                forged
                    .operations
                    .get_mut(&restored.operation_id)
                    .unwrap()
                    .result["force"] = json!(true);
            }
            6 => {
                forged
                    .operation_principals
                    .insert(restored.operation_id, "admin".into());
            }
            7 => {
                forged
                    .operation_principals
                    .insert(deleted.operation_id, "other-admin".into());
            }
            8 => {
                forged
                    .operations
                    .get_mut(&restored.operation_id)
                    .unwrap()
                    .sequence = deleted.sequence;
            }
            9 => {
                forged
                    .operations
                    .get_mut(&deleted.operation_id)
                    .unwrap()
                    .kind = "file.delete".into();
            }
            10 => {
                forged
                    .operations
                    .get_mut(&restored.operation_id)
                    .unwrap()
                    .request_digest = deleted.request_digest.clone();
            }
            11 => {
                let node = frozen.file_deletions.values().next().unwrap().clone();
                forged.file_deletions.insert(node.file_id.clone(), node);
            }
            12 => {
                forged.operations.remove(&deleted.operation_id);
            }
            13 => {
                forged
                    .operations
                    .get_mut(&restored.operation_id)
                    .unwrap()
                    .result["removed_deletion_nodes"][0]["history_tree_id"] =
                    json!(crate::new_id("hist").unwrap());
            }
            14 => {
                forged
                    .operations
                    .get_mut(&restored.operation_id)
                    .unwrap()
                    .result["removed_deletion_nodes"][0]["node_id"] =
                    json!(crate::new_id("fdel").unwrap());
            }
            _ => unreachable!(),
        }
        assert!(
            crate::file_deletion::publication::admit_current(&forged).is_err(),
            "cold case {case}"
        );
        assert!(
            crate::file_deletion::publication::verify(&frozen, &forged, None).is_err(),
            "delta case {case}"
        );
    }
    // A semantically unchanged deletion table cannot carry a fake batch undo.
    let mut unchanged = valid.clone();
    unchanged.file_deletions = frozen.file_deletions.clone();
    assert!(crate::file_deletion::publication::verify(&frozen, &unchanged, None).is_err());

    // Another well-formed receipt cannot consume the same original Ds twice.
    // Cold admission requires the entire set to be active at its own slot.
    let mut duplicate = valid.clone();
    duplicate.sequence += 1;
    let mut receipt = restored.clone();
    receipt.operation_id = op();
    receipt.sequence = duplicate.sequence;
    let mut event = duplicate.events.iter().last().unwrap().clone();
    event.sequence = receipt.sequence;
    event.operation_id = receipt.operation_id;
    event.object_id = receipt.operation_id.to_string();
    duplicate
        .operation_principals
        .insert(receipt.operation_id, "other-admin".into());
    duplicate
        .operations
        .insert(receipt.operation_id, receipt.clone());
    duplicate.events.push(event);
    assert!(admit_undo(&duplicate, &receipt).is_ok());
    assert!(crate::file_deletion::publication::admit_current(&duplicate).is_err());

    let mut oversized = restored;
    oversized.result["removed_deletion_nodes"][0]["author"] = json!("x".repeat(MAX_RESULT_BYTES));
    assert!(admit_undo(&valid, &oversized).is_err());
}

#[cfg(feature = "fault-injection")]
#[test]
fn batch_undo_publication_boundaries_recover_all_members_or_none_and_replay_once() {
    for (point, accepted) in [
        ("after_payloads", false),
        ("after_prepare_1", false),
        ("after_accept_1", true),
        ("after_finalize", true),
        ("after_publication_1", true),
    ] {
        let mut fixture = Fixture::new();
        let deleted = fixture.delete();
        let before = canonical_json(fixture.engine.state()).unwrap();
        let operations = fixture.engine.state().operations.len();
        let operation = op();
        fixture.engine.inject_fault_once(point);
        assert_eq!(
            fixture
                .undo(deleted.operation_id, "admin", operation)
                .unwrap_err()
                .code(),
            if accepted {
                ErrorCode::InDoubt
            } else {
                ErrorCode::InjectedFailure
            },
            "{point}"
        );
        assert_eq!(
            canonical_json(fixture.engine.state()).unwrap(),
            before,
            "{point}"
        );
        drop(fixture.engine);
        fixture.engine = Engine::open(&fixture.root).unwrap();
        assert_eq!(
            fixture.engine.state().operations.contains_key(&operation),
            accepted,
            "{point}"
        );
        fixture.assert_visibility(accepted);
        let recovered = canonical_json(fixture.engine.state()).unwrap();
        let receipt = fixture
            .undo(deleted.operation_id, "admin", operation)
            .unwrap();
        if accepted {
            assert_eq!(
                canonical_json(fixture.engine.state()).unwrap(),
                recovered,
                "{point}"
            );
        }
        assert_eq!(
            fixture.engine.state().operations.len(),
            operations + 1,
            "{point}"
        );
        fixture.assert_visibility(true);
        assert_eq!(
            fixture
                .undo(deleted.operation_id, "admin", operation)
                .unwrap(),
            receipt
        );
        drop(fixture.engine);
        fixture.engine = Engine::open(&fixture.root).unwrap();
        fixture.assert_visibility(true);
    }
}
