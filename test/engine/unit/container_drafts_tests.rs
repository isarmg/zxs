use crate::{CommitRequest, Config, Engine, OperationId};
use storage_contract::container_edit::{CommitMode, DraftRevision, SetCommitMode};
use storage_contract::{ContentHash, FileId, Generation, Principal};
use std::{fs, io::Read};

#[test]
fn manual_flush_is_shared_after_cold_open_without_publishing_history() {
    let temporary = tempfile::tempdir().unwrap();
    let system = temporary.path().join("system");
    let config = Config::development([temporary.path().join("disk")]).unwrap();
    let mut engine = Engine::initialize(&system, config).unwrap();
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
    let request = CommitRequest {
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
    engine.prepare_container_draft_commit(&request, file, dirty.generation(), draft).unwrap();
    drop(engine);
    let mut engine = Engine::open(&system).unwrap();
    let published = engine.commit_workspace(request.clone()).unwrap();
    assert!(engine.open_container_draft(file).unwrap().is_none());
    assert_eq!(engine.container_edit(file).unwrap().mode(), CommitMode::Manual);
    assert_eq!(engine.state().versions.len(), versions.len() + 1);
    let committed_versions = engine.state().versions.clone();
    let committed_branches = engine.state().branches.clone();
    drop(engine);
    let mut engine = Engine::open(&system).unwrap();
    engine.prepare_container_draft_commit(&request, file, dirty.generation(), draft).unwrap();
    assert_eq!(engine.commit_workspace(request).unwrap(), published);
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
    engine.save_container_draft(file, &revision, fs::File::open(source).unwrap()).unwrap();
    let dirty = engine.container_edit(file).unwrap();
    engine.discard_container_draft(
        file, dirty.generation(), revision.operation, &principal, OperationId::generate().unwrap(),
    ).unwrap();
    drop(engine);
    let engine = Engine::open(&system).unwrap();
    assert!(engine.open_container_draft(file).unwrap().is_none());
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
