//! Public Engine regressions on real temporary directories, not model previews.
//! Receipt assertions describe structural placement only; explicit body reads
//! below are separate test evidence. This does not exercise physical power loss.
#[path = "../support/directory_git_fixture.rs"]
mod directory_git_fixture;

use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
};
use storage_engine::{
    BranchName, CommitRequest, Config, DirectoryCommitRequest, DirectoryDefaultSelection, Engine,
    Entry, ErrorCode, NamespaceOperation, NamespaceOperationKind, OperationId, Repository,
    directory_git::{DirectoryGitInitializeBody, DirectoryGitStatus},
};

fn op() -> OperationId {
    OperationId::generate().unwrap()
}
fn identity(path: &Path) -> (u64, u64) {
    let metadata = fs::symlink_metadata(path).unwrap();
    assert!(metadata.is_dir());
    (metadata.dev(), metadata.ino())
}
fn file_snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}
fn container_paths(root: &Path) -> BTreeMap<String, Vec<PathBuf>> {
    fn visit(directory: &Path, result: &mut BTreeMap<String, Vec<PathBuf>>) {
        let marker = directory.join(".zxs-container-owner.json");
        if marker.is_file() {
            let file: String = json_codec::from_slice(&fs::read(marker).unwrap()).unwrap();
            result.entry(file).or_default().push(directory.to_owned());
            return;
        }
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                visit(&entry.path(), result);
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, &mut result);
    result
}

struct Fixture {
    temporary: tempfile::TempDir,
    primary: PathBuf,
    engine: Engine,
    repository: Repository,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let config = Config::development([temporary.path().join("primary")]).unwrap();
        let primary = config.disks[0].root.clone();
        assert_eq!(config.disks[0].slot_id, "P01");
        let mut engine = Engine::initialize(&temporary.path().join("system"), config).unwrap();
        let repository = engine.create_disk_root("P01", "admin", op()).unwrap();
        assert_eq!(repository.target_copies, 1);
        Self {
            temporary,
            primary,
            engine,
            repository,
        }
    }
    fn reopen(self) -> Self {
        let Self {
            temporary,
            primary,
            engine,
            repository,
        } = self;
        drop(engine);
        let engine = Engine::open(&temporary.path().join("system")).unwrap();
        Self {
            temporary,
            primary,
            engine,
            repository,
        }
    }
    fn assert_initial_backup_listing(&self) {
        assert!(
            self.engine
                .published_backup_directories("P01")
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            self.engine
                .published_backup_directories("P99")
                .unwrap_err()
                .code(),
            ErrorCode::SlotNotFound
        );
    }
    fn entry(&self, path: &str) -> Entry {
        self.engine
            .namespace_tree(&self.repository.main_branch_id)
            .unwrap()
            .get(path)
            .unwrap()
            .clone()
    }
    fn save(&mut self, path: &str, body: &[u8]) -> Entry {
        let workspace = self
            .engine
            .create_workspace(&self.repository.main_branch_id, "admin")
            .unwrap();
        self.engine
            .stage_bytes(&workspace.workspace_id, path, body)
            .unwrap();
        self.engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: op(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "directory layout integration save".into(),
                request_digest: None,
            })
            .unwrap();
        self.entry(path)
    }
    fn prepare_save(&mut self, path: &str, body: &[u8]) -> storage_engine::PreparedWorkspaceCommit {
        let workspace = self
            .engine
            .create_workspace(&self.repository.main_branch_id, "admin")
            .unwrap();
        self.engine
            .stage_bytes(&workspace.workspace_id, path, body)
            .unwrap();
        self.engine
            .prepare_workspace_commit(
                CommitRequest {
                    workspace_id: workspace.workspace_id,
                    operation_id: op(),
                    principal: "admin".into(),
                    expected_head: workspace.base_head,
                    fencing_token: workspace.fencing_token,
                    message: "directory layout detached preparation".into(),
                    request_digest: None,
                },
                None,
            )
            .unwrap()
            .execute()
            .unwrap()
    }
    fn status(&self, path: &str) -> DirectoryGitStatus {
        self.engine
            .directory_git_status(&self.repository.repository_id, path)
            .unwrap()
    }
    fn initialize(&mut self, path: &str) -> DirectoryGitStatus {
        let head = self.engine.state().branches[&self.repository.main_branch_id]
            .head_commit_id
            .clone();
        self.engine
            .initialize_directory_git(
                &self.repository.repository_id,
                &DirectoryGitInitializeBody {
                    path: path.into(),
                    expected_namespace_head: head,
                    operation_id: op(),
                },
                "admin",
            )
            .unwrap();
        self.status(path)
    }
    fn checkpoint(&mut self, path: &str) -> DirectoryGitStatus {
        directory_git_fixture::checkpoint(&mut self.engine, &self.repository, path)
    }
    fn create_branch(&mut self, path: &str, name: &str) -> String {
        let path = (!path.is_empty()).then(|| name_policy::LookupPath::parse(path).unwrap());
        self.engine
            .create_directory_branches(
                &self.repository.repository_id,
                path.as_ref(),
                &BranchName::parse(name).unwrap(),
                "admin",
                op(),
            )
            .unwrap()
            .result["directory_branch_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    fn checkout(&mut self, branch: &str) {
        let tree = self.engine.state().directory_branches[branch]
            .head_tree_id
            .clone();
        let head = self.engine.state().branches[&self.repository.main_branch_id]
            .head_commit_id
            .clone();
        self.engine
            .set_directory_default(DirectoryDefaultSelection {
                repository_id: &self.repository.repository_id,
                directory_branch_id: branch,
                expected_tree_id: &tree,
                expected_head: &head,
                principal: "admin",
                operation_id: op(),
            })
            .unwrap();
    }
    fn rename(&mut self, source: &str, target: &str) {
        self.engine
            .file_namespace(
                &self.repository.repository_id,
                NamespaceOperation::parse(NamespaceOperationKind::Rename, source, Some(target))
                    .unwrap(),
                "admin",
                op(),
            )
            .unwrap();
    }
    fn history(&self) -> PathBuf {
        self.primary
            .join("histories")
            .join(&self.repository.history_tree_id)
    }
    fn root(&self) -> PathBuf {
        self.history()
            .join("directory-repositories")
            .join(&self.repository.repository_id)
    }
    fn retained(&self, logical_root: &Path, entry: &Entry) -> PathBuf {
        // The complete history/file pair matches the pure planner's flat key.
        logical_root.join(".zxs-retained-files").join(format!(
            "f-{}-{}",
            self.repository.history_tree_id, entry.file_id
        ))
    }
    fn assert_body(&self, container: &Path, entry: &Entry, expected: &[u8]) {
        let version = &self.engine.state().versions[&entry.version_id];
        let owner: String =
            json_codec::from_slice(&fs::read(container.join(".zxs-container-owner.json")).unwrap())
                .unwrap();
        assert_eq!(owner, entry.file_id);
        assert_eq!(
            fs::read(container.join("versions").join(&version.storage_name)).unwrap(),
            expected
        );
        let mut opened = self.engine.open_content(&entry.version_id).unwrap();
        let mut actual = Vec::new();
        opened.file.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn saved_uncommitted_main_auto_publication_and_directory_rename_survive_cold_open() {
    let mut fixture = Fixture::new();
    fixture.assert_initial_backup_listing();
    let original = fixture.save("Before/file.txt", b"before initialization");
    let initialized = fixture.initialize("");
    let main = initialized.directory_branch_id.unwrap();
    assert!(initialized.head_tree_id.is_none());
    let saved = fixture.save("Before/file.txt", b"saved but never user committed");
    let added = fixture.save("Before/new.txt", b"new saved main file");
    assert_eq!(saved.file_id, original.file_id);
    let git = &fixture.engine.state().directory_git_repositories[&fixture.repository.repository_id];
    assert!(git.branches[&main].head_tree_id.is_none());
    assert_ne!(
        git.branches[&main].index_tree_id,
        fixture
            .engine
            .directory_git_main_workspace(
                &fixture.repository.repository_id,
                &fixture.repository.repository_id
            )
            .unwrap()
            .head_tree_id
    );
    let receipt = fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id)
        .unwrap();
    fixture.assert_initial_backup_listing();
    assert_eq!(receipt.state_sequence, fixture.engine.state().sequence);
    assert_eq!(receipt.containers, 2);
    assert!(receipt.directories >= 3);
    assert!(receipt.unavailable_containers.is_empty());
    assert!(
        fs::read_dir(fixture.history().join(".zxs-layout/prepared"))
            .unwrap()
            .next()
            .is_none()
    );
    let root = fixture.root();
    fixture.assert_body(
        &root.join("Before/file.txt"),
        &original,
        b"before initialization",
    );
    fixture.assert_body(
        &root.join("Before/file.txt"),
        &saved,
        b"saved but never user committed",
    );
    fixture.assert_body(&root.join("Before/new.txt"), &added, b"new saved main file");
    let directory = fixture.entry("Before").entry_id;
    let before_inode = identity(&root.join("Before"));
    let later = fixture.save(
        "Before/automatic.txt",
        b"automatic publication after activation",
    );
    fixture.assert_body(
        &root.join("Before/automatic.txt"),
        &later,
        b"automatic publication after activation",
    );
    fixture.rename("Before", "After");
    fixture.assert_initial_backup_listing();
    assert_eq!(fixture.entry("After").entry_id, directory);
    assert_eq!(identity(&root.join("After")), before_inode);
    assert!(!root.join("Before").exists());
    fixture.assert_body(
        &root.join("After/file.txt"),
        &saved,
        b"saved but never user committed",
    );
    fixture.assert_body(
        &root.join("After/automatic.txt"),
        &later,
        b"automatic publication after activation",
    );
    assert!(fixture.history().join(".zxs-layout").is_dir());
    assert!(!root.join(".zxs-layout").exists());
    let fixture = fixture.reopen();
    fixture.assert_initial_backup_listing();
    assert_eq!(identity(&fixture.root().join("After")), before_inode);
    fixture.assert_body(
        &fixture.root().join("After/file.txt"),
        &original,
        b"before initialization",
    );
    fixture.assert_body(
        &fixture.root().join("After/file.txt"),
        &saved,
        b"saved but never user committed",
    );
    fixture.assert_body(
        &fixture.root().join("After/automatic.txt"),
        &later,
        b"automatic publication after activation",
    );
    let git = &fixture.engine.state().directory_git_repositories[&fixture.repository.repository_id];
    assert_eq!(git.main_branch_id, main);
    assert!(git.branches[&main].head_tree_id.is_none());
}

#[test]
fn checkout_paths_do_not_relocate_stable_main_and_topic_only_files_are_retained() {
    let mut fixture = Fixture::new();
    let original = fixture.save("main/report.txt", b"main body");
    fixture.initialize("");
    let clean = fixture.checkpoint("");
    let main = clean.directory_branch_id.unwrap();
    let topic = fixture.create_branch("", "topic");
    fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id)
        .unwrap();
    let root = fixture.root();
    let container_inode = identity(&root.join("main/report.txt"));
    fixture.checkout(&topic);
    fixture.rename("main/report.txt", "main/topic-report.txt");
    let changed = fixture.save("main/topic-report.txt", b"topic saved body");
    let topic_only = fixture.save("topic-only.txt", b"topic only body");
    assert_eq!(changed.file_id, original.file_id);
    assert_eq!(
        fixture
            .engine
            .directory_git_main_workspace(
                &fixture.repository.repository_id,
                &fixture.repository.repository_id
            )
            .unwrap()
            .directory_branch_id,
        main
    );
    assert_eq!(identity(&root.join("main/report.txt")), container_inode);
    assert!(!root.join("main/topic-report.txt").exists());
    assert!(!root.join("topic-only.txt").exists());
    fixture.assert_body(&root.join("main/report.txt"), &original, b"main body");
    fixture.assert_body(&root.join("main/report.txt"), &changed, b"topic saved body");
    fixture.assert_body(
        &fixture.retained(&root, &topic_only),
        &topic_only,
        b"topic only body",
    );
    let containers = container_paths(&root);
    assert_eq!(containers[&original.file_id].len(), 1);
    assert_eq!(containers[&topic_only.file_id].len(), 1);
    let fixture = fixture.reopen();
    assert_eq!(
        fixture.engine.state().directory_git_repositories[&fixture.repository.repository_id]
            .current_branch_id,
        topic
    );
    assert_eq!(
        identity(&fixture.root().join("main/report.txt")),
        container_inode
    );
    fixture.assert_body(
        &fixture.root().join("main/report.txt"),
        &changed,
        b"topic saved body",
    );
    fixture.assert_body(
        &fixture.retained(&fixture.root(), &topic_only),
        &topic_only,
        b"topic only body",
    );
}

#[test]
fn independent_child_uses_its_own_main_and_retained_directory_beyond_parent_pin() {
    let mut fixture = Fixture::new();
    let parent_file = fixture.save("parent.txt", b"parent main body");
    let child_original = fixture.save("child/current.txt", b"child committed body");
    fixture.initialize("");
    let child = fixture.initialize("child");
    let child_id = child.directory_id;
    let child_main = child.directory_branch_id.unwrap();
    let committed_child = fixture.checkpoint("child").head_tree_id.unwrap();
    let committed_parent = fixture.checkpoint("").head_tree_id.unwrap();
    let child_topic = fixture.create_branch("child", "child-topic");
    fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id)
        .unwrap();
    fixture.checkout(&child_topic);
    let child_only = fixture.save("child/topic-only.txt", b"child retained body");
    fixture.checkpoint("child");
    fixture.checkout(&child_main);
    fixture.rename("child/current.txt", "child/current-renamed.txt");
    let child_saved = fixture.save(
        "child/current-renamed.txt",
        b"child saved beyond parent pin",
    );
    assert_eq!(child_saved.file_id, child_original.file_id);
    let parent_topic = fixture.create_branch("", "parent-topic");
    fixture.checkout(&parent_topic);
    let parent_only = fixture.save("parent-topic-only.txt", b"parent retained body");
    let root = fixture.root();
    fixture.assert_body(&root.join("parent.txt"), &parent_file, b"parent main body");
    fixture.assert_body(
        &root.join("child/current-renamed.txt"),
        &child_saved,
        b"child saved beyond parent pin",
    );
    fixture.assert_body(
        &root.join("child/current-renamed.txt"),
        &child_original,
        b"child committed body",
    );
    assert!(!root.join("child/current.txt").exists());
    assert!(!root.join("child/topic-only.txt").exists());
    let child_retained = fixture.retained(&root.join("child"), &child_only);
    let parent_retained = fixture.retained(&root, &parent_only);
    fixture.assert_body(&child_retained, &child_only, b"child retained body");
    fixture.assert_body(&parent_retained, &parent_only, b"parent retained body");
    assert!(!fixture.retained(&root, &child_only).exists());
    assert!(!fixture.retained(&root.join("child"), &parent_only).exists());
    assert!(!root.join("child/.zxs-layout").exists());
    assert_eq!(
        fixture.engine.state().directory_trees[&committed_parent].directory_refs[&child_id].tree_id,
        committed_child
    );
    let fixture = fixture.reopen();
    fixture.assert_body(
        &fixture.root().join("child/current-renamed.txt"),
        &child_saved,
        b"child saved beyond parent pin",
    );
    fixture.assert_body(
        &fixture.retained(&fixture.root().join("child"), &child_only),
        &child_only,
        b"child retained body",
    );
    fixture.assert_body(
        &fixture.retained(&fixture.root(), &parent_only),
        &parent_only,
        b"parent retained body",
    );
    assert_eq!(
        fixture.engine.state().directory_git_repositories[&child_id].main_branch_id,
        child_main
    );
    assert_eq!(
        fixture.engine.state().directory_trees[&committed_parent].directory_refs[&child_id].tree_id,
        committed_child
    );
}

#[test]
fn unknown_empty_target_is_rejected_without_changing_original_data() {
    let mut fixture = Fixture::new();
    let entry = fixture.save("kept.txt", b"original bytes must survive refusal");
    fixture.initialize("");
    let legacy = fixture.history().join("file-containers");
    let source = legacy
        .join(&entry.file_id)
        .join("namespace")
        .join(&fixture.engine.state().file_locations[&entry.file_id]);
    let source_inode = identity(&source);
    let before = file_snapshot(&legacy);
    let state = storage_engine::canonical_json(fixture.engine.state()).unwrap();
    let unknown = fixture.root();
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&unknown)
        .unwrap();
    let unknown_inode = identity(&unknown);
    assert!(fs::read_dir(&unknown).unwrap().next().is_none());
    for _ in 0..2 {
        let error = fixture
            .engine
            .reconcile_directory_layout(&fixture.repository.repository_id)
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ContainerLayoutInvalid);
        assert!(
            fs::read_dir(fixture.history().join(".zxs-layout/prepared"))
                .unwrap()
                .next()
                .is_none(),
            "rejected retries must not accumulate private preparation directories"
        );
    }
    assert_eq!(identity(&source), source_inode);
    assert_eq!(file_snapshot(&legacy), before);
    assert_eq!(
        storage_engine::canonical_json(fixture.engine.state()).unwrap(),
        state
    );
    assert_eq!(identity(&unknown), unknown_inode);
    assert!(fs::read_dir(&unknown).unwrap().next().is_none());
    let version = &fixture.engine.state().versions[&entry.version_id];
    assert_eq!(
        fs::read(source.join("versions").join(&version.storage_name)).unwrap(),
        b"original bytes must survive refusal"
    );
    let mut opened = fixture.engine.open_content(&entry.version_id).unwrap();
    let mut body = Vec::new();
    opened.file.read_to_end(&mut body).unwrap();
    assert_eq!(body, b"original bytes must survive refusal");
}

#[test]
fn engine_recovers_after_container_names_without_replaying_old_layout_prefixes() {
    let mut fixture = Fixture::new();
    let original = fixture.save("First/file.txt", b"original version before activation");
    fixture.initialize("");
    fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id)
        .unwrap();
    let root = fixture.root();
    let directory_id = fixture.entry("First").entry_id;
    let directory_inode = identity(&root.join("First"));
    let container_inode = identity(&root.join("First/file.txt"));

    // Several already-published prefixes precede the decided, interrupted rename.
    let first_saved = fixture.save("First/file.txt", b"first saved active version");
    let sibling = fixture.save("First/sibling.txt", b"sibling published after activation");
    fixture.rename("First", "Second");
    let latest = fixture.save("Second/file.txt", b"latest version before the name fault");
    fixture.rename("Second", "Third");
    assert_eq!(original.file_id, first_saved.file_id);
    assert_eq!(original.file_id, latest.file_id);
    assert_eq!(identity(&root.join("Third")), directory_inode);
    assert_eq!(identity(&root.join("Third/file.txt")), container_inode);
    let sibling_inode = identity(&root.join("Third/sibling.txt"));
    let published_prefix = fixture.engine.state().sequence;
    let operation = op();
    fixture.engine.inject_fault_once("after_container_names");
    let error = fixture
        .engine
        .file_namespace(
            &fixture.repository.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Rename, "Third", Some("Final"))
                .unwrap(),
            "admin",
            operation,
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InDoubt);
    assert_eq!(fixture.engine.state().sequence, published_prefix);
    assert_eq!(
        fixture
            .engine
            .operation_record(operation)
            .unwrap()
            .result()
            .commit_state,
        storage_engine::CommitState::InDoubt
    );
    assert_eq!(
        fixture
            .engine
            .reconcile_directory_layout(&fixture.repository.repository_id)
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    // names.apply really ran before the failure, while the live State is still
    // the earlier publication. Recovery must not reapply that State's old name.
    assert_eq!(identity(&root.join("Final")), directory_inode);
    assert_eq!(identity(&root.join("Final/file.txt")), container_inode);
    assert_eq!(identity(&root.join("Final/sibling.txt")), sibling_inode);
    for old in ["First", "Second", "Third"] {
        assert!(
            !root.join(old).exists(),
            "old physical prefix {old} reappeared"
        );
    }

    let assert_final = |fixture: &Fixture| {
        let root = fixture.root();
        assert_eq!(fixture.entry("Final").entry_id, directory_id);
        assert_eq!(fixture.entry("Final/file.txt").file_id, original.file_id);
        assert_eq!(identity(&root.join("Final")), directory_inode);
        assert_eq!(identity(&root.join("Final/file.txt")), container_inode);
        assert_eq!(identity(&root.join("Final/sibling.txt")), sibling_inode);
        for old in ["First", "Second", "Third"] {
            assert!(
                !root.join(old).exists(),
                "cold replay restored physical prefix {old}"
            );
            assert!(
                fixture
                    .engine
                    .namespace_tree(&fixture.repository.main_branch_id)
                    .unwrap()
                    .get(old)
                    .is_none(),
                "cold replay restored logical prefix {old}"
            );
        }
        fixture.assert_body(
            &root.join("Final/file.txt"),
            &original,
            b"original version before activation",
        );
        fixture.assert_body(
            &root.join("Final/file.txt"),
            &first_saved,
            b"first saved active version",
        );
        fixture.assert_body(
            &root.join("Final/file.txt"),
            &latest,
            b"latest version before the name fault",
        );
        fixture.assert_body(
            &root.join("Final/sibling.txt"),
            &sibling,
            b"sibling published after activation",
        );
        let containers = container_paths(&root);
        assert_eq!(containers[&original.file_id].len(), 1);
        assert_eq!(containers[&sibling.file_id].len(), 1);
    };
    let mut fixture = fixture.reopen();
    assert_final(&fixture);
    let completed = fixture
        .engine
        .operation_record(operation)
        .unwrap()
        .into_result();
    assert_eq!(
        completed.commit_state,
        storage_engine::CommitState::Published
    );
    assert!(completed.sequence > published_prefix);
    let recovered_sequence = fixture.engine.state().sequence;
    let replay = fixture
        .engine
        .file_namespace(
            &fixture.repository.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Rename, "Third", Some("Final"))
                .unwrap(),
            "admin",
            operation,
        )
        .unwrap();
    assert_eq!(replay.sequence, completed.sequence);
    assert_eq!(replay.commit_id, completed.commit_id);
    assert_eq!(fixture.engine.state().sequence, recovered_sequence);
    assert_final(&fixture);

    let fixture = fixture.reopen();
    assert_final(&fixture);
    assert_eq!(fixture.engine.state().sequence, recovered_sequence);
    let again = fixture
        .engine
        .operation_record(operation)
        .unwrap()
        .into_result();
    assert_eq!(again.commit_state, storage_engine::CommitState::Published);
    assert_eq!(again.sequence, completed.sequence);
    assert_eq!(again.commit_id, completed.commit_id);
}

#[test]
fn cancelled_preparations_clean_only_their_original_empty_directories() {
    for replace_inode in [false, true] {
        let mut fixture = Fixture::new();
        let kept = fixture.save("kept.txt", b"already published");
        fixture.initialize("");
        fixture
            .engine
            .reconcile_directory_layout(&fixture.repository.repository_id)
            .unwrap();
        let sequence = fixture.engine.state().sequence;
        let prepared = fixture.prepare_save("Cancelled/new.txt", b"not published");
        let preparations = fixture.history().join(".zxs-layout/prepared");
        let requests = fs::read_dir(&preparations)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(requests.len(), 1);
        let private = &requests[0];
        let leaf = fs::read_dir(private)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let original_identity = identity(&leaf);
        let moved = private.join("unknown-original");
        let replacement_identity = if replace_inode {
            fs::rename(&leaf, &moved).unwrap();
            fs::create_dir(&leaf).unwrap();
            let replacement = identity(&leaf);
            assert_ne!(replacement, original_identity);
            Some(replacement)
        } else {
            None
        };
        drop(prepared);
        assert_eq!(fixture.engine.state().sequence, sequence);
        assert!(!fixture.root().join("Cancelled").exists());
        if let Some(replacement) = replacement_identity {
            assert_eq!(identity(&leaf), replacement);
            assert_eq!(identity(&moved), original_identity);
        } else {
            assert!(fs::read_dir(&preparations).unwrap().next().is_none());
        }
        fixture.assert_body(
            &fixture.root().join("kept.txt"),
            &kept,
            b"already published",
        );
    }
}

#[test]
fn finalized_before_layout_application_recovers_after_private_cleanup() {
    let mut fixture = Fixture::new();
    fixture.save("kept.txt", b"already published");
    fixture.initialize("");
    fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id)
        .unwrap();
    let database = fixture.root().join(".zxs-directory.sqlite");
    let before = databases::records(&database);
    let prepared = fixture.prepare_save("Decided/new.txt", b"decided before layout application");
    fixture.engine.inject_fault_once("after_finalize");
    let error = fixture
        .engine
        .finish_workspace_commit(prepared)
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InDoubt);
    assert_eq!(databases::records(&database), before);
    assert!(!fixture.root().join("Decided").exists());
    assert!(
        fs::read_dir(fixture.history().join(".zxs-layout/prepared"))
            .unwrap()
            .next()
            .is_none()
    );
    let fixture = fixture.reopen();
    let entry = fixture.entry("Decided/new.txt");
    assert!(
        databases::records(&database)
            .values()
            .filter_map(|record| record.get("entries"))
            .flat_map(|entries| entries.as_array().unwrap())
            .any(|saved| saved["version_id"] == entry.version_id)
    );
    fixture.assert_body(
        &fixture.root().join("Decided/new.txt"),
        &entry,
        b"decided before layout application",
    );
    assert!(
        fs::read_dir(fixture.history().join(".zxs-layout/prepared"))
            .unwrap()
            .next()
            .is_none()
    );
}

#[path = "directory_layout_execution/databases.rs"]
mod databases;

#[path = "directory_layout_execution/sqlite_review_regressions.rs"]
mod sqlite_review_regressions;

#[path = "directory_layout_execution/sqlite_install_recovery_review.rs"]
mod sqlite_install_recovery_review;

#[path = "directory_layout_execution/file_database_budget.rs"]
mod file_database_budget;

#[path = "directory_layout_execution/object_material.rs"]
mod object_material;

#[path = "directory_layout_execution/rescue_integrity.rs"]
mod rescue_integrity;
