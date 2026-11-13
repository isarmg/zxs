//! Real disk resources and immutable bodies. These tests never claim that a
//! local intent alone publishes a shared revision; engine publication owns that.
use super::*;
use crate::{Config, Engine};
use std::{collections::BTreeMap, fs, io::{Read, Seek, SeekFrom}};
use std::os::unix::fs::MetadataExt;
use storage_contract::{ContentHash, FileNodeId, OperationId};
use storage_contract::container_edit::DraftBase;

fn fixture() -> (tempfile::TempDir, Engine, FileId, DraftRevision) {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("p"), temp.path().join("b"), temp.path().join("t"),
    ]).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("state"), config).unwrap();
    let repo = engine.create_repository(
        &storage_contract::RepositoryName::parse("original").unwrap(), "P01", 3,
        "admin", OperationId::generate().unwrap(),
    ).unwrap();
    let ws = engine.create_workspace(&repo.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&ws.workspace_id, "file.txt", b"immutable history").unwrap();
    engine.commit_workspace(crate::CommitRequest {
        workspace_id: ws.workspace_id, operation_id: OperationId::generate().unwrap(),
        principal: "admin".into(), expected_head: ws.base_head, fencing_token: ws.fencing_token,
        message: "original".into(), request_digest: None,
    }).unwrap();
    let entry = engine.list_tree(&repo.main_branch_id).unwrap().into_iter()
        .find(|entry| entry.path() == "file.txt").unwrap();
    let version = engine.state().versions.get(&entry.version_id).unwrap();
    let revision = DraftRevision {
        operation: OperationId::generate().unwrap(), principal: "admin".parse().unwrap(),
        expected_generation: Generation::new(1),
        base: DraftBase {
            entry: entry.entry_id.parse().unwrap(),
            repository: repo.repository_id.parse().unwrap(),
            branch: repo.main_branch_id.parse().unwrap(),
            node: FileNodeId::Version(version.version_id.parse().unwrap()),
            version: version.version_id.parse().unwrap(), selection_generation: Generation::new(0),
        },
        content_hash: ContentHash::from_digest(blake3::hash(b"working draft")), size: 13,
    };
    let file = version.file_id.parse().unwrap();
    (temp, engine, file, revision)
}
fn limits() -> DraftLimits {
    DraftLimits {
        max_file_bytes: 1024, checksum_chunk_bytes: 65_536, max_entries: 8,
        max_retained_bytes: 8192,
    }
}
fn source(temp: &tempfile::TempDir, bytes: &[u8]) -> File {
    let path = temp.path().join("source");
    fs::write(&path, bytes).unwrap();
    File::open(path).unwrap()
}
fn disk(engine: &Engine) -> std::sync::Arc<DiskHandle> {
    engine.disk_resources.get(engine.disk("P01").unwrap()).unwrap().clone()
}
fn body(value: PreparedDraft) -> Vec<u8> {
    let mut file = value.into_body();
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    bytes
}
fn image(path: &std::path::Path) -> BTreeMap<PathBuf, (u64, u64, u32, Vec<u8>)> {
    fn walk(root: &std::path::Path, path: &std::path::Path,
        values: &mut BTreeMap<PathBuf, (u64, u64, u32, Vec<u8>)>)
    {
        let metadata = fs::symlink_metadata(path).unwrap();
        let bytes = if metadata.is_file() { fs::read(path).unwrap() } else { Vec::new() };
        values.insert(path.strip_prefix(root).unwrap().to_owned(),
            (metadata.dev(), metadata.ino(), metadata.mode(), bytes));
        if metadata.is_dir() {
            for child in fs::read_dir(path).unwrap() {
                walk(root, &child.unwrap().path(), values);
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(path, path, &mut result);
    result
}

#[test]
fn draft_body_reopens_after_restart_without_creating_history_or_selecting_itself() {
    let (temp, engine, file, revision) = fixture();
    let before = storage_contract::canonical_json(engine.state()).unwrap();
    let held = disk(&engine);
    let history = held.root.path().join("histories");
    let original = image(&history);
    let file_parent = directory(&held, file, &revision).parent().unwrap().to_owned();
    assert_eq!(fs::symlink_metadata(&file_parent).unwrap_err().kind(),
        std::io::ErrorKind::NotFound);
    let prepared = prepare(&held, file, revision.expected_generation, &revision,
        source(&temp, b"working draft"), limits()).unwrap();
    assert!(fs::symlink_metadata(&file_parent).unwrap().is_dir());
    assert_eq!(prepared.file(), file);
    assert_eq!(prepared.expected(), revision.expected_generation);
    assert_eq!(prepared.revision(), &revision);
    assert_eq!(body(prepared), b"working draft");
    assert_eq!(storage_contract::canonical_json(engine.state()).unwrap(), before);
    assert_eq!(image(&history), original);
    drop(held);
    drop(engine);
    let engine = Engine::open(&temp.path().join("state")).unwrap();
    assert_eq!(storage_contract::canonical_json(engine.state()).unwrap(), before);
    let held = disk(&engine);
    let prepared = reopen(&held, file, revision.expected_generation, &revision, limits()).unwrap();
    assert_eq!(body(prepared), b"working draft");
    assert_eq!(image(&history), original);
}

#[test]
fn exact_intent_retry_preserves_previous_body_and_rejects_changed_operation_proof() {
    let (temp, engine, file, revision) = fixture();
    let held = disk(&engine);
    assert!(matches!(prepare(&held, file, revision.expected_generation, &revision,
        source(&temp, b"wrong"), limits()), Err(StoreError::Conflict)));
    let path = directory(&held, file, &revision);
    let original_intent = fs::read(path.join(layout::INTENT.as_path())).unwrap();
    assert!(!path.join(layout::BODY.as_path()).exists());
    let saved = prepare(&held, file, revision.expected_generation, &revision,
        source(&temp, b"working draft"), limits()).unwrap();
    assert_eq!(body(saved), b"working draft");
    assert_eq!(fs::read(path.join(layout::INTENT.as_path())).unwrap(), original_intent);
    let mut changed = revision.clone();
    changed.principal = "different".parse().unwrap();
    let input = source(&temp, b"working draft");
    let before = image(&path);
    assert!(matches!(prepare(&held, file, changed.expected_generation, &changed,
        input, limits()), Err(StoreError::Conflict)));
    assert_eq!(image(&path), before);
    let replay = prepare(&held, file, revision.expected_generation, &revision,
        source(&temp, b"working draft"), limits()).unwrap();
    assert_eq!(body(replay), b"working draft");
    assert_eq!(image(&path), before);
}

#[test]
fn unknown_interrupted_revision_cannot_be_adopted_or_hide_the_selected_body() {
    let (temp, engine, file, original) = fixture();
    let held = disk(&engine);
    drop(prepare(&held, file, original.expected_generation, &original,
        source(&temp, b"working draft"), limits()).unwrap());
    let mut next = original.clone();
    next.operation = OperationId::generate().unwrap();
    let path = directory(&held, file, &next);
    held.root.create_private_directory_synced(&path).unwrap();
    held.root.write_atomic(&path.join(layout::SCRATCH.as_path()), b"interrupted", false).unwrap();
    let before = image(held.root.path());
    assert!(matches!(prepare(&held, file, next.expected_generation, &next,
        source(&temp, b"working draft"), limits()), Err(StoreError::Incomplete)));
    assert_eq!(image(held.root.path()), before);
    assert_eq!(body(reopen(&held, file, original.expected_generation, &original,
        limits()).unwrap()), b"working draft");
    assert_eq!(image(held.root.path()), before);
}

#[test]
fn corrupt_body_and_exhausted_budget_never_fall_back_to_history_or_overwrite_evidence() {
    let (temp, engine, file, original) = fixture();
    let held = disk(&engine);
    drop(prepare(&held, file, original.expected_generation, &original,
        source(&temp, b"working draft"), limits()).unwrap());
    let mut next = original.clone();
    next.operation = OperationId::generate().unwrap();
    let mut bounded = limits();
    bounded.max_entries = 1;
    let before = image(held.root.path());
    let other_file = FileId::generate().unwrap();
    // A different FileId cannot obtain a fresh independent per-file allowance.
    for target in [file, other_file] {
        assert!(matches!(prepare(&held, target, next.expected_generation, &next,
            source(&temp, b"working draft"), bounded), Err(StoreError::Budget)));
        assert_eq!(image(held.root.path()), before);
    }
    let retained = directory(&held, file, &original);
    bounded.max_entries = limits().max_entries;
    bounded.max_retained_bytes = fs::metadata(retained.join(layout::INTENT.as_path()))
        .unwrap().len() + original.size;
    assert!(matches!(prepare(&held, other_file, next.expected_generation, &next,
        source(&temp, b"working draft"), bounded), Err(StoreError::Budget)));
    assert_eq!(image(held.root.path()), before);
    // Current reads are independent of allocation capacity and orphan discovery.
    assert_eq!(body(reopen(&held, file, original.expected_generation, &original,
        bounded).unwrap()), b"working draft");
    let path = directory(&held, file, &original).join(layout::BODY.as_path());
    fs::write(&path, b"damaged draft").unwrap();
    let before = image(held.root.path());
    assert!(matches!(reopen(&held, file, original.expected_generation, &original,
        limits()), Err(StoreError::Payload(_))));
    assert_eq!(image(held.root.path()), before);
}

#[test]
fn range_manifest_is_bound_to_selected_bytes_and_detects_later_chunk_tampering() {
    let (temp, engine, file, mut revision) = fixture();
    let held = disk(&engine);
    let mut bytes = vec![b'a'; 65_536 + 19];
    bytes[65_536..].fill(b'b');
    revision.size = bytes.len() as u64;
    revision.content_hash = ContentHash::from_digest(blake3::hash(&bytes));
    let mut bounds = limits();
    bounds.max_file_bytes = 128 * 1024;
    bounds.max_retained_bytes = 256 * 1024;
    let prepared = prepare(&held, file, revision.expected_generation, &revision,
        source(&temp, &bytes), bounds).unwrap();
    let (body, checksums) = prepared.into_read_parts();
    assert_eq!(checksums.logical_size, revision.size);
    assert_eq!(checksums.chunk_bytes, 65_536);
    assert_eq!(checksums.chunks.len(), 2);
    assert_eq!(checksums.chunks[0], *blake3::hash(&bytes[..65_536]).as_bytes());
    assert_eq!(checksums.chunks[1], *blake3::hash(&bytes[65_536..]).as_bytes());
    // The wire manifest remains a real checksum manifest, without VersionId.
    let decoded = ChecksumManifest::decode(&checksums.encode().unwrap()).unwrap();
    let mut last_chunk = [0; 19];
    body.read_exact_at(&mut last_chunk, 65_536).unwrap();
    assert_eq!(*blake3::hash(&last_chunk).as_bytes(), decoded.chunks[1]);
    let path = directory(&held, file, &revision).join(layout::BODY.as_path());
    let writer = fs::OpenOptions::new().write(true).open(&path).unwrap();
    writer.write_all_at(b"x", 65_540).unwrap();
    writer.sync_all().unwrap();
    body.read_exact_at(&mut last_chunk, 65_536).unwrap();
    assert_ne!(*blake3::hash(&last_chunk).as_bytes(), decoded.chunks[1]);
    let before = image(held.root.path());
    assert!(matches!(reopen(&held, file, revision.expected_generation, &revision,
        bounds), Err(StoreError::Payload(_))));
    assert_eq!(image(held.root.path()), before);
}
