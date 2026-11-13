use crate::{CommitRequest, Config, Engine, OperationId};
use storage_contract::container_edit::{CommitMode, DraftRevision, SetCommitMode};
use storage_contract::{ContentHash, FileId, Generation, Principal};
use std::{fs, io::Read, os::unix::fs::PermissionsExt};

#[test]
fn manual_flush_is_shared_after_cold_open_without_publishing_history() {
    exercise_manual_commit(false, false);
}

#[test]
fn explicit_public_file_commit_consumes_exact_shared_draft_after_cold_open() {
    exercise_manual_commit(true, false);
}

#[test]
fn explicit_file_commit_materializes_selected_historical_branch_only_after_draft_save() {
    exercise_manual_commit(true, true);
}

fn exercise_manual_commit(public: bool, historical: bool) {
    let temporary = tempfile::tempdir().unwrap();
    let system = temporary.path().join("system");
    let disk = temporary.path().join("disk");
    for path in [&system, &disk] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let config = Config::development([disk]).unwrap();
    let mut engine = Engine::initialize_manual_container_storage(
        &system, config, OperationId::generate().unwrap(),
    ).unwrap();
    let principal = Principal::new("alice".into()).unwrap();
    let repository = engine.create_repository(
        &storage_contract::RepositoryName::parse("files").unwrap(),
        "P01", 1, principal.as_str(), OperationId::generate().unwrap(),
    ).unwrap();
    let workspace = engine.create_workspace(&repository.main_branch_id, principal.as_str())
        .unwrap();
    engine.stage_bytes(&workspace.workspace_id, "note.txt", b"original").unwrap();
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate().unwrap(),
        principal: principal.as_str().to_owned(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "original".into(),
        request_digest: None,
    }).unwrap();
    let entry = engine.list_tree(&repository.main_branch_id).unwrap().remove(0);
    let file: FileId = entry.file_id.parse().unwrap();
    let selected_history = if historical {
        let newer = engine.create_workspace(&repository.main_branch_id, principal.as_str())
            .unwrap();
        engine.stage_bytes(&newer.workspace_id, "note.txt", b"newer history").unwrap();
        engine.commit_workspace(CommitRequest {
            workspace_id: newer.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: principal.as_str().to_owned(),
            expected_head: newer.base_head,
            fencing_token: newer.fencing_token,
            message: "advance before selecting old node".into(),
            request_digest: None,
        }).unwrap();
        let branch = engine.create_file_branch_from_node(
            &entry.file_id, &entry.version_id, &entry.version_id,
            &storage_contract::BranchName::parse("historical manual draft").unwrap(),
            principal.as_str(), OperationId::generate().unwrap(),
        ).unwrap().result["branch_id"].as_str().unwrap().to_owned();
        engine.select_file_branch_at(
            &repository.repository_id, entry.location(), &entry.file_id, &branch,
            principal.as_str(), OperationId::generate().unwrap(),
        ).unwrap();
        assert_eq!(engine.state().branches[&branch].pending_file_versions[&entry.file_id],
            entry.version_id);
        Some(branch)
    } else {
        None
    };
    assert_eq!(engine.container_edit(file).unwrap().mode(), CommitMode::Automatic);
    engine.set_container_commit_mode(
        &SetCommitMode { file, expected_generation: Generation::new(0), mode: CommitMode::Manual },
        &principal, OperationId::generate().unwrap(),
    ).unwrap();
    assert_installed_edit(
        temporary.path().join("disk").as_path(), file, &engine.container_edit(file).unwrap(),
    );
    let versions = engine.state().versions.clone();
    let commits = engine.state().commits.clone();
    let branches = engine.state().branches.clone();
    let base = engine.current_container_draft_base(
        file, repository.repository_id.parse().unwrap(), entry.entry_id.parse().unwrap(),
    ).unwrap();
    let bodies = [b"first draft".as_slice(), b"second draft".as_slice()];
    for (index, bytes) in bodies.iter().enumerate() {
        let source = temporary.path().join("source");
        fs::write(&source, bytes).unwrap();
        let revision = DraftRevision {
            operation: OperationId::generate().unwrap(),
            expected_generation: engine.container_edit(file).unwrap().generation(),
            principal: principal.clone(),
            base: base.clone(),
            content_hash: ContentHash::from_digest(blake3::hash(bytes)),
            size: bytes.len() as u64,
        };
        let published = engine.save_container_draft(
            file, &revision, fs::File::open(&source).unwrap(),
        ).unwrap();
        assert_installed_edit(
            temporary.path().join("disk").as_path(), file, &engine.container_edit(file).unwrap(),
        );
        assert_eq!(engine.state().versions, versions);
        assert_eq!(engine.state().commits, commits);
        assert_eq!(engine.state().branches, branches);
        drop(engine);
        engine = Engine::open(&system).unwrap();
        let mut draft = engine.open_container_draft(file).unwrap().unwrap();
        let mut actual = Vec::new();
        draft.file.read_to_end(&mut actual).unwrap();
        assert_eq!(actual.as_slice(), *bytes, "flush {index}");
        assert_eq!(draft.revision, revision);
        let retry = engine.save_container_draft(
            file, &revision, fs::File::open(&source).unwrap(),
        ).unwrap();
        assert_eq!(retry, published);
        let mut original = engine.open_content(&entry.version_id).unwrap();
        let mut historical = Vec::new();
        original.file.read_to_end(&mut historical).unwrap();
        assert_eq!(historical, b"original");
        assert!(engine.set_container_commit_mode(
            &SetCommitMode {
                file,
                expected_generation: engine.container_edit(file).unwrap().generation(),
                mode: CommitMode::Automatic,
            },
            &principal, OperationId::generate().unwrap(),
        ).is_err());
    }
    let dirty = engine.container_edit(file).unwrap();
    let draft = dirty.draft().unwrap().operation;
    let workspace = engine.create_workspace(&repository.main_branch_id, principal.as_str())
        .unwrap();
    engine.stage_bytes(&workspace.workspace_id, "note.txt", b"second draft").unwrap();
    engine.seal_workspace(&workspace.workspace_id, principal.as_str()).unwrap();
    let workspace = engine.workspace(&workspace.workspace_id).unwrap();
    let mut request = CommitRequest {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: OperationId::generate().unwrap(),
        principal: principal.as_str().to_owned(),
        expected_head: workspace.base_head.clone(),
        fencing_token: workspace.fencing_token,
        message: "explicit draft commit".into(),
        request_digest: None,
    };
    // Matching bytes alone cannot turn a normal flush into an explicit commit.
    assert!(engine.commit_workspace(request.clone()).is_err());
    assert_eq!(engine.container_edit(file).unwrap(), dirty);
    let envelope = if public {
        let context = storage_contract::file_container_tags::Context {
            repository_id: repository.repository_id.parse().unwrap(),
            path: name_policy::PortablePath::parse("note.txt").unwrap(),
        };
        let sealed = engine.prepare_shared_container_draft_commit(
            file, &context, dirty.generation(), draft, &principal, request.operation_id,
        ).unwrap();
        let prepared_count = engine.workspaces.len();
        let repeated = engine.prepare_shared_container_draft_commit(
            file, &context, dirty.generation(), draft, &principal, request.operation_id,
        ).unwrap();
        assert_eq!(
            crate::canonical_json(&sealed).unwrap(), crate::canonical_json(&repeated).unwrap(),
        );
        assert_eq!(engine.workspaces.len(), prepared_count);
        request.workspace_id = sealed.workspace_id.clone();
        request.expected_head = sealed.expected_head.clone();
        request.fencing_token = sealed.fencing_token;
        let mut body = json_codec::to_value(sealed).unwrap();
        body["message"] = json_codec::Value::String(request.message.clone());
        let digest = crate::wire::commit_request_digest(&request.workspace_id, &body).unwrap();
        body["request_digest"] = json_codec::Value::String(digest);
        let mut altered = body.clone();
        altered["container_draft_commit"]["operation"] =
            json_codec::Value::String(OperationId::generate().unwrap().to_string());
        let changed_digest = crate::wire::commit_request_digest(
            &request.workspace_id, &altered,
        ).unwrap();
        altered["request_digest"] = json_codec::Value::String(changed_digest);
        let altered = crate::PublicCommitEnvelope::parse(&request.workspace_id, altered).unwrap();
        assert!(engine.commit_workspace_wire(request.clone(), altered).is_err());
        assert_eq!(engine.container_edit(file).unwrap(), dirty);
        assert_eq!(engine.state().versions, versions);
        let envelope = crate::PublicCommitEnvelope::parse(&request.workspace_id, body).unwrap();
        Some(envelope)
    } else {
        engine.prepare_container_draft_commit(&request, file, dirty.generation(), draft).unwrap();
        None
    };
    drop(engine);
    let mut engine = Engine::open(&system).unwrap();
    let published = match envelope.clone() {
        Some(envelope) => engine.commit_workspace_wire(request.clone(), envelope).unwrap(),
        None => engine.commit_workspace(request.clone()).unwrap(),
    };
    assert!(engine.open_container_draft(file).unwrap().is_none());
    assert_eq!(engine.container_edit(file).unwrap().mode(), CommitMode::Manual);
    assert_eq!(engine.state().versions.len(), versions.len() + 1);
    if let Some(selected) = &selected_history {
        let branch = &engine.state().branches[selected];
        assert!(!branch.pending_file_versions.contains_key(&entry.file_id));
        assert!(branch.file_node_id.is_none());
        assert_eq!(engine.state().branches.len(), branches.len());
        let latest = engine.list_tree(&repository.main_branch_id).unwrap().remove(0);
        assert_ne!(latest.version_id, entry.version_id);
        assert_eq!(engine.state().file_node_parents[&latest.version_id],
            vec![entry.version_id.clone()]);
    }
    let committed_versions = engine.state().versions.clone();
    let committed_branches = engine.state().branches.clone();
    drop(engine);
    let mut engine = Engine::open(&system).unwrap();
    let replay = match envelope {
        Some(envelope) => engine.commit_workspace_wire(request, envelope).unwrap(),
        None => {
            engine.prepare_container_draft_commit(&request, file, dirty.generation(), draft)
                .unwrap();
            engine.commit_workspace(request).unwrap()
        }
    };
    assert_eq!(replay, published);
    assert_eq!(engine.state().versions, committed_versions);
    assert_eq!(engine.state().branches, committed_branches);
    assert_installed_edit(
        temporary.path().join("disk").as_path(), file, &engine.container_edit(file).unwrap(),
    );
    let mut original = engine.open_content(&entry.version_id).unwrap();
    let mut historical = Vec::new();
    original.file.read_to_end(&mut historical).unwrap();
    assert_eq!(historical, b"original");
    let source = temporary.path().join("discard-source");
    fs::write(&source, b"discard this draft").unwrap();
    let revision = DraftRevision {
        operation: OperationId::generate().unwrap(),
        expected_generation: engine.container_edit(file).unwrap().generation(),
        principal: principal.clone(),
        base: engine.current_container_draft_base(
            file, repository.repository_id.parse().unwrap(), entry.entry_id.parse().unwrap(),
        ).unwrap(),
        content_hash: ContentHash::from_digest(blake3::hash(b"discard this draft")),
        size: b"discard this draft".len() as u64,
    };
    assert!(engine.container_draft_save_receipt(file, &revision, &principal).unwrap().is_none());
    let saved = engine.save_container_draft(
        file, &revision, fs::File::open(source).unwrap(),
    ).unwrap();
    let dirty = engine.container_edit(file).unwrap();
    engine.discard_container_draft(
        file, dirty.generation(), revision.operation, &principal, OperationId::generate().unwrap(),
    ).unwrap();
    drop(engine);
    let engine = Engine::open(&system).unwrap();
    assert!(engine.open_container_draft(file).unwrap().is_none());
    let before_query = engine.state().clone();
    assert_eq!(engine.container_draft_save_receipt(file, &revision, &principal).unwrap(),
        Some(saved));
    let mut changed = revision.clone();
    changed.size += 1;
    assert!(engine.container_draft_save_receipt(file, &changed, &principal).is_err());
    let other = Principal::new("other".into()).unwrap();
    assert!(engine.container_draft_save_receipt(file, &revision, &other).is_err());
    assert_eq!(*engine.state(), before_query);
    assert_eq!(engine.state().versions, committed_versions);
    assert_eq!(engine.state().branches, committed_branches);
}

// Read the installed FILE bytes, not a fresh projection reconstructed from SYSTEM.
fn assert_installed_edit(
    root: &std::path::Path,
    file: FileId,
    expected: &storage_contract::container_edit::ContainerEdit,
) {
    use sqlite_access::{Connection, Row, SqliteConnection};
    use sqlite_access::sqlite::{SqliteConnectOptions, SqliteOwnedBuf};

    fn collect(path: &std::path::Path, databases: &mut Vec<std::path::PathBuf>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                collect(&entry.path(), databases);
            } else if entry.file_name() == ".zxs-file.sqlite" {
                databases.push(entry.path());
            }
        }
    }
    let mut databases = Vec::new();
    collect(root, &mut databases);
    assert!(!databases.is_empty());
    let mut matches = 0;
    for path in databases {
        let bytes = fs::read(path).unwrap();
        let body: Option<String> = platform_io::sqlite::block_on_sqlite_connection(async {
            let mut connection = SqliteConnection::connect_with(
                &SqliteConnectOptions::new().in_memory(true),
            ).await.unwrap();
            connection.deserialize(None, SqliteOwnedBuf::try_from(bytes.as_slice()).unwrap(), true)
                .await.unwrap();
            let row = sqlite_access::query("SELECT body FROM records WHERE kind='file' AND id=?")
                .bind(file.to_string()).fetch_optional(&mut connection).await.unwrap();
            let body = row.map(|row| row.try_get("body").unwrap());
            connection.close().await.unwrap();
            body
        });
        if let Some(body) = body {
            let row: json_codec::Value = json_codec::from_str(&body).unwrap();
            let actual: storage_contract::container_edit::ContainerEdit =
                json_codec::from_value(row["container_edit"].clone()).unwrap();
            assert_eq!(&actual, expected);
            matches += 1;
        }
    }
    assert!(matches > 0);
}
