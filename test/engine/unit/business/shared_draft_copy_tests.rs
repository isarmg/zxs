//! Real ordinary-copy admission. These tests do not synthesize working rows or
//! infer copy authority from a name; all files and drafts are native writes.
use super::*;
use name_policy::{Name, PortablePath};
use std::{fs, io::{Read, Write}, os::unix::fs::{MetadataExt, PermissionsExt}};
use storage_contract::file_draft::{BirthRequest, CommitMode, DraftCommand, DraftLocation};
use storage_contract::{ContentHash, Mode, Principal};

struct Fixture {
    temporary: tempfile::TempDir,
    engine: Engine,
    repository: Repository,
    scope: String,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let system = temporary.path().join("system");
        let disk = temporary.path().join("primary");
        for path in [&system, &disk] {
            fs::create_dir(path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let config = Config::independent_roots(
            Mode::Development, [(Name::parse("Primary").unwrap(), disk)],
        ).unwrap();
        let slot = config.disks[0].slot_id.clone();
        let mut engine = Engine::initialize(&system, config).unwrap();
        let repository = engine.create_disk_root(&slot, "admin", operation()).unwrap();
        let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
        for path in ["first.txt", "second.txt", "folder/published.txt", "other/published.txt"] {
            engine.stage_bytes(&workspace.workspace_id, path, b"original committed bytes").unwrap();
        }
        engine.commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id, operation_id: operation(),
            principal: "admin".into(), expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token, message: "real committed sources".into(),
            request_digest: None,
        }).unwrap();
        let scope = crate::directory_git_fixture::checkpoint(&mut engine, &repository, "")
            .directory_branch_id.unwrap();
        Self { temporary, engine, repository, scope }
    }

    fn target(&self, path: &str) -> DraftLocation {
        let entry = self.engine.list_tree(&self.repository.main_branch_id).unwrap()
            .into_iter().find(|entry| entry.path() == path).unwrap();
        DraftLocation {
            repository: self.repository.repository_id.parse().unwrap(),
            path: PortablePath::parse(path).unwrap(),
            entry: entry.entry_id.parse().unwrap(), file: entry.file_id.parse().unwrap(),
        }
    }

    fn dirty(&mut self, path: &str, bytes: &[u8]) -> DraftLocation {
        let target = self.target(path);
        let principal: Principal = "admin".parse().unwrap();
        self.engine.change_file_working(&DraftCommand::SetMode {
            target: target.clone(), expected: self.engine.file_working_state(&target).unwrap().revision(),
            mode: CommitMode::Manual.into(),
        }, &principal, operation()).unwrap();
        let write = DraftCommand::Write {
            controller: storage_contract::file_draft::SaveController::Manual {},
            target: target.clone(), expected: self.engine.file_working_state(&target).unwrap().revision(),
            base: self.engine.file_draft_base(&target).unwrap(),
            content_hash: ContentHash::from_digest(blake3::hash(bytes)), logical_size: bytes.len() as u64,
        };
        let mut input = tempfile::tempfile().unwrap();
        input.write_all(bytes).unwrap();
        input.sync_all().unwrap();
        self.engine.write_file_draft(&write, &principal, operation(), input.into()).unwrap();
        target
    }

    fn request(&self, path: &str, destination: &str) -> DirectoryNamespaceBatch {
        let source = self.engine.current_directory_entries(&self.scope).unwrap().iter()
            .find(|entry| entry.path() == path).unwrap();
        let tree = self.engine.state().directory_branches[&self.scope].head_tree_id.clone();
        DirectoryNamespaceBatch {
            kind: DirectoryBatchKind::Copy,
            items: vec![DirectoryNamespaceItem {
                source: DirectoryNamespaceSource {
                    directory_branch_id: self.scope.clone(), expected_tree_id: tree.clone(),
                    path: path.into(), entry_id: source.entry_id.clone(),
                },
                target: Some(DirectoryNamespaceTarget {
                    directory_branch_id: self.scope.clone(), expected_tree_id: tree,
                    path: destination.into(),
                }),
            }],
        }
    }

    fn copy(&mut self, request: &DirectoryNamespaceBatch, id: OperationId) -> Result<OperationResult> {
        self.engine.directory_namespace_batch(&self.repository.repository_id, request, "admin", id)
    }

    fn reopen(self) -> Self {
        let Self { temporary, engine, repository, scope } = self;
        drop(engine);
        let engine = Engine::open(&temporary.path().join("system")).unwrap();
        Self { temporary, engine, repository, scope }
    }
}

fn operation() -> OperationId { OperationId::generate().unwrap() }

fn materials(root: &std::path::Path) -> BTreeMap<std::path::PathBuf, (u64, u64, Vec<u8>)> {
    fn visit(root: &std::path::Path, path: &std::path::Path,
        found: &mut BTreeMap<std::path::PathBuf, (u64, u64, Vec<u8>)>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() { visit(root, &path, found); }
            else {
                assert!(metadata.is_file());
                found.insert(path.strip_prefix(root).unwrap().into(),
                    (metadata.dev(), metadata.ino(), fs::read(path).unwrap()));
            }
        }
    }
    let mut found = BTreeMap::new();
    visit(root, root, &mut found);
    found
}

#[test]
fn ordinary_copy_rejects_selected_draft_before_material_and_replays_original_receipt() {
    let mut fixture = Fixture::new();
    let original = fixture.request("first.txt", "earlier.txt");
    let id = operation();
    let receipt = fixture.copy(&original, id).unwrap();
    let target = fixture.dirty("first.txt", b"the newer shared draft");
    for cold in [false, true] {
        if cold { fixture = fixture.reopen(); }
        let rejected = fixture.request("first.txt", "must-not-appear.txt");
        let before = fixture.engine.state().clone();
        let physical = materials(&fixture.temporary.path().join("primary"));
        let rejected_id = operation();
        assert_eq!(fixture.copy(&rejected, rejected_id).unwrap_err().code(),
            ErrorCode::FileBranchConflict);
        assert_eq!(fixture.engine.state(), &before);
        assert_eq!(materials(&fixture.temporary.path().join("primary")), physical);
        assert!(!fixture.engine.state().operations.contains_key(&rejected_id));
        assert_eq!(fixture.copy(&original, id).unwrap(), receipt);
        assert_eq!(fixture.engine.state(), &before);
        let mut bytes = Vec::new();
        fixture.engine.open_file_draft(&target).unwrap().read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"the newer shared draft");
        let unrelated = fixture.request("second.txt", if cold { "clean-cold.txt" } else { "clean-hot.txt" });
        fixture.copy(&unrelated, operation()).unwrap();
        assert_eq!(fixture.engine.state().file_containers[&target.file], before.file_containers[&target.file]);
    }
}

#[test]
fn current_directory_copy_rejects_only_selected_unborn_parent_identities() {
    let mut fixture = Fixture::new();
    let birth_id = operation();
    fixture.engine.create_unborn_file(&BirthRequest {
        repository: fixture.repository.repository_id.parse().unwrap(),
        path: PortablePath::parse("folder/unborn.txt").unwrap(),
        expected_namespace_head: fixture.engine.state().branches[&fixture.repository.main_branch_id]
            .head_commit_id.parse().unwrap(),
    }, &"admin".parse().unwrap(), birth_id).unwrap();
    let (file, born) = fixture.engine.state().file_containers.iter()
        .find(|(_, record)| record.created_by == birth_id).map(|(file, record)| (*file, record.clone())).unwrap();
    assert!(born.working.unborn().is_some());
    for cold in [false, true] {
        if cold { fixture = fixture.reopen(); }
        let selected = fixture.request("folder", "must-not-copy-folder");
        let before = fixture.engine.state().clone();
        let physical = materials(&fixture.temporary.path().join("primary"));
        assert_eq!(fixture.copy(&selected, operation()).unwrap_err().code(), ErrorCode::FileBranchConflict);
        assert_eq!(fixture.engine.state(), &before);
        assert_eq!(materials(&fixture.temporary.path().join("primary")), physical);
        let unrelated = fixture.request("other", if cold { "other-cold" } else { "other-hot" });
        fixture.copy(&unrelated, operation()).unwrap();
        assert_eq!(fixture.engine.state().file_containers[&file], born);
    }
}
