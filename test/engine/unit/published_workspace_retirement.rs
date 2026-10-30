use super::*;

fn publish(engine: &mut Engine, branch: &str, payload: &[u8]) -> (CommitRequest, OperationResult) {
    let workspace = engine.create_workspace(branch, "admin").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file", payload)
        .unwrap();
    let request = CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "retained publication".into(),
        request_digest: None,
    };
    let result = engine.commit_workspace(request.clone()).unwrap();
    (request, result)
}

fn manifests(fixture: &Fixture, id: &str) -> [std::path::PathBuf; 2] {
    [
        fixture
            .root
            .join("workspaces")
            .join(id)
            .join("workspace.json"),
        fixture.config.disks[0]
            .root
            .join("staging/workspaces")
            .join(id)
            .join("workspace.json"),
    ]
}

#[test]
fn published_normal_workspaces_release_records_but_keep_replay_and_bytes() {
    let mut f = Fixture::new();
    f.config.max_retained_workspaces = 1;
    f.config.max_retained_workspaces_per_principal = 1;
    let mut engine = f.initialize();
    let repo = repository(&mut engine, 1);
    let mut saves = Vec::new();
    let mut bytes_charged = 0;
    for payload in [b"first".as_slice(), b"second", b"third", b"fourth"] {
        let (request, result) = publish(&mut engine, &repo.main_branch_id, payload);
        assert!(engine.workspaces.is_empty());
        let capacity = engine.workspace_capacity("admin");
        assert_eq!(capacity.principal.records.used, 0);
        assert_eq!(capacity.principal.cold_records, saves.len() as u64 + 1);
        assert!(capacity.principal.metadata_bytes.used > bytes_charged);
        bytes_charged = capacity.principal.metadata_bytes.used;
        let workspace = engine.workspace(&request.workspace_id).unwrap();
        assert_eq!(workspace.state, WorkspaceState::Committed);
        for file in workspace.files.values() {
            assert_eq!(fs::read(&file.source_path).unwrap(), payload);
        }
        for path in manifests(&f, &request.workspace_id) {
            assert!(path.is_file());
        }
        assert!(engine.native_workspace_commit_completed(&request).unwrap());
        saves.push((request, result));
    }
    // Original-ID replay remains available while a different live workspace
    // occupies the complete resident allowance.
    let active = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let capacity = engine.workspace_capacity("admin");
    let sequence = engine.state().sequence;
    for (request, result) in &saves {
        assert_eq!(engine.commit_workspace(request.clone()).unwrap(), *result);
    }
    assert_eq!(engine.state().sequence, sequence);
    assert_eq!(engine.workspace_capacity("admin"), capacity);
    assert!(engine.workspaces.contains_key(&active.workspace_id));
    engine.assert_workspace_budget_consistent();
    drop(engine);
    let mut engine = Engine::open(&f.root).unwrap();
    assert_eq!(engine.workspace_capacity("admin"), capacity);
    for (request, result) in saves {
        assert_eq!(engine.commit_workspace(request).unwrap(), result);
    }
    engine.assert_workspace_budget_consistent();
}

#[test]
fn published_abort_stays_cold_at_full_resident_quota_across_restart() {
    let mut f = Fixture::new();
    f.config.max_retained_workspaces = 1;
    f.config.max_retained_workspaces_per_principal = 1;
    let mut engine = f.initialize();
    let repo = repository(&mut engine, 1);
    let (request, result) = publish(&mut engine, &repo.main_branch_id, b"published bytes");
    engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let capacity = engine.workspace_capacity("admin");
    for _ in 0..3 {
        engine
            .abort_workspace(&request.workspace_id, "admin")
            .unwrap();
        assert_eq!(engine.commit_workspace(request.clone()).unwrap(), result);
        assert_eq!(engine.workspace_capacity("admin"), capacity);
    }
    let closed = engine.workspace(&request.workspace_id).unwrap();
    assert_eq!(closed.state, WorkspaceState::Aborted);
    assert!(engine.native_workspace_commit_completed(&request).unwrap());
    drop(engine);
    let mut engine = Engine::open(&f.root).unwrap();
    assert_eq!(engine.workspace_capacity("admin"), capacity);
    assert_eq!(
        canonical_json(&engine.workspace(&request.workspace_id).unwrap()).unwrap(),
        canonical_json(&closed).unwrap()
    );
    engine
        .abort_workspace(&request.workspace_id, "admin")
        .unwrap();
    assert_eq!(engine.commit_workspace(request).unwrap(), result);
    assert_eq!(engine.workspace_capacity("admin"), capacity);
    engine.assert_workspace_budget_consistent();
}

#[test]
fn published_workspace_survives_system_loss_without_reactivating_record_charge() {
    let mut f = Fixture::new();
    f.config.max_retained_workspaces = 1;
    f.config.max_retained_workspaces_per_principal = 1;
    let mut engine = f.initialize();
    let repo = repository(&mut engine, 1);
    let mut saves = Vec::new();
    for payload in [b"before".as_slice(), b"after"] {
        saves.push(publish(&mut engine, &repo.main_branch_id, payload));
    }
    let capacity = engine.workspace_capacity("admin");
    let sequence = engine.state().sequence;
    drop(engine);
    let restored = f.temp.path().join("restored");
    let disks: Vec<_> = f
        .config
        .disks
        .iter()
        .map(|disk| disk.root.clone())
        .collect();
    let mut engine = Engine::restore_from_disks(&restored, &disks).unwrap();
    assert_eq!(engine.workspace_capacity("admin"), capacity);
    assert!(engine.workspaces.is_empty());
    assert_eq!(engine.state().sequence, sequence);
    for (request, result) in saves {
        assert_eq!(engine.commit_workspace(request.clone()).unwrap(), result);
        engine
            .abort_workspace(&request.workspace_id, "admin")
            .unwrap();
    }
    assert_eq!(engine.workspace_capacity("admin"), capacity);
    engine.assert_workspace_budget_consistent();
}

#[test]
fn unbound_terminal_records_cannot_bypass_quota_before_recovery_writes() {
    for corruption in ["missing", "actor", "workspace", "action", "digest", "fence"] {
        let mut f = Fixture::new();
        f.config.max_retained_workspaces = 1;
        f.config.max_retained_workspaces_per_principal = 1;
        let mut engine = f.initialize();
        let repo = repository(&mut engine, 1);
        let unrelated = *engine.state.operations.keys().next().unwrap();
        let mut retained = Vec::new();
        for payload in [b"one".as_slice(), b"two"] {
            let (mut request, _) = publish(&mut engine, &repo.main_branch_id, payload);
            let mut workspace = engine.workspace(&request.workspace_id).unwrap();
            match corruption {
                "actor" => request.principal = "foreign".into(),
                "workspace" => request.workspace_id = new_id("ws"),
                "action" => request.operation_id = unrelated,
                "digest" => request.message = "unbound request".into(),
                "fence" => workspace.fencing_token += 1,
                "missing" => (),
                _ => unreachable!(),
            }
            for path in manifests(&f, &workspace.workspace_id) {
                crate::disk::write_json(&path, &workspace, true).unwrap();
                let saved = path.parent().unwrap().join("commit-request.json");
                if corruption == "missing" {
                    fs::remove_file(saved).unwrap();
                } else {
                    crate::disk::write_json(&saved, &request, true).unwrap();
                }
            }
            retained.push(workspace);
        }
        drop(engine);
        f.config.max_retained_workspaces = 1;
        f.config.max_retained_workspaces_per_principal = 1;
        crate::disk::write_json(&f.root.join("config.json"), &f.config, true).unwrap();
        // A torn tail makes an accidental recovery write observable.
        for disk in &f.config.disks {
            fs::OpenOptions::new()
                .append(true)
                .open(crate::journal::log_path(disk))
                .unwrap()
                .write_all(b"ZXS")
                .unwrap();
        }
        let logs: Vec<_> = f
            .config
            .disks
            .iter()
            .map(|disk| fs::read(crate::journal::log_path(disk)).unwrap())
            .collect();
        for recovery in [false, true] {
            let result = if recovery {
                Engine::open_for_recovery(&f.root)
            } else {
                Engine::open(&f.root)
            };
            assert_eq!(
                result.err().unwrap().code(),
                ErrorCode::ResourceLimit,
                "{corruption}"
            );
            for (disk, bytes) in f.config.disks.iter().zip(&logs) {
                assert_eq!(fs::read(crate::journal::log_path(disk)).unwrap(), *bytes);
            }
        }
        let restored = f.temp.path().join("rejected-restore");
        let disks: Vec<_> = f
            .config
            .disks
            .iter()
            .map(|disk| disk.root.clone())
            .collect();
        assert_eq!(
            Engine::restore_from_disks_with_workspace_limits(
                &restored,
                &disks,
                Some(crate::RetainedWorkspaceLimits {
                    max_retained_workspaces: 1,
                    max_retained_workspaces_per_principal: 1,
                    max_retained_workspace_metadata_bytes: f
                        .config
                        .max_retained_workspace_metadata_bytes,
                    max_retained_workspace_metadata_bytes_per_principal: f
                        .config
                        .max_retained_workspace_metadata_bytes_per_principal,
                })
            )
            .err()
            .unwrap()
            .code(),
            ErrorCode::ResourceLimit
        );
        assert!(!restored.join("config.json").exists());
        for workspace in retained {
            for path in manifests(&f, &workspace.workspace_id) {
                assert_eq!(fs::read(path).unwrap(), canonical_json(&workspace).unwrap());
            }
            for file in workspace.files.values() {
                assert!(file.source_path.is_file());
            }
        }
        for (disk, bytes) in f.config.disks.iter().zip(&logs) {
            assert_eq!(fs::read(crate::journal::log_path(disk)).unwrap(), *bytes);
        }
    }
}

#[test]
fn recovery_source_search_reads_old_full_generations_from_cold_workspaces() {
    let f = Fixture::new();
    let mut engine = f.initialize();
    let repo = repository(&mut engine, 3);
    let cold = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let old = engine
        .stage_bytes(&cold.workspace_id, "file", b"only old FULL has these bytes")
        .unwrap();
    let current = engine
        .stage_bytes(
            &cold.workspace_id,
            "file",
            b"current published bytes differ",
        )
        .unwrap();
    assert_ne!(old.source_path, current.source_path);
    engine
        .commit_workspace(CommitRequest {
            workspace_id: cold.workspace_id.clone(),
            operation_id: OperationId::generate(),
            principal: "admin".into(),
            expected_head: cold.base_head,
            fencing_token: cold.fencing_token,
            message: "cold source".into(),
            request_digest: None,
        })
        .unwrap();
    assert!(!engine.workspaces.contains_key(&cold.workspace_id));
    let pending = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let staged = engine
        .stage_bytes(
            &pending.workspace_id,
            "file",
            b"only old FULL has these bytes",
        )
        .unwrap();
    let operation = OperationId::generate();
    engine.inject_fault_once("after_accept_2");
    assert_eq!(
        engine
            .commit_workspace(CommitRequest {
                workspace_id: pending.workspace_id,
                operation_id: operation,
                principal: "admin".into(),
                expected_head: pending.base_head,
                fencing_token: pending.fencing_token,
                message: "repair from cold".into(),
                request_digest: None,
            })
            .unwrap_err()
            .code(),
        ErrorCode::InDoubt
    );
    let member = &f.config.disks[0];
    let held = &engine.disk_resources.get(member).unwrap().root;
    let scan = crate::journal::scan_at(held, member).unwrap();
    let hash = &scan.records.last().unwrap().0.candidate_hash;
    let candidate: crate::journal::Candidate =
        crate::journal::load_object_at(held, member, "candidate", hash).unwrap();
    assert_eq!(candidate.operation_id, operation);
    drop(engine);
    fs::remove_file(&staged.source_path).unwrap();
    for evidence in &candidate.versions {
        for disk in &f.config.disks {
            fs::remove_dir_all(crate::disk::version_dir(
                disk,
                &evidence.version.history_tree_id,
                &evidence.version.version_id,
            ))
            .unwrap();
        }
    }
    let engine = Engine::open(&f.root).unwrap();
    assert_eq!(
        engine.operation(operation).unwrap().commit_state,
        CommitState::Published
    );
    assert!(!engine.workspaces.contains_key(&cold.workspace_id));
    let version = &candidate.versions[0].version;
    assert_eq!(
        bytes(&engine, &version.version_id),
        b"only old FULL has these bytes"
    );
    assert_eq!(
        fs::read(&old.source_path).unwrap(),
        b"only old FULL has these bytes"
    );
    assert_eq!(
        fs::read(&current.source_path).unwrap(),
        b"current published bytes differ"
    );
    engine.assert_workspace_budget_consistent();
}

#[test]
fn published_completion_with_upload_owner_or_unknown_material_stays_resident() {
    for material in ["upload-owner", "unknown", ".zxs-upload-pending.prepared"] {
        let f = Fixture::new();
        let mut engine = f.initialize();
        let repo = repository(&mut engine, 1);
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "file", b"published")
            .unwrap();
        let mut owner = None;
        if material == "upload-owner" {
            let source = f.temp.path().join("unconfirmed");
            fs::write(&source, b"unconfirmed").unwrap();
            owner = Some(
                engine
                    .prepare_upload(
                        &workspace.workspace_id,
                        &new_id("entry"),
                        "later",
                        &source,
                        1,
                    )
                    .unwrap(),
            );
        } else {
            fs::write(
                manifests(&f, &workspace.workspace_id)[1]
                    .parent()
                    .unwrap()
                    .join(material),
                b"retained",
            )
            .unwrap();
        }
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id.clone(),
                operation_id: OperationId::generate(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "keep resident".into(),
                request_digest: None,
            })
            .unwrap();
        assert!(engine.workspaces.contains_key(&workspace.workspace_id));
        assert_eq!(engine.workspace_capacity("admin").principal.records.used, 1);
        drop(owner);
        engine
            .abort_workspace(&workspace.workspace_id, "admin")
            .unwrap();
        assert_eq!(
            engine.workspaces.contains_key(&workspace.workspace_id),
            material != "upload-owner"
        );
    }
}

#[test]
fn published_cold_point_reads_and_recovery_hints_keep_exact_mirror_proof() {
    for change in [
        "one-manifest",
        "both-manifests",
        "missing-mirror",
        "unknown-material",
    ] {
        let f = Fixture::new();
        let mut engine = f.initialize();
        let repo = repository(&mut engine, 1);
        let (request, _) = publish(&mut engine, &repo.main_branch_id, b"retained body");
        let paths = manifests(&f, &request.workspace_id);
        let capacity = engine.workspace_capacity("admin");
        if change == "missing-mirror" {
            fs::remove_file(&paths[0]).unwrap();
        } else if change == "unknown-material" {
            fs::write(paths[1].parent().unwrap().join("unknown"), b"unconfirmed").unwrap();
        } else {
            let mut changed = engine.workspace(&request.workspace_id).unwrap();
            changed.created_at += 1;
            for path in paths
                .iter()
                .take(if change == "one-manifest" { 1 } else { 2 })
            {
                crate::disk::write_json(path, &changed, true).unwrap();
            }
        }
        assert_eq!(
            engine.workspace(&request.workspace_id).unwrap_err().code(),
            ErrorCode::RecoveryRequired,
            "{change}"
        );
        assert_eq!(
            engine
                .workspace_recovery_hint(&request.workspace_id)
                .unwrap_err()
                .code(),
            ErrorCode::RecoveryRequired,
            "{change}"
        );
        assert_eq!(engine.workspace_capacity("admin"), capacity);
    }
}

#[test]
fn damaged_request_mirror_replays_admitted_cold_but_restart_keeps_record_resident() {
    let mut f = Fixture::new();
    f.config.max_retained_workspaces = 1;
    f.config.max_retained_workspaces_per_principal = 1;
    let mut engine = f.initialize();
    let repo = repository(&mut engine, 1);
    let (request, published) = publish(&mut engine, &repo.main_branch_id, b"retained publication");
    engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let admitted = engine.workspace_capacity("admin");
    assert_eq!(admitted.principal.records.used, 1);
    assert_eq!(admitted.principal.cold_records, 1);
    let sequence = engine.state().sequence;
    let paths = manifests(&f, &request.workspace_id);
    let manifests_before = paths.each_ref().map(|path| fs::read(path).unwrap());
    let damaged = paths[0].with_file_name("commit-request.json");
    let original_request_bytes = fs::read(&damaged).unwrap();
    fs::write(&damaged, b"{").unwrap();
    // Existing admission proves the exact manifest and mirror set. The healthy
    // original request copy still authorizes replay without new admission.
    assert_eq!(engine.commit_workspace(request.clone()).unwrap(), published);
    assert_eq!(engine.workspace_capacity("admin"), admitted);
    assert_eq!(engine.state().sequence, sequence);
    drop(engine);
    let logs: Vec<_> = f
        .config
        .disks
        .iter()
        .map(|disk| fs::read(crate::journal::log_path(disk)).unwrap())
        .collect();
    assert_eq!(
        Engine::open(&f.root).err().unwrap().code(),
        ErrorCode::ResourceLimit
    );
    assert_eq!(fs::read(&damaged).unwrap(), b"{");
    assert_eq!(
        paths.each_ref().map(|path| fs::read(path).unwrap()),
        manifests_before
    );
    for (disk, expected) in f.config.disks.iter().zip(logs) {
        assert_eq!(fs::read(crate::journal::log_path(disk)).unwrap(), expected);
    }
    let raised = crate::RetainedWorkspaceLimits {
        max_retained_workspaces: 2,
        max_retained_workspaces_per_principal: 2,
        max_retained_workspace_metadata_bytes: f.config.max_retained_workspace_metadata_bytes,
        max_retained_workspace_metadata_bytes_per_principal: f
            .config
            .max_retained_workspace_metadata_bytes_per_principal,
    };
    let mut engine = Engine::open_with_workspace_limits(&f.root, Some(raised)).unwrap();
    let resident = engine.workspace_capacity("admin");
    assert_eq!(resident.principal.records.used, 2);
    assert_eq!(resident.principal.cold_records, 0);
    assert_eq!(
        resident.principal.metadata_bytes,
        admitted.principal.metadata_bytes
    );
    assert_eq!(engine.commit_workspace(request.clone()).unwrap(), published);
    assert_eq!(engine.workspace_capacity("admin"), resident);
    assert_eq!(engine.state().sequence, sequence);
    assert_eq!(fs::read(&damaged).unwrap(), b"{");
    engine.assert_workspace_budget_consistent();
    drop(engine);
    // Restoring the exact saved evidence permits fresh cold admission again;
    // the runtime limit override has not changed the pool baseline.
    fs::write(&damaged, original_request_bytes).unwrap();
    let mut engine = Engine::open(&f.root).unwrap();
    assert_eq!(engine.workspace_capacity("admin"), admitted);
    assert_eq!(engine.commit_workspace(request).unwrap(), published);
    assert_eq!(engine.state().sequence, sequence);
    engine.assert_workspace_budget_consistent();
}

#[test]
fn historical_workspace_commit_abort_and_restart_keep_resident_charge() {
    let mut f = Fixture::new();
    f.config.max_retained_workspaces = 1;
    f.config.max_retained_workspaces_per_principal = 1;
    let mut engine = f.initialize();
    let repo = repository(&mut engine, 1);
    let head = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let workspace = engine
        .create_historical_workspace(&repo.main_branch_id, &head, "admin")
        .unwrap();
    let staged = engine
        .stage_bytes(
            &workspace.workspace_id,
            "historical",
            b"retained historical FULL",
        )
        .unwrap();
    assert_eq!(
        engine.workspace(&workspace.workspace_id).unwrap().purpose,
        WorkspacePurpose::HistoricalBranch
    );
    assert_eq!(engine.workspace_capacity("admin").principal.records.used, 1);
    assert_eq!(engine.workspace_capacity("admin").principal.cold_records, 0);
    let plan = engine
        .create_historical_save_plan(
            &repo.history_tree_id,
            crate::HistoricalSaveOptions {
                source_commit_id: head,
                name: "historical resident witness".into(),
                workspace_id: Some(workspace.workspace_id.clone()),
                primary_slot_id: None,
                target_copies: None,
            },
            "admin",
        )
        .unwrap();
    assert_eq!(
        engine.workspace(&workspace.workspace_id).unwrap().state,
        WorkspaceState::Sealed
    );
    assert_eq!(engine.workspace_capacity("admin").principal.records.used, 1);
    assert_eq!(engine.workspace_capacity("admin").principal.cold_records, 0);
    let operation = OperationId::generate();
    let published = engine
        .commit_historical_save_plan(&plan.plan_id, "admin", operation)
        .unwrap();
    assert_eq!(published.commit_state, CommitState::Published);
    let committed = engine.workspace(&workspace.workspace_id).unwrap();
    assert_eq!(committed.purpose, WorkspacePurpose::HistoricalBranch);
    assert_eq!(committed.state, WorkspaceState::Committed);
    assert!(engine.workspaces.contains_key(&workspace.workspace_id));
    let capacity = engine.workspace_capacity("admin");
    assert_eq!(capacity.principal.records.used, 1);
    assert_eq!(capacity.principal.cold_records, 0);
    assert!(capacity.principal.metadata_bytes.used > 0);
    assert_eq!(
        engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap_err()
            .code(),
        ErrorCode::ResourceLimit
    );
    let sequence = engine.state().sequence;
    engine.assert_workspace_budget_consistent();
    drop(engine);
    let mut engine = Engine::open(&f.root).unwrap();
    assert_eq!(engine.workspace_capacity("admin"), capacity);
    assert!(engine.workspaces.contains_key(&workspace.workspace_id));
    assert_eq!(
        canonical_json(&engine.workspace(&workspace.workspace_id).unwrap()).unwrap(),
        canonical_json(&committed).unwrap()
    );
    assert_eq!(
        engine
            .commit_historical_save_plan(&plan.plan_id, "admin", operation)
            .unwrap(),
        published
    );
    assert_eq!(engine.state().sequence, sequence);
    assert_eq!(
        fs::read(&staged.source_path).unwrap(),
        b"retained historical FULL"
    );
    engine
        .abort_workspace(&workspace.workspace_id, "admin")
        .unwrap();
    let aborted = engine.workspace(&workspace.workspace_id).unwrap();
    assert_eq!(aborted.purpose, WorkspacePurpose::HistoricalBranch);
    assert_eq!(aborted.state, WorkspaceState::Aborted);
    assert!(engine.workspaces.contains_key(&workspace.workspace_id));
    assert_eq!(engine.workspace_capacity("admin"), capacity);
    for path in manifests(&f, &workspace.workspace_id) {
        assert_eq!(fs::read(path).unwrap(), canonical_json(&aborted).unwrap());
    }
    engine.assert_workspace_budget_consistent();
    drop(engine);
    let mut engine = Engine::open(&f.root).unwrap();
    assert_eq!(engine.workspace_capacity("admin"), capacity);
    assert!(engine.workspaces.contains_key(&workspace.workspace_id));
    assert_eq!(
        canonical_json(&engine.workspace(&workspace.workspace_id).unwrap()).unwrap(),
        canonical_json(&aborted).unwrap()
    );
    assert_eq!(
        engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap_err()
            .code(),
        ErrorCode::ResourceLimit
    );
    assert_eq!(
        engine
            .commit_historical_save_plan(&plan.plan_id, "admin", operation)
            .unwrap(),
        published
    );
    assert_eq!(engine.state().sequence, sequence);
    assert_eq!(engine.workspace_capacity("admin"), capacity);
    assert_eq!(
        fs::read(&staged.source_path).unwrap(),
        b"retained historical FULL"
    );
    engine.assert_workspace_budget_consistent();
}

fn evidence_entries(
    root: &std::path::Path,
) -> std::collections::BTreeMap<std::path::PathBuf, Option<blake3::Hash>> {
    let mut entries = std::collections::BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            entries.insert(path.clone(), None);
            entries.extend(evidence_entries(&path));
        } else {
            entries.insert(path.clone(), Some(blake3::hash(&fs::read(path).unwrap())));
        }
    }
    entries
}

#[test]
fn terminal_manifest_beyond_verified_publication_prefix_cannot_release_quota_or_write() {
    for (point, publication_count) in [("after_publication_1", 1), ("after_publication_2", 2)] {
        let mut f = Fixture::new();
        f.config.max_retained_workspaces = 1;
        f.config.max_retained_workspaces_per_principal = 1;
        let raised = crate::RetainedWorkspaceLimits {
            max_retained_workspaces: 2,
            max_retained_workspaces_per_principal: 2,
            max_retained_workspace_metadata_bytes: f.config.max_retained_workspace_metadata_bytes,
            max_retained_workspace_metadata_bytes_per_principal: f
                .config
                .max_retained_workspace_metadata_bytes_per_principal,
        };
        // Build two real resident records under an explicit runtime override;
        // the saved pool baseline remains one record for restart and restore.
        let mut engine =
            Engine::initialize_with_workspace_limits(&f.root, f.config.clone(), Some(raised))
                .unwrap();
        let repo = repository(&mut engine, 3);
        let anchor = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        let anchor_full = engine
            .stage_bytes(&anchor.workspace_id, "anchor", b"unpublished quota anchor")
            .unwrap();
        engine
            .abort_workspace(&anchor.workspace_id, "admin")
            .unwrap();
        let pending = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        let staged = engine
            .stage_bytes(&pending.workspace_id, "file", b"unproven publication FULL")
            .unwrap();
        let request = CommitRequest {
            workspace_id: pending.workspace_id.clone(),
            operation_id: OperationId::generate(),
            principal: "admin".into(),
            expected_head: pending.base_head,
            fencing_token: pending.fencing_token,
            message: "incomplete member publication".into(),
            request_digest: None,
        };
        let prefix_sequence = engine.state().sequence;
        engine.inject_fault_once(point);
        let failure = engine.commit_workspace(request.clone()).unwrap_err();
        assert_eq!(failure.code(), ErrorCode::InDoubt);
        let Error::Context { source, .. } = &failure else {
            panic!("publication fault lost its concrete source: {failure:?}");
        };
        assert_eq!(source.code(), ErrorCode::InjectedFailure);
        assert_eq!(source.message(), format!("interrupted at {point}"));
        assert_eq!(engine.state().sequence, prefix_sequence);
        assert!(
            !engine
                .state()
                .operations
                .contains_key(&request.operation_id)
        );
        // Publication failpoints count members in the publisher's disk-ID
        // order, independently of their configured primary/backup positions.
        let members = engine.members();
        assert_eq!(members.len(), 3);
        let scans: Vec<_> = members
            .iter()
            .map(|disk| crate::journal::scan(disk).unwrap())
            .collect();
        let record = &scans[0]
            .records
            .iter()
            .rev()
            .find(|(record, _)| {
                record.record_type == RecordType::Publication && record.sequence > prefix_sequence
            })
            .unwrap()
            .0;
        let member = &members[0];
        let held = &engine.disk_resources.get(member).unwrap().root;
        let candidate: crate::journal::Candidate =
            crate::journal::load_object_at(held, member, "candidate", &record.candidate_hash)
                .unwrap();
        let publication: crate::journal::Publication =
            crate::journal::load_object_at(held, member, "publication", &record.object_hash)
                .unwrap();
        assert_eq!(candidate.operation_id, request.operation_id);
        assert!(candidate.sequence > prefix_sequence);
        assert_eq!(publication.result.operation_id, request.operation_id);
        assert_eq!(
            scans
                .iter()
                .flat_map(|scan| &scan.records)
                .filter(|(frame, _)| frame.record_type == RecordType::Publication
                    && frame.candidate_hash == record.candidate_hash)
                .count(),
            publication_count
        );
        let barrier_error = crate::publication_evidence::verified_publication_seal_at(
            &engine.disk_resources,
            &members,
            &candidate,
            &publication,
        )
        .unwrap_err();
        match barrier_error {
            Error::Filesystem(platform_io::fs_safety::Error::Io(source))
            | Error::Contract(storage_contract::Error::Io { source, .. }) => {
                assert_eq!(source.kind(), std::io::ErrorKind::NotFound)
            }
            other => panic!("missing publication barrier lost its I/O cause: {other:?}"),
        }
        // Only the on-disk manifest claims completion. No State receipt is
        // fabricated or installed; the real publication lacks the full barrier.
        let mut claimed = engine.workspace(&request.workspace_id).unwrap();
        claimed.state = WorkspaceState::Committed;
        claimed.fencing_token += 1;
        assert!(
            crate::engine::native_workspace_commit_completed(
                &claimed,
                &request,
                &publication.result
            )
            .unwrap()
        );
        for path in manifests(&f, &request.workspace_id) {
            crate::disk::write_json(&path, &claimed, true).unwrap();
        }
        assert_eq!(engine.workspace_capacity("admin").principal.records.used, 2);
        assert_eq!(engine.workspace_capacity("admin").principal.cold_records, 0);
        // For both interruption points, the final publisher member has no
        // PUBLICATION for this candidate. Retain that journal without erasing
        // either of the one/two actual publication receipts being tested.
        assert!(
            scans[2]
                .records
                .iter()
                .all(|(frame, _)| frame.record_type != RecordType::Publication
                    || frame.candidate_hash != record.candidate_hash)
        );
        drop(engine);
        let missing_journal = crate::journal::log_path(&members[2]);
        let retained_journal = missing_journal.with_extension("retained");
        fs::rename(&missing_journal, &retained_journal).unwrap();
        let retained_bytes = fs::read(&retained_journal).unwrap();
        // Writer conversion would recreate the missing journal; replay would
        // truncate these tails or fill the missing publication replicas.
        for disk in &members[..2] {
            fs::OpenOptions::new()
                .append(true)
                .open(crate::journal::log_path(disk))
                .unwrap()
                .write_all(b"ZXS")
                .unwrap();
        }
        let roots: Vec<_> = std::iter::once(f.root.clone())
            .chain(f.config.disks.iter().map(|disk| disk.root.clone()))
            .collect();
        let before: Vec<_> = roots.iter().map(|root| evidence_entries(root)).collect();
        let restored = f.temp.path().join("unproven-restore");
        let disks: Vec<_> = f
            .config
            .disks
            .iter()
            .map(|disk| disk.root.clone())
            .collect();
        for mode in ["open", "preserve", "restore"] {
            let result = match mode {
                "open" => Engine::open(&f.root),
                "preserve" => Engine::open_for_recovery(&f.root),
                "restore" => Engine::restore_from_disks(&restored, &disks),
                _ => unreachable!(),
            };
            let error = result
                .err()
                .expect("unproven terminal record must consume resident quota");
            assert_eq!(error.code(), ErrorCode::ResourceLimit, "{point}: {mode}");
            assert!(error.message().contains("global record count"));
            assert!(
                !missing_journal.exists(),
                "writer conversion ran before quota refusal"
            );
            assert_eq!(fs::read(&retained_journal).unwrap(), retained_bytes);
            assert_eq!(
                roots
                    .iter()
                    .map(|root| evidence_entries(root))
                    .collect::<Vec<_>>(),
                before
            );
            assert!(!restored.join("config.json").exists());
            assert!(!restored.join("index.sqlite").exists());
            for path in manifests(&f, &request.workspace_id) {
                assert_eq!(fs::read(path).unwrap(), canonical_json(&claimed).unwrap());
            }
            assert_eq!(
                fs::read(&staged.source_path).unwrap(),
                b"unproven publication FULL"
            );
            assert_eq!(
                fs::read(&anchor_full.source_path).unwrap(),
                b"unpublished quota anchor"
            );
        }
        assert!(
            fs::read_dir(restored)
                .unwrap()
                .all(|entry| entry.unwrap().file_name() == "writer.lock")
        );
    }
}
