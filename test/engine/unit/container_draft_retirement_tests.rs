//! Exact permanent successors, live readers and real cold admission.
use super::*;
use std::path::{Path, PathBuf};

struct Fixture {
    root: tempfile::TempDir,
    engine: Engine,
    file: FileId,
    base: storage_contract::container_edit::DraftBase,
    principal: Principal,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let system = root.path().join("system");
        let disk = root.path().join("disk");
        for path in [&system, &disk] {
            fs::create_dir(path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut config = Config::development([disk]).unwrap();
        config.max_file_bytes = 128 * 1024;
        config.max_workspace_bytes = 256 * 1024;
        let mut engine = Engine::initialize_manual_container_storage(
            &system, config, OperationId::generate().unwrap(),
        ).unwrap();
        let principal = Principal::new("alice".into()).unwrap();
        let repo = engine.create_repository(
            &storage_contract::RepositoryName::parse("retirement").unwrap(), "P01", 1,
            principal.as_str(), OperationId::generate().unwrap(),
        ).unwrap();
        let workspace = engine.create_workspace(&repo.main_branch_id, principal.as_str()).unwrap();
        engine.stage_bytes(&workspace.workspace_id, "file.txt", b"history").unwrap();
        engine.commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(), principal: principal.as_str().into(),
            expected_head: workspace.base_head, fencing_token: workspace.fencing_token,
            message: "history".into(), request_digest: None,
        }).unwrap();
        let entry = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
        let file = entry.file_id.parse().unwrap();
        engine.set_container_commit_mode(&SetCommitMode {
            file, expected_generation: Generation::new(0), mode: CommitMode::Manual,
        }, &principal, OperationId::generate().unwrap()).unwrap();
        let base = engine.current_container_draft_base(
            file, repo.repository_id.parse().unwrap(), entry.entry_id.parse().unwrap(),
        ).unwrap();
        Self { root, engine, file, base, principal }
    }
    fn revision(&self, byte: u8) -> DraftRevision {
        DraftRevision {
            operation: OperationId::generate().unwrap(), principal: self.principal.clone(),
            expected_generation: self.engine.container_edit(self.file).unwrap().generation(),
            base: self.base.clone(), size: 64 * 1024,
            content_hash: ContentHash::from_digest(blake3::hash(&vec![byte; 64 * 1024])),
        }
    }
    fn save(
        &mut self, revision: &DraftRevision, byte: u8,
    ) -> crate::Result<crate::OperationResult> {
        let path = self.root.path().join("upload");
        fs::write(&path, vec![byte; 64 * 1024]).unwrap();
        self.engine.save_container_draft(self.file, revision, fs::File::open(path).unwrap())
    }
    fn body(&self, revision: &DraftRevision) -> PathBuf {
        self.root.path().join("disk/container-drafts-v1").join(self.file.to_string())
            .join(revision.operation.to_string()).join("body")
    }
}
fn read_current(engine: &Engine, file: FileId, byte: u8) {
    let mut draft = engine.open_container_draft(file).unwrap().unwrap();
    let mut bytes = Vec::new();
    draft.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, vec![byte; 64 * 1024]);
}
fn cold(root: &Path) -> Engine {
    Engine::open(&root.join("system")).unwrap()
}

#[test]
fn permanent_successor_keeps_cloned_reader_then_retries_retirement_before_allocation() {
    let mut fixture = Fixture::new();
    let first = fixture.revision(1);
    let receipt = fixture.save(&first, 1).unwrap();
    let held = fixture.engine.open_container_draft(fixture.file).unwrap().unwrap();
    let mut clone = held.file.try_clone().unwrap();
    let second = fixture.revision(2);
    fixture.save(&second, 2).unwrap();
    assert!(fixture.body(&first).is_file());
    drop(held);
    let third = fixture.revision(3);
    fixture.save(&third, 3).unwrap();
    assert!(fixture.body(&first).is_file());
    assert!(!fixture.body(&second).exists());
    let mut bytes = Vec::new();
    clone.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, vec![1; 64 * 1024]);
    drop(clone);
    // A new allocation retries cleanup before accounting its requested bytes.
    let fourth = fixture.revision(4);
    fixture.save(&fourth, 4).unwrap();
    assert!(!fixture.body(&first).exists());
    assert!(!fixture.body(&third).exists());
    assert!(fixture.body(&fourth).is_file());
    assert_eq!(fixture.engine.container_draft_save_receipt(
        fixture.file, &first, &fixture.principal,
    ).unwrap(), Some(receipt));
    let before = fixture.engine.state().clone();
    drop(fixture.engine);
    let engine = cold(fixture.root.path());
    assert_eq!(engine.state(), &before);
    read_current(&engine, fixture.file, 4);
}

#[test]
fn cold_replay_accepts_only_exact_superseded_intent_after_interrupted_body_unlink() {
    let mut fixture = Fixture::new();
    let first = fixture.revision(1);
    fixture.save(&first, 1).unwrap();
    let held = fixture.engine.open_container_draft(fixture.file).unwrap().unwrap();
    let second = fixture.revision(2);
    let receipt = fixture.save(&second, 2).unwrap();
    drop(held);
    // Simulate interruption after unlink but before the cleanup sync/retry.
    fs::remove_file(fixture.body(&first)).unwrap();
    let before = fixture.engine.state().clone();
    drop(fixture.engine);
    let mut engine = cold(fixture.root.path());
    assert_eq!(engine.state(), &before);
    read_current(&engine, fixture.file, 2);
    let source = fixture.root.path().join("upload");
    assert_eq!(engine.save_container_draft(
        fixture.file, &second, fs::File::open(&source).unwrap(),
    ).unwrap(), receipt);
    assert_eq!(engine.state(), &before);
    drop(engine);
    assert_eq!(cold(fixture.root.path()).state(), &before);
}

#[test]
fn missing_current_or_changed_retired_intent_never_gains_successor_authority() {
    for remove_current in [false, true] {
        let mut fixture = Fixture::new();
        let first = fixture.revision(1);
        fixture.save(&first, 1).unwrap();
        let second = fixture.revision(2);
        fixture.save(&second, 2).unwrap();
        let path = if remove_current {
            fixture.body(&second)
        } else {
            fixture.body(&first).parent().unwrap().join("intent.json")
        };
        if remove_current {
            fs::remove_file(&path).unwrap();
        } else {
            fs::write(&path, b"changed retained intent").unwrap();
        }
        drop(fixture.engine);
        assert!(Engine::open(&fixture.root.path().join("system")).is_err());
        if !remove_current {
            assert_eq!(fs::read(&path).unwrap(), b"changed retained intent");
        }
    }
}

#[test]
fn unpublished_preparation_is_not_retired_by_a_later_successful_save() {
    let mut fixture = Fixture::new();
    let first = fixture.revision(1);
    fixture.save(&first, 1).unwrap();
    let unknown = fixture.revision(9);
    let path = fixture.root.path().join("unknown-upload");
    fs::write(&path, vec![9u8; 64 * 1024]).unwrap();
    let disk = fixture.engine.draft_disk(&unknown).unwrap();
    drop(crate::engine::container_drafts::store::prepare(
        &disk, fixture.file, unknown.expected_generation, &unknown,
        fs::File::open(path).unwrap(), fixture.engine.draft_limits(),
    ).unwrap());
    let second = fixture.revision(2);
    fixture.save(&second, 2).unwrap();
    assert!(!fixture.body(&first).exists());
    assert_eq!(fs::read(fixture.body(&unknown)).unwrap(), vec![9; 64 * 1024]);
    assert!(fixture.engine.container_draft_save_receipt(
        fixture.file, &unknown, &fixture.principal,
    ).unwrap().is_none());
    drop(disk);
    drop(fixture.engine);
    let engine = cold(fixture.root.path());
    read_current(&engine, fixture.file, 2);
}

#[test]
fn changed_superseded_body_is_preserved_without_reinterpreting_permanent_success() {
    let mut fixture = Fixture::new();
    let first = fixture.revision(1);
    fixture.save(&first, 1).unwrap();
    let held = fixture.engine.open_container_draft(fixture.file).unwrap().unwrap();
    let second = fixture.revision(2);
    let receipt = fixture.save(&second, 2).unwrap();
    drop(held);
    fs::write(fixture.body(&first), b"corrupt retained body").unwrap();
    let before = fixture.engine.state().clone();
    assert!(fixture.save(&second, 2).is_err());
    assert_eq!(fixture.engine.state(), &before);
    assert_eq!(fixture.engine.container_draft_save_receipt(
        fixture.file, &second, &fixture.principal,
    ).unwrap(), Some(receipt));
    assert_eq!(fs::read(fixture.body(&first)).unwrap(), b"corrupt retained body");
    read_current(&fixture.engine, fixture.file, 2);
    drop(fixture.engine);
    assert!(Engine::open(&fixture.root.path().join("system")).is_err());
}

#[test]
fn journal_rollback_cannot_borrow_the_removed_successor_to_excuse_missing_body() {
    let mut fixture = Fixture::new();
    let first = fixture.revision(1);
    fixture.save(&first, 1).unwrap();
    let log = crate::journal::log_path(fixture.engine.disk("P01").unwrap());
    let original_prefix = fs::read(&log).unwrap();
    let second = fixture.revision(2);
    fixture.save(&second, 2).unwrap();
    assert!(!fixture.body(&first).exists());
    drop(fixture.engine);
    // Keep the real newer publication barrier: an old journal cannot claim
    // an earlier tip or use a later State without its actual published prefix.
    fs::write(&log, &original_prefix).unwrap();
    assert!(Engine::open(&fixture.root.path().join("system")).is_err());
    assert_eq!(fs::read(&log).unwrap(), original_prefix);
}

#[test]
fn retained_explicit_commit_workspace_keeps_its_exact_draft_body_without_live_reader() {
    let mut fixture = Fixture::new();
    let first = fixture.revision(1);
    fixture.save(&first, 1).unwrap();
    let prepared = fixture.engine.prepare_shared_container_draft_commit(
        fixture.file,
        &storage_contract::file_container_tags::Context {
            repository_id: fixture.base.repository,
            path: name_policy::PortablePath::parse("file.txt").unwrap(),
        },
        fixture.engine.container_edit(fixture.file).unwrap().generation(),
        first.operation, &fixture.principal, OperationId::generate().unwrap(),
    ).unwrap();
    let second = fixture.revision(2);
    fixture.save(&second, 2).unwrap();
    let workspace = fixture.engine.workspace(&prepared.workspace_id).unwrap();
    assert_eq!(workspace.container_draft_commit.unwrap().revision, first);
    assert_eq!(fs::read(fixture.body(&first)).unwrap(), vec![1; 64 * 1024]);
    read_current(&fixture.engine, fixture.file, 2);
    fixture.engine.abort_workspace(&prepared.workspace_id, fixture.principal.as_str()).unwrap();
    fixture.save(&second, 2).unwrap();
    assert!(!fixture.body(&first).exists());
    read_current(&fixture.engine, fixture.file, 2);
}
