//! The existing transport-failure adapter drops only the reply, after the actual
//! Engine writer returns. Neither a synthetic receipt nor a recovery cap is used.
use super::*;
use storage_contract::file_draft::{CommitMode, DraftCommand, DraftLocation};

#[test]
fn lost_manual_reply_reuses_the_frozen_request_and_never_acknowledges_later_fd_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(&temp.path().join("state"), Config::development([
        (name_policy::Name::parse("manual owner").unwrap(), temp.path().join("p")),
    ]).unwrap()).unwrap();
    let repository = engine.create_disk_root("P01", "alice", OperationId::generate().unwrap())
        .unwrap();
    let workspace = engine.create_workspace(&repository.main_branch_id, "alice").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "report.txt", b"original").unwrap();
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        operation_id: OperationId::generate().unwrap(),
        principal: "alice".into(), message: "real automatic source".into(), request_digest: None,
    }).unwrap();
    let entry = engine.namespace_tree(&repository.main_branch_id).unwrap()
        .get("report.txt").unwrap().clone();
    let path = name_policy::PortablePath::parse("report.txt").unwrap();
    // The actual file-manager save already materialized its selected FILE branch.
    // Creating another branch here selects unwritten inheritance, not a draft base.
    let context = engine.file_context(&repository.repository_id, path.as_str()).unwrap();
    let branch = &engine.state().branches[&context.branch_id];
    assert_eq!(branch.branch_kind, storage_contract::BranchKind::File);
    assert_eq!(branch.file_id.as_deref(), Some(entry.file_id.as_str()));
    assert_eq!(branch.history_tree_id, repository.history_tree_id);
    assert_ne!(branch.branch_id, repository.main_branch_id);
    assert!(branch.pending_file_versions.is_empty());
    assert!(context.inherited_from.is_none());
    assert_eq!(engine.file_branch_head(&branch.branch_id).unwrap(), entry.version_id);
    let target = DraftLocation {
        repository: repository.repository_id.parse().unwrap(), path,
        entry: entry.entry_id.parse().unwrap(), file: entry.file_id.parse().unwrap(),
    };
    let working = engine.file_working_state(&target).unwrap();
    engine.change_file_working(&DraftCommand::SetMode {
        target: target.clone(), expected: working.revision(), mode: CommitMode::Manual.into(),
    }, &"alice".parse().unwrap(), OperationId::generate().unwrap()).unwrap();
    let (fixture, shared, backend) = fixture_backend(temp, engine);
    let mut session = named_session(shared.clone(), &repository.main_branch_id);
    let (opened, file) = open_file(&mut session, true);
    assert!(opened.workspace_id.is_none());
    let original = b"acknowledged by Engine but the response is lost";
    session.prepare_write(opened.handle, original.len() as u64, original.len() as u64).unwrap();
    file.write_all_at(original, 0).unwrap();
    file.set_len(original.len() as u64).unwrap();
    session.dirty(opened.handle).unwrap();
    let operation = match session.handles.get(&opened.handle).unwrap() {
        Handle::ManualWrite(writer, _) => writer.operation(),
        _ => panic!("real Manual container must not allocate a workspace writer"),
    };
    backend.fail.store(true, Ordering::SeqCst);
    let error = session.flush(opened.handle).unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::IoError);
    assert!(matches!(error.source.as_deref(), Some(crate::http_boundary::ApiErrorSource::Io(
        source,
    )) if source.kind() == io::ErrorKind::BrokenPipe));
    let published = backend.inner.operation_outcome(&session.identity, operation).unwrap();
    assert_eq!(published.kind, "file.draft.write");
    assert_eq!(published.commit_state, storage_contract::CommitState::Published);
    let saved = backend.inner.file_working(&identity(&shared), target.clone()).unwrap();
    // The SMB binding cannot turn the saved draft into a FILE node through
    // either the management adapter or the Write-only SMB adapter.
    let commit = crate::ChangeFileWorking {
        operation_id: OperationId::generate().unwrap(),
        command: DraftCommand::Commit {
            target: target.clone(), expected: saved.working.revision(), base: saved.base,
            message: "SMB identity cannot explicitly commit".into(),
        },
    };
    let error = backend.inner.change_file_working(&session.identity, commit.clone()).unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::Forbidden);
    let upload = crate::UploadSource::from_file(file.try_clone().unwrap(), 1024 * 1024).unwrap();
    let error = backend.inner.write_smb_draft(&session.identity, commit, &upload).unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::Forbidden);
    let source = shared.spool.join(format!("manual-{operation}"));
    let record = source.with_extension("work.json");
    let pending: Value = json_codec::from_slice(&fs::read(&record).unwrap()).unwrap();
    assert_eq!(pending["publication"]["state"], "pending");
    let frozen = pending["publication"]["request"].clone();
    assert_eq!(frozen["operation_id"], operation.to_string());
    let quota = fs::read(source.with_extension("budget.json")).unwrap();

    let later = b"later unacknowledged FD bytes";
    let error = session.prepare_write(opened.handle, later.len() as u64, later.len() as u64)
        .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::RecoveryRequired);
    // Inject the FD change that a peer can make despite the rejected RPC.
    file.write_all_at(later, 0).unwrap();
    file.set_len(later.len() as u64).unwrap();
    let error = session.flush(opened.handle).unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::FileBranchConflict);
    assert_eq!(backend.inner.operation_outcome(&session.identity, operation).unwrap(), published);
    assert_eq!(backend.inner.file_working(&identity(&shared), target.clone()).unwrap().working,
        saved.working);
    let reconciled: Value = json_codec::from_slice(&fs::read(&record).unwrap()).unwrap();
    assert_eq!(reconciled["publication"]["state"], "published");
    assert_eq!(reconciled["publication"]["request"], frozen);
    assert_eq!(fs::read(&source).unwrap(), later);
    assert_eq!(fs::read(source.with_extension("budget.json")).unwrap(), quota);
    assert_eq!(backend.inner.file_history(&identity(&shared), &entry.file_id).unwrap().len(), 1);

    // A new authorized Dirty is the only successor of the reconciled operation.
    session.dirty(opened.handle).unwrap();
    let next = session.flush(opened.handle).unwrap();
    assert_ne!(next["operation_id"], operation.to_string());
    assert_eq!(next["operation_kind"], "file.draft.write");
    let working = backend.inner.file_working(&identity(&shared), target.clone()).unwrap().working;
    assert_eq!(working.revision(), saved.working.revision().next().unwrap());
    let mut content = backend.inner.file_draft_content(&identity(&shared), crate::ReadFileDraft {
        target, deadline: std::time::Instant::now() + std::time::Duration::from_secs(30),
    }).unwrap();
    let mut bytes = Vec::new();
    content.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, later);
    session.close(opened.handle, false).unwrap();
    assert!(!source.exists());
    assert!(!record.exists());
    assert!(!source.with_extension("budget.json").exists());
    assert_eq!(backend.inner.file_history(&identity(&shared), &entry.file_id).unwrap().len(), 1);
    assert!(fixture.path().join("state").is_dir());
}
