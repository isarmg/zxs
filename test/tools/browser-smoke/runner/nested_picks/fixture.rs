//! Fixed, private evidence from the actual root and FILE producers.
use super::{Evidence, LEFT, LEFT_BODY, RIGHT, RIGHT_BODY, Result, SKIP, SKIP_BODY};
use name_policy::PortablePath;
use std::cell::Cell;
use storage_contract::{
    OperationId, Principal,
    file_draft::{BirthRequest, CommitMode, DraftCommand, DraftLocation, UnbornBody},
    snapshot_worktree::{
        InitializeSnapshot,
        draft_commit::{
            DirectoryDraftExpectation, DirectoryUnbornExpectation, SelectedDirectoryDraft,
        },
    },
};
use storage_engine::{CommitRequest, Engine, NamespaceOperation, Repository};

fn birth(
    engine: &mut Engine,
    repo: &Repository,
    actor: &Principal,
    path: &str,
) -> Result<DraftLocation> {
    let request = BirthRequest {
        repository: repo.repository_id.parse()?,
        path: PortablePath::parse(path)?,
        expected_namespace_head: engine.state().branches[&repo.main_branch_id]
            .head_commit_id
            .parse()?,
    };
    let operation = OperationId::generate()?;
    engine.create_unborn_file(&request, actor, operation)?;
    let (file, object) = engine
        .state()
        .file_containers
        .iter()
        .find(|(_, f)| f.created_by == operation)
        .ok_or("missing real birth operation")?;
    Ok(DraftLocation {
        repository: request.repository,
        path: request.path,
        entry: object.namespace.entry,
        file: *file,
    })
}

fn write(
    engine: &mut Engine,
    actor: &Principal,
    target: &DraftLocation,
    body: &[u8],
) -> Result<()> {
    let mut handle = engine.open_native_working(target, actor, true)?;
    if engine.write_native_working(&mut handle, actor, 0, body)? != body.len() {
        return Err("short ordinary write in the nested browser fixture".into());
    }
    engine
        .flush_native_working(&mut handle, actor)?
        .ok_or("the real ordinary save has no original operation")?;
    Ok(())
}

pub(super) fn seed(engine: &mut Engine, repo: &Repository, actor: &Principal) -> Result<Evidence> {
    for dir in ["pick-left", "pick-right"] {
        engine.file_namespace(
            &repo.repository_id,
            NamespaceOperation::Mkdir {
                path: PortablePath::parse(dir)?,
            },
            actor.as_str(),
            OperationId::generate()?,
        )?;
    }
    let ws = engine.create_workspace(&repo.main_branch_id, actor.as_str())?;
    for (path, bytes) in [
        (LEFT, &b"old"[..]),
        ("pick-right/anchor.txt", &b"do not select"[..]),
    ] {
        engine.stage_bytes(&ws.workspace_id, path, bytes)?;
    }
    engine.commit_workspace(CommitRequest {
        workspace_id: ws.workspace_id,
        operation_id: OperationId::generate()?,
        principal: actor.to_string(),
        expected_head: ws.base_head,
        fencing_token: ws.fencing_token,
        message: "real nested selection sources".into(),
        request_digest: None,
    })?;
    let repository = repo.repository_id.parse()?;
    let context = engine.snapshot_root_context(repository)?;
    let source = engine.initialize_directory_snapshot(
        &InitializeSnapshot {
            host: context.host,
            source: context.source,
            operation_id: OperationId::generate()?,
        },
        actor,
    )?;
    let entry = engine
        .list_tree(&repo.main_branch_id)?
        .into_iter()
        .find(|e| e.path() == LEFT)
        .ok_or("missing original nested FILE")?;
    let left = DraftLocation {
        repository,
        path: PortablePath::parse(LEFT)?,
        entry: entry.entry_id.parse()?,
        file: entry.file_id.parse()?,
    };
    let old_version = entry.version_id;
    let right = birth(engine, repo, actor, RIGHT)?;
    let skipped = birth(engine, repo, actor, SKIP)?;
    let current = engine.file_working_state(&left)?;
    if current.mode() != CommitMode::Manual {
        engine.change_file_working(
            &DraftCommand::SetMode {
                target: left.clone(),
                expected: current.revision(),
                mode: CommitMode::Manual.into(),
            },
            actor,
            OperationId::generate()?,
        )?;
    }
    write(engine, actor, &left, LEFT_BODY)?;
    write(engine, actor, &right, RIGHT_BODY)?;
    write(engine, actor, &skipped, SKIP_BODY)?;
    let w = engine.file_working_state(&left)?;
    let (base, payload) = w.draft().ok_or("expected a real Born working save")?;
    let left = SelectedDirectoryDraft {
        target: left,
        expected: DirectoryDraftExpectation::Born {
            revision: w.revision(),
            base: *base,
            payload: *payload,
        },
    };
    let w = engine.unborn_working_state(&right)?;
    let UnbornBody::Saved { manifest } = w.body() else {
        return Err("expected a real saved Unborn".into());
    };
    let right = SelectedDirectoryDraft {
        target: right,
        expected: DirectoryDraftExpectation::Unborn {
            revision: w.revision(),
            content: DirectoryUnbornExpectation::Saved {
                generation: manifest.generation(),
                commitment: manifest.commitment(),
            },
        },
    };
    let skipped_revision = engine.unborn_working_state(&skipped)?.revision();
    Ok(Evidence {
        source,
        actor: actor.clone(),
        selected: [left, right],
        skipped,
        skipped_revision,
        old_version,
        original: Cell::new(None),
    })
}
