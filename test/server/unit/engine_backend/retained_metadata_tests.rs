//! Current credential admission remains before metadata mutation and receipt replay.
use super::*;
use crate::{Backend, NoteTarget, RenameFileBranch, UpdateNote};
use std::sync::Arc;
use storage_contract::{BranchName, CommitRequest, Entry, ErrorCode, Note, OperationId};

fn op() -> OperationId {
    OperationId::generate().unwrap()
}
fn saved_file(engine: &mut Engine, slot: &str, name: &str) -> (Entry, String) {
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse(name).unwrap(),
            slot,
            1,
            "owner",
            op(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", b"saved")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: op(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "metadata authorization fixture".into(),
            request_digest: None,
        })
        .unwrap();
    let entry = engine
        .namespace_tree(&repo.main_branch_id)
        .unwrap()
        .get("file.txt")
        .unwrap()
        .clone();
    // An ordinary repository's write context can name its full main branch.
    // Label edits require an independent public branch bound to this file.
    let branch = engine
        .create_file_branch(
            &entry.file_id,
            &entry.version_id,
            &BranchName::parse("metadata branch").unwrap(),
            "owner",
            op(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    assert_eq!(
        engine.state().branches[&branch].file_id.as_deref(),
        Some(entry.file_id.as_str())
    );
    (entry, branch)
}
fn file_note(file: &str, operation_id: OperationId) -> UpdateNote {
    UpdateNote {
        target: NoteTarget::File(file.into()),
        note: Note::parse("current metadata".into()).unwrap(),
        operation_id,
    }
}
fn version_note(version: &str, operation_id: OperationId) -> UpdateNote {
    UpdateNote {
        target: NoteTarget::Version(version.into()),
        note: Note::parse("historical metadata".into()).unwrap(),
        operation_id,
    }
}
fn rename(file: &str, branch: &str, operation_id: OperationId) -> RenameFileBranch {
    RenameFileBranch {
        file_id: file.into(),
        branch_id: branch.into(),
        name: BranchName::parse("public label").unwrap(),
        operation_id,
    }
}

#[test]
fn retained_metadata_d3_preserves_current_authorization_for_edits_and_receipts() {
    let root = crate::tests::private_tempdir();
    let mut config =
        storage_engine::Config::development((0..4).map(|n| root.path().join(format!("disk-{n}"))))
            .unwrap();
    config.disks[3].level = 1;
    config.disks[3].upstream_slot_id = None;
    config.disks[3].metadata_member = false;
    let target = config.disks[0].disk_id.clone();
    let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    let (live, live_branch) = saved_file(&mut engine, "D04", "retained");
    let (hidden, hidden_branch) = saved_file(&mut engine, "P01", "historical");
    let backend = Arc::new(EngineBackend::new(engine));
    let mut auth = backend
        .protect_auth(
            crate::auth::AuthStore::initialize(
                &root.path().join("authentication"),
                "owner",
                "integration-test-password",
            )
            .unwrap(),
        )
        .unwrap();
    let token = auth
        .login("owner", "integration-test-password")
        .unwrap()
        .token;
    let identity = auth.authenticate(&token).unwrap();
    {
        let mut engine = backend.engine.lock().unwrap();
        engine
            .begin_disk_exit(
                &target,
                storage_engine::MembershipExitAction::Deregister,
                "owner",
                op(),
            )
            .unwrap();
        let archive = engine.build_managed_membership_control_archive().unwrap();
        engine
            .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
            .unwrap();
        engine.resume_membership_joint_decision().unwrap();
        engine.activate_pending_disk_exit().unwrap();
    }
    let note_id = op();
    let version_id = op();
    let rename_id = op();
    let file_receipt = backend
        .update_note(&identity, file_note(&live.file_id, note_id))
        .unwrap();
    let version_receipt = backend
        .update_note(&identity, version_note(&hidden.version_id, version_id))
        .unwrap();
    let rename_receipt = backend
        .rename_file_branch(&identity, rename(&live.file_id, &live_branch, rename_id))
        .unwrap();
    assert_eq!(
        backend
            .update_note(&identity, file_note(&hidden.file_id, op()))
            .unwrap_err()
            .code,
        ErrorCode::FileNotFound
    );
    assert_eq!(
        backend
            .rename_file_branch(&identity, rename(&hidden.file_id, &hidden_branch, op()))
            .unwrap_err()
            .code,
        ErrorCode::FileBranchNotFound
    );
    assert_eq!(
        backend
            .update_note(&identity, file_note(&live.file_id, note_id))
            .unwrap(),
        file_receipt
    );
    assert_eq!(
        backend
            .update_note(&identity, version_note(&hidden.version_id, version_id))
            .unwrap(),
        version_receipt
    );
    assert_eq!(
        backend
            .rename_file_branch(&identity, rename(&live.file_id, &live_branch, rename_id))
            .unwrap(),
        rename_receipt
    );

    let stranger = Identity {
        principal: "stranger".into(),
        ..identity.clone()
    };
    let before = json_codec::to_value(backend.engine.lock().unwrap().state()).unwrap();
    for denied in [
        backend.update_note(&stranger, file_note(&live.file_id, op())),
        backend.update_note(&stranger, version_note(&hidden.version_id, op())),
        backend.rename_file_branch(&stranger, rename(&live.file_id, &live_branch, op())),
    ] {
        assert_eq!(denied.unwrap_err().code, ErrorCode::Unauthorized);
    }
    assert_eq!(
        json_codec::to_value(backend.engine.lock().unwrap().state()).unwrap(),
        before
    );

    auth.logout(&token).unwrap();
    let after_revocation = json_codec::to_value(backend.engine.lock().unwrap().state()).unwrap();
    for denied in [
        backend.update_note(&identity, file_note(&live.file_id, op())),
        backend.update_note(&identity, version_note(&hidden.version_id, op())),
        backend.rename_file_branch(&identity, rename(&live.file_id, &live_branch, op())),
        backend.update_note(&identity, file_note(&live.file_id, note_id)),
        backend.update_note(&identity, version_note(&hidden.version_id, version_id)),
        backend.rename_file_branch(&identity, rename(&live.file_id, &live_branch, rename_id)),
    ] {
        assert_eq!(denied.unwrap_err().code, ErrorCode::Unauthorized);
    }
    assert_eq!(
        json_codec::to_value(backend.engine.lock().unwrap().state()).unwrap(),
        after_revocation
    );
}
