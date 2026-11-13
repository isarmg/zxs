//! Recovery and HTTP-adaptation boundaries of the real public layout API.
//! These tests use temporary roots only. No endpoint/operation protocol is
//! invented here, and no filesystem snapshot is treated as a content receipt.
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};
use storage_engine::{
    CommitRequest, CommitState, Config, DirectoryLayoutReceipt, Engine, Entry, ErrorCode,
    NamespaceOperation, NamespaceOperationKind, OperationId, OperationResult, Repository, Result,
    directory_git::DirectoryGitInitializeBody,
};

fn operation() -> OperationId {
    OperationId::generate().unwrap()
}
fn identity(path: &Path) -> (u64, u64) {
    let metadata = fs::symlink_metadata(path).unwrap();
    assert!(metadata.is_dir());
    (metadata.dev(), metadata.ino())
}
#[derive(Debug, PartialEq, Eq)]
enum ObservedEntry {
    Directory((u64, u64), u32),
    File((u64, u64), u32, Vec<u8>),
}
/// Include empty directories and exact record bytes/inodes. Publication
/// evidence, journals and the disposable SQLite index are outside this view.
fn layout_snapshot(history: &Path) -> BTreeMap<PathBuf, ObservedEntry> {
    fn visit(base: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, ObservedEntry>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let key = path.strip_prefix(base).unwrap().to_owned();
        let inode = (metadata.dev(), metadata.ino());
        let mode = metadata.mode();
        if metadata.is_dir() {
            entries.insert(key, ObservedEntry::Directory(inode, mode));
            for child in fs::read_dir(path).unwrap() {
                visit(base, &child.unwrap().path(), entries);
            }
        } else {
            assert!(metadata.is_file() && !metadata.file_type().is_symlink());
            assert_eq!(metadata.nlink(), 1);
            entries.insert(
                key,
                ObservedEntry::File(inode, mode, fs::read(path).unwrap()),
            );
        }
    }
    let mut entries = BTreeMap::new();
    for name in [".zxs-layout", "directory-repositories", "file-containers"] {
        let path = history.join(name);
        if path.exists() {
            visit(history, &path, &mut entries);
        }
    }
    entries
}

struct Fixture {
    temporary: tempfile::TempDir,
    config: Config,
    repository: Repository,
    engine: Option<Engine>,
}
impl Fixture {
    fn new(roots: usize, primary_is_metadata: bool) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let mut config = Config::development(
            (0..roots).map(|index| temporary.path().join(format!("disk-{index}"))),
        )
        .unwrap();
        config.disks[0].metadata_member = primary_is_metadata;
        let mut engine =
            Engine::initialize(&temporary.path().join("system"), config.clone()).unwrap();
        let repository = if roots > 1 {
            // Disk roots intentionally start with one copy. This offline-body
            // case explicitly requests a healthy second copy before saving.
            engine
                .create_repository(
                    &storage_contract::RepositoryName::parse("layout boundary replicas").unwrap(),
                    "P01",
                    2,
                    "admin",
                    operation(),
                )
                .unwrap()
        } else {
            engine
                .create_disk_root("P01", "admin", operation())
                .unwrap()
        };
        Self {
            temporary,
            config,
            repository,
            engine: Some(engine),
        }
    }
    fn engine(&self) -> &Engine {
        self.engine.as_ref().unwrap()
    }
    fn engine_mut(&mut self) -> &mut Engine {
        self.engine.as_mut().unwrap()
    }
    fn system(&self) -> PathBuf {
        self.temporary.path().join("system")
    }
    fn history(&self) -> PathBuf {
        self.config.disks[0]
            .root
            .join("histories")
            .join(&self.repository.history_tree_id)
    }
    fn root(&self) -> PathBuf {
        self.history()
            .join("directory-repositories")
            .join(&self.repository.repository_id)
    }
    fn close(&mut self) {
        drop(self.engine.take().unwrap());
    }
    fn reopen(&mut self, preserve_pending: bool) {
        assert!(self.engine.is_none());
        self.engine = Some(if preserve_pending {
            Engine::open_for_recovery(&self.system()).unwrap()
        } else {
            Engine::open(&self.system()).unwrap()
        });
    }
    fn save(&mut self, path: &str, body: &[u8]) -> Entry {
        let branch = self.repository.main_branch_id.clone();
        let workspace = self
            .engine_mut()
            .create_workspace(&branch, "admin")
            .unwrap();
        self.engine_mut()
            .stage_bytes(&workspace.workspace_id, path, body)
            .unwrap();
        self.engine_mut()
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: operation(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "layout recovery boundary save".into(),
                request_digest: None,
            })
            .unwrap();
        self.engine()
            .namespace_tree(&branch)
            .unwrap()
            .get(path)
            .unwrap()
            .clone()
    }
    fn initialize(&mut self) {
        let repository = self.repository.repository_id.clone();
        let head = self.engine().state().branches[&self.repository.main_branch_id]
            .head_commit_id
            .clone();
        self.engine_mut()
            .initialize_directory_git(
                &repository,
                &DirectoryGitInitializeBody {
                    path: String::new(),
                    expected_namespace_head: head,
                    operation_id: operation(),
                },
                "admin",
            )
            .unwrap();
    }
    fn reconcile(&mut self) -> Result<DirectoryLayoutReceipt> {
        let repository = self.repository.repository_id.clone();
        self.engine_mut().reconcile_directory_layout(&repository)
    }
    fn rename(&mut self, from: &str, to: &str, id: OperationId) -> Result<OperationResult> {
        let repository = self.repository.repository_id.clone();
        self.engine_mut().file_namespace(
            &repository,
            NamespaceOperation::parse(NamespaceOperationKind::Rename, from, Some(to)).unwrap(),
            "admin",
            id,
        )
    }
    fn assert_body(&self, container: &Path, entry: &Entry, body: &[u8]) {
        let version = &self.engine().state().versions[&entry.version_id];
        let owner: String =
            json_codec::from_slice(&fs::read(container.join(".zxs-container-owner.json")).unwrap())
                .unwrap();
        assert_eq!(owner, entry.file_id);
        assert_eq!(
            fs::read(container.join("versions").join(format!("{}-{}", version.storage_id,
                container.file_name().unwrap().to_str().unwrap()))).unwrap(),
            body
        );
        let mut read = self.engine().open_content(&entry.version_id).unwrap();
        let mut actual = Vec::new();
        read.file.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, body);
    }
}

#[test]
fn preserving_a_finalized_pending_rename_keeps_layout_unchanged_until_complete_recovery() {
    let mut fixture = Fixture::new(1, true);
    let entry = fixture.save("Before/file.txt", b"published working body");
    fixture.initialize();
    fixture.reconcile().unwrap();
    let original_directory = identity(&fixture.root().join("Before"));
    let original_container = identity(&fixture.root().join("Before/file.txt"));
    let sequence = fixture.engine().state().sequence;
    let id = operation();
    fixture.engine_mut().inject_fault_once("after_finalize");
    assert_eq!(
        fixture.rename("Before", "After", id).unwrap_err().code(),
        ErrorCode::InDoubt
    );
    assert_eq!(fixture.engine().state().sequence, sequence);
    assert!(fixture.root().join("Before").is_dir());
    assert!(!fixture.root().join("After").exists());
    fixture.close();
    let before = layout_snapshot(&fixture.history());
    fixture.reopen(true);
    assert!(fixture.engine().is_read_only());
    assert_eq!(fixture.engine().state().sequence, sequence);
    assert_eq!(
        fixture.reconcile().unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(layout_snapshot(&fixture.history()), before);
    assert_eq!(identity(&fixture.root().join("Before")), original_directory);
    fixture.assert_body(
        &fixture.root().join("Before/file.txt"),
        &entry,
        b"published working body",
    );
    fixture.close();
    fixture.reopen(false);
    assert!(!fixture.engine().is_read_only());
    assert_eq!(identity(&fixture.root().join("After")), original_directory);
    assert_eq!(
        identity(&fixture.root().join("After/file.txt")),
        original_container
    );
    assert!(!fixture.root().join("Before").exists());
    let receipt = fixture.engine().operation(id).unwrap();
    assert_eq!(receipt.commit_state, CommitState::Published);
    assert!(receipt.sequence > sequence);
    fixture.assert_body(
        &fixture.root().join("After/file.txt"),
        &entry,
        b"published working body",
    );
    fixture.close();
    fixture.reopen(false);
    assert_eq!(
        identity(&fixture.root().join("After/file.txt")),
        original_container
    );
    assert_eq!(
        fixture.engine().operation(id).unwrap().sequence,
        receipt.sequence
    );
}

#[test]
fn missing_file_locator_refuses_reopen_without_recreating_or_relocating_the_owned_container() {
    let mut fixture = Fixture::new(1, true);
    let entry = fixture.save("Docs/file.txt", b"do not duplicate this body");
    fixture.initialize();
    fixture.reconcile().unwrap();
    let container = fixture.root().join("Docs/file.txt");
    let original = identity(&container);
    let record = fixture
        .history()
        .join(".zxs-layout/files")
        .join(format!("{}.json", entry.file_id));
    let bytes = fs::read(&record).unwrap();
    let mode = fs::metadata(&record).unwrap().mode() & 0o777;
    fixture.close();
    // Simulate lost derived metadata only inside this test's temporary root.
    fs::remove_file(&record).unwrap();
    let before = layout_snapshot(&fixture.history());
    let error = Engine::open(&fixture.system())
        .err()
        .expect("unindexed shared owner must be rejected");
    assert_eq!(error.code(), ErrorCode::ContainerLayoutInvalid);
    assert_eq!(layout_snapshot(&fixture.history()), before);
    assert_eq!(identity(&container), original);
    assert!(!record.exists());
    // Exact same-root record restoration is not foreign inode rebinding.
    fs::write(&record, &bytes).unwrap();
    fs::set_permissions(&record, fs::Permissions::from_mode(mode)).unwrap();
    fixture.reopen(false);
    assert_eq!(identity(&container), original);
    fixture.assert_body(&container, &entry, b"do not duplicate this body");
}

#[test]
fn rejected_active_layout_preflight_does_not_publish_an_operation_or_fence_future_repairs() {
    let mut fixture = Fixture::new(1, true);
    let entry = fixture.save("file.txt", b"same entity across repairs");
    fixture.initialize();
    let state = storage_engine::canonical_json(fixture.engine().state()).unwrap();
    let initial_sequence = fixture.engine().state().sequence;
    let first = fixture.reconcile().unwrap();
    assert_eq!(first.state_sequence, initial_sequence);
    assert_eq!(
        storage_engine::canonical_json(fixture.engine().state()).unwrap(),
        state
    );
    let original = identity(&fixture.root().join("file.txt"));
    let unknown = fixture.root().join("unknown-empty");
    fs::create_dir(&unknown).unwrap();
    fs::set_permissions(&unknown, fs::Permissions::from_mode(0o700)).unwrap();
    let unknown_inode = identity(&unknown);
    let before = layout_snapshot(&fixture.history());
    let error = fixture.reconcile().unwrap_err();
    assert_eq!(error.code(), ErrorCode::ContainerLayoutInvalid);
    assert!(!error.is_retryable());
    assert!(!fixture.engine().is_read_only());
    assert_eq!(
        storage_engine::canonical_json(fixture.engine().state()).unwrap(),
        state
    );
    assert_eq!(layout_snapshot(&fixture.history()), before);
    assert_eq!(identity(&unknown), unknown_inode);
    fs::remove_dir(&unknown).unwrap();
    let repeated = fixture.reconcile().unwrap();
    assert_eq!(repeated.state_sequence, initial_sequence);
    assert_eq!(repeated.containers, 1);
    assert!(repeated.unavailable_containers.is_empty());
    assert!(repeated.unavailable_histories.is_empty());
    assert_eq!(identity(&fixture.root().join("file.txt")), original);
    assert_eq!(
        storage_engine::canonical_json(fixture.engine().state()).unwrap(),
        state
    );
    fixture.assert_body(
        &fixture.root().join("file.txt"),
        &entry,
        b"same entity across repairs",
    );
}

#[test]
fn offline_payload_only_primary_returns_explicit_partial_receipt_until_reattached() {
    let mut fixture = Fixture::new(3, false);
    let entry = fixture.save("file.txt", b"available on healthy backup");
    fixture.initialize();
    assert_eq!(
        fixture
            .engine()
            .config()
            .disks
            .iter()
            .filter(|disk| disk.metadata_member)
            .count(),
        2
    );
    fixture.close();
    let primary = fixture.config.disks[0].root.clone();
    let offline = fixture.temporary.path().join("temporarily-offline-primary");
    fs::rename(&primary, &offline).unwrap();
    fixture.reopen(false);
    assert!(!fixture.engine().is_read_only());
    let before = storage_engine::canonical_json(fixture.engine().state()).unwrap();
    let partial = fixture.reconcile().unwrap();
    assert_eq!(partial.state_sequence, fixture.engine().state().sequence);
    assert_eq!(partial.directories, 0);
    assert_eq!(partial.containers, 0);
    assert_eq!(
        partial.unavailable_histories,
        vec![("P01".into(), fixture.repository.history_tree_id.clone())]
    );
    assert_eq!(
        partial.unavailable_containers,
        vec![(
            "P01".into(),
            fixture.repository.history_tree_id.clone(),
            entry.file_id.clone()
        )]
    );
    assert_eq!(
        storage_engine::canonical_json(fixture.engine().state()).unwrap(),
        before
    );
    assert!(!primary.exists());
    let mut read = fixture.engine().open_content(&entry.version_id).unwrap();
    let mut body = Vec::new();
    read.file.read_to_end(&mut body).unwrap();
    assert_eq!(body, b"available on healthy backup");
    drop(read);
    fixture.close();
    fs::rename(&offline, &primary).unwrap();
    fixture.reopen(false);
    let completed = fixture.reconcile().unwrap();
    assert_eq!(completed.containers, 1);
    assert!(completed.unavailable_histories.is_empty());
    assert!(completed.unavailable_containers.is_empty());
    fixture.assert_body(
        &fixture.root().join("file.txt"),
        &entry,
        b"available on healthy backup",
    );
}
