//! Bounded real-body acceptance: engine drafts and private SMB copies are separate.
use super::*;
use std::os::unix::fs::MetadataExt;

const BODY_BYTES: usize = 512 * 1024;
const ROUNDS: u8 = 8;

#[derive(Debug, Default)]
struct Usage {
    bodies: u64,
    logical: u64,
    allocated: u64,
    other_logical: u64,
    other_allocated: u64,
}

fn usage(path: &Path, engine_store: bool) -> Usage {
    fn walk(path: &Path, engine_store: bool, result: &mut Usage) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let metadata = fs::symlink_metadata(entry.path()).unwrap();
            assert!(!metadata.file_type().is_symlink());
            if metadata.is_dir() {
                walk(&entry.path(), engine_store, result);
                continue;
            }
            assert!(metadata.is_file());
            let name = entry.file_name();
            let name = name.to_str().unwrap();
            let body = if engine_store {
                name == "body"
            } else {
                name.starts_with("manual-") && !name.contains('.')
            };
            if body {
                result.bodies += 1;
                result.logical += metadata.len();
                result.allocated += metadata.blocks() * 512;
            } else {
                result.other_logical += metadata.len();
                result.other_allocated += metadata.blocks() * 512;
            }
        }
    }
    let mut result = Usage::default();
    walk(path, engine_store, &mut result);
    result
}

#[test]
fn repeated_large_manual_flushes_release_superseded_engine_bodies_after_readers_close() {
    let (root, mut shared, backend, branch) = manual_fixture();
    // The generic tiny-file fixture reserves only 4 KiB per file/session.
    // This bounded acceptance uses 512 KiB bodies inside the unchanged
    // 16 MiB account / 64 MiB global spool limits, not product quota changes.
    Arc::get_mut(&mut shared).unwrap().max_file_bytes = BODY_BYTES as u64;
    let actor = identity(&shared);
    let entry = session(shared.clone(), &branch).entries().unwrap().remove(0);
    let context = crate::ReadContainerEdit {
        file: entry.file_id.parse().unwrap(),
        repository: repository(&shared).parse().unwrap(),
        path: entry.path.clone(),
    };
    shared.backend.set_container_commit_mode(&actor, crate::SetContainerCommitMode {
        context: context.clone(),
        expected_generation: storage_contract::Generation::new(0),
        mode: storage_contract::container_edit::CommitMode::Manual,
        operation_id: OperationId::generate().unwrap(),
    }).unwrap();
    let before = backend.inner.test_with_engine_locked(|engine| {
        (engine.state().versions.clone(), engine.state().commits.clone(),
            engine.state().branches.clone())
    });
    let store = root.path().join("p/container-drafts-v1");
    let mut first_allocation = None;
    let mut operations = Vec::new();
    for index in 1..=ROUNDS {
        let mut client = session(shared.clone(), &branch);
        client.max_file_bytes = BODY_BYTES as u64;
        let (opened, descriptor) = client.open(
            "report.txt".into(), true, false, false, false, false,
        ).unwrap();
        let file = File::from(descriptor);
        if index > 1 {
            let mut previous = vec![0; BODY_BYTES];
            file.read_exact_at(&mut previous, 0).unwrap();
            assert_eq!(previous, vec![index - 1; BODY_BYTES]);
        }
        let bytes = vec![index; BODY_BYTES];
        client.prepare_write(opened.handle, BODY_BYTES as u64, BODY_BYTES as u64).unwrap();
        file.write_all_at(&bytes, 0).unwrap();
        client.dirty(opened.handle).unwrap();
        let result = client.flush(opened.handle).unwrap();
        assert_eq!(result["draft_persisted"], true);
        assert!(result.get("commit_id").is_none());
        operations.push(result["operation_id"].as_str().unwrap().to_owned());
        client.close(opened.handle, false).unwrap();
        drop(file);
        drop(client);
        let engine_usage = usage(&store, true);
        let spool_usage = usage(&shared.spool, false);
        eprintln!("round={index} engine={engine_usage:?} SMB-frozen={spool_usage:?}");
        if index == 1 {
            first_allocation = Some(engine_usage.allocated);
        }
        assert_eq!(spool_usage.bodies, 0, "private frozen copies must be retired separately");
    }
    backend.inner.test_with_engine_locked(|engine| {
        assert_eq!(engine.state().versions, before.0);
        assert_eq!(engine.state().commits, before.1);
        assert_eq!(engine.state().branches, before.2);
        for operation in &operations {
            assert!(engine.state().operations.contains_key(&operation.parse().unwrap()));
        }
    });
    drop((shared, backend));
    let engine = Engine::open(&root.path().join("state")).unwrap();
    let mut current = engine.open_container_draft(context.file).unwrap().unwrap();
    let mut bytes = Vec::new();
    current.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, vec![ROUNDS; BODY_BYTES]);
    drop(current);
    assert_eq!(engine.state().versions, before.0);
    assert_eq!(engine.state().commits, before.1);
    assert_eq!(engine.state().branches, before.2);
    let engine_usage = usage(&store, true);
    // No reader remains pinned. Check pathname retention and physical block
    // accounting separately; unlink alone cannot establish physical release.
    assert_eq!(engine_usage.bodies, 1,
        "superseded engine bodies accumulated: {engine_usage:?}");
    assert_eq!(engine_usage.logical, BODY_BYTES as u64);
    let baseline = first_allocation.unwrap();
    if baseline > 0 {
        assert!(engine_usage.allocated <= baseline + 4096,
            "allocated engine body blocks did not return to one-body bound: {engine_usage:?}");
    }
}

#[test]
fn engine_draft_quota_failure_preserves_selected_body_receipt_and_cold_history() {
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("state");
    let disk = root.path().join("disk");
    for path in [&system, &disk] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut config = Config::development([disk.clone()]).unwrap();
    config.max_file_bytes = 2 * 1024 * 1024;
    config.max_workspace_bytes = 2 * 1024 * 1024;
    let mut engine = Engine::initialize_manual_container_storage(
        &system, config, OperationId::generate().unwrap(),
    ).unwrap();
    let repo = engine.create_repository(
        &storage_contract::RepositoryName::parse("quota").unwrap(), "P01", 1,
        "alice", OperationId::generate().unwrap(),
    ).unwrap();
    let workspace = engine.create_workspace(&repo.main_branch_id, "alice").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "report.txt", b"original").unwrap();
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate().unwrap(), principal: "alice".into(),
        expected_head: workspace.base_head, fencing_token: workspace.fencing_token,
        message: "original".into(), request_digest: None,
    }).unwrap();
    let entry = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    let file = entry.file_id.parse().unwrap();
    let principal = storage_contract::Principal::new("alice".into()).unwrap();
    engine.set_container_commit_mode(&storage_contract::container_edit::SetCommitMode {
        file, expected_generation: storage_contract::Generation::new(0),
        mode: storage_contract::container_edit::CommitMode::Manual,
    }, &principal, OperationId::generate().unwrap()).unwrap();
    let base = engine.current_container_draft_base(
        file, repo.repository_id.parse().unwrap(), entry.entry_id.parse().unwrap(),
    ).unwrap();
    let bytes = vec![7u8; BODY_BYTES];
    let source = root.path().join("upload");
    fs::write(&source, &bytes).unwrap();
    let revision = storage_contract::container_edit::DraftRevision {
        operation: OperationId::generate().unwrap(), principal: principal.clone(),
        expected_generation: engine.container_edit(file).unwrap().generation(),
        base, content_hash: storage_contract::ContentHash::from_digest(blake3::hash(&bytes)),
        size: bytes.len() as u64,
    };
    let receipt = engine.save_container_draft(
        file, &revision, File::open(&source).unwrap(),
    ).unwrap();
    let before = engine.state().clone();
    let selected = engine.open_container_draft(file).unwrap().unwrap();
    let mut selected_reader = selected.file;
    let oversized = vec![9u8; 2 * 1024 * 1024];
    fs::write(&source, &oversized).unwrap();
    let mut rejected = revision.clone();
    rejected.operation = OperationId::generate().unwrap();
    rejected.expected_generation = engine.container_edit(file).unwrap().generation();
    rejected.size = oversized.len() as u64;
    rejected.content_hash = storage_contract::ContentHash::from_digest(blake3::hash(&oversized));
    let error = engine.save_container_draft(
        file, &rejected, File::open(&source).unwrap(),
    ).unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::ResourceLimit);
    assert_eq!(engine.state(), &before);
    assert_eq!(engine.container_draft_save_receipt(file, &revision, &principal).unwrap(),
        Some(receipt.clone()));
    assert!(engine.container_draft_save_receipt(file, &rejected, &principal).unwrap().is_none());
    let mut retained = Vec::new();
    selected_reader.read_to_end(&mut retained).unwrap();
    assert_eq!(retained, bytes, "quota failure cannot invalidate an existing reader");
    drop(selected_reader);
    drop(engine);
    let engine = Engine::open(&system).unwrap();
    assert_eq!(engine.state(), &before);
    let mut selected = engine.open_container_draft(file).unwrap().unwrap();
    let mut retained = Vec::new();
    selected.file.read_to_end(&mut retained).unwrap();
    assert_eq!(retained, bytes);
    assert_eq!(engine.container_draft_save_receipt(file, &revision, &principal).unwrap(),
        Some(receipt));
}
