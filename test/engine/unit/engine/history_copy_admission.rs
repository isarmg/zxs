//! Pure proof attacks against an actually published current-registry fork.
use super::*;
use crate::*;

fn operation() -> OperationId {
    OperationId::generate().unwrap()
}
fn saved(engine: &mut Engine, branch: &str, bytes: &[u8]) {
    let workspace = engine.create_workspace(branch, "admin").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "old/file", bytes)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: operation(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap();
}

#[test]
fn history_copy_admission_rejects_incomplete_vectors_or_foreign_state_and_sources() {
    for retained in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let mut config =
            Config::development((0..5).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
        for disk in config.disks.iter_mut().skip(3) {
            disk.level = 1;
            disk.upstream_slot_id = None;
            disk.metadata_member = false;
        }
        let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
        let source = engine
            .create_disk_root("P01", "admin", operation())
            .unwrap();
        let target = engine
            .create_repository(
                &RepositoryName::parse("target").unwrap(),
                "D04",
                1,
                "admin",
                operation(),
            )
            .unwrap();
        saved(&mut engine, &source.main_branch_id, b"first");
        saved(&mut engine, &source.main_branch_id, b"second");
        engine
            .file_namespace(
                &source.repository_id,
                NamespaceOperation::parse(NamespaceOperationKind::Rename, "old/file", Some("file"))
                    .unwrap(),
                "admin",
                operation(),
            )
            .unwrap();
        engine
            .begin_disk_exit(
                &config.disks[4].disk_id,
                MembershipExitAction::Deregister,
                "admin",
                operation(),
            )
            .unwrap();
        let archive = engine.build_managed_membership_control_archive().unwrap();
        engine
            .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
            .unwrap();
        engine.resume_membership_joint_decision().unwrap();
        engine.activate_pending_disk_exit().unwrap();
        let source_entry = engine
            .namespace_tree(&source.main_branch_id)
            .unwrap()
            .get("file")
            .unwrap()
            .clone();
        let deletion = engine
            .soft_delete_file(DeleteFileRequest {
                target: FileDeletionTarget {
                    repository_id: &storage_contract::history_window::HistoryRepositoryId::parse(
                        &source.repository_id,
                    )
                    .unwrap(),
                    file_id: &storage_contract::history_window::HistoryFileId::parse(
                        &source_entry.file_id,
                    )
                    .unwrap(),
                    path: &name_policy::LookupPath::parse("file").unwrap(),
                },
                principal: "admin",
                operation_id: operation(),
            })
            .unwrap();
        let source_deletion = engine.file_deletion(&source_entry.file_id).unwrap().clone();
        engine
            .undo_file_deletion(UndoFileDeletionRequest {
                file_id: &storage_contract::history_window::HistoryFileId::parse(
                    &source_entry.file_id,
                )
                .unwrap(),
                deletion_node_id: deletion.result["deletion_node"]["node_id"]
                    .as_str()
                    .unwrap(),
                principal: "admin",
                operation_id: operation(),
            })
            .unwrap();
        let before = engine.state.clone();
        let view = engine.current_namespaces.clone().unwrap();
        let request = FileHistoryCopyRequest {
            source_repository_id: source.repository_id.clone(),
            source_path: "file".into(),
            expected_source_head: before.branches[&source.main_branch_id]
                .head_commit_id
                .clone(),
            target_repository_id: target.repository_id.clone(),
            target_name: "file".into(),
            expected_target_head: before.branches[&target.main_branch_id]
                .head_commit_id
                .clone(),
        };
        // An exact fork must not turn a historically split file into an implicit
        // placement migration. Exercise both primary and same-level route changes.
        let source_file = before.commits[&request.expected_source_head]
            .entries
            .iter()
            .find(|entry| entry.path() == "file")
            .unwrap()
            .file_id
            .clone();
        let old = before
            .versions
            .values()
            .find(|version| version.file_id == source_file)
            .unwrap();
        for changed_primary in [true, false] {
            let mut split = before.as_ref().clone();
            let placement = split.placements.get_mut(&old.version_id).unwrap();
            if changed_primary {
                placement.primary_slot_id = "D04".into();
                placement.route_slot_ids = vec!["D04".into()];
                placement.binding_generations = [("D04".into(), 1)].into();
            } else {
                placement.target_copies = 2;
                placement.route_slot_ids.push("B01".into());
                placement.binding_generations.insert("B01".into(), 1);
            }
            assert_eq!(
                crate::business::history_copy::sources(&split, Some(&view), &request)
                    .err()
                    .unwrap()
                    .code(),
                ErrorCode::UnsupportedOperation
            );
        }
        let result = if retained {
            engine.copy_retained_file_history(&request, "admin", operation())
        } else {
            engine.copy_file_history(&request, "admin", operation())
        }
        .unwrap();
        let after = engine.state.clone();
        let advanced = view.advance(&before, &after).unwrap();
        let historical_directory = after
            .commits
            .values()
            .filter(|commit| commit.history_tree_id == target.history_tree_id)
            .flat_map(|commit| &commit.entries)
            .find(|entry| entry.kind == NamespaceEntryKind::Directory)
            .unwrap();
        let target_head = &after.branches[&target.main_branch_id].head_commit_id;
        let cold: State =
            json_codec::from_slice(&storage_contract::canonical_json(after.as_ref()).unwrap())
                .unwrap();
        assert!(matches!(
            cold.commits.changes_since(&before.commits),
            storage_contract::MapChanges::All
        ));
        let mut cold_view = view.as_ref().clone();
        cold_view.reserve_history_copy_identities(&before, &cold);
        assert!(
            cold_view
                .known_entries
                .contains(&historical_directory.entry_id)
        );

        assert_eq!(
            advanced
                .validate_upload_identity(
                    &after.commits[target_head].entries,
                    "resurrected",
                    &historical_directory.entry_id
                )
                .unwrap_err()
                .code(),
            ErrorCode::WorkspaceChanged
        );
        let mut forged = after.as_ref().clone();
        forged.file_notes.insert(
            result.result["source_file_id"].as_str().unwrap().into(),
            "source tampered".into(),
        );
        assert_eq!(
            view.advance(&before, &forged).err().unwrap().code(),
            ErrorCode::PublicationInvalid
        );
        let mut forged = after.as_ref().clone();
        forged
            .repositories
            .get_mut(&target.repository_id)
            .unwrap()
            .target_copies = 2;
        assert_eq!(
            view.advance(&before, &forged).err().unwrap().code(),
            ErrorCode::PublicationInvalid
        );
        let ids = result.result["version_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_str().unwrap())
            .collect::<Vec<_>>();
        let payloads = || {
            ids.iter()
                .map(|id| (&after.versions[*id], &after.placements[*id]))
        };
        view.verify_history_copy_payloads(&after, &result, payloads())
            .unwrap();
        assert_eq!(
            view.verify_history_copy_payloads(&after, &result, payloads().take(1))
                .unwrap_err()
                .code(),
            ErrorCode::PublicationInvalid
        );
        assert_eq!(
            view.verify_history_copy_payloads(&after, &result, payloads().rev())
                .unwrap_err()
                .code(),
            ErrorCode::PublicationInvalid
        );
        let prepared = payloads()
            .map(|(version, placement)| Prepared {
                version: version.clone(),
                placement: placement.clone(),
                source: ReadSource::external(temp.path().join("untrusted-identical-bytes")),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            view.verify_history_copy_prepared(&before, &after, &result, &prepared)
                .unwrap_err()
                .code(),
            ErrorCode::PublicationInvalid
        );
        let mut forged = after.as_ref().clone();
        forged.versions.remove(ids[0]);
        assert!(view.advance(&before, &forged).is_err());

        if retained {
            assert_eq!(
                result.result["retained_lifecycle"]["events"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
            for attack in 0..5 {
                let mut forged = after.as_ref().clone();
                let receipt = forged.operations.get_mut(&result.operation_id).unwrap();
                match attack {
                    0 => {
                        receipt.result["retained_lifecycle"]["events"] = json!([]);
                    }
                    1 => {
                        receipt.result["retained_lifecycle"]["source_records_hash"] =
                            json!("0".repeat(64));
                    }
                    2 => {
                        receipt.result["retained_lifecycle"]["events"][0]["node"]["file_id"] =
                            json!(source_file);
                    }
                    3 => {
                        receipt
                            .result
                            .as_object_mut()
                            .unwrap()
                            .remove("retained_lifecycle");
                    }
                    4 => {
                        forged
                            .file_deletions
                            .insert(source_file.clone(), source_deletion.clone());
                    }
                    _ => unreachable!(),
                }
                let cold: State =
                    json_codec::from_slice(&storage_contract::canonical_json(&forged).unwrap())
                        .unwrap();
                assert_eq!(
                    view.advance(&before, &cold).err().unwrap().code(),
                    ErrorCode::PublicationInvalid
                );
            }
        }
    }
}
