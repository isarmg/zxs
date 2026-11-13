//! Explicit FILE-source export to a fresh root, not in-place host registration.
use crate::{CommitRequest, Config, Engine, OperationId, Repository};
use crate::persist::layout;
use name_policy::PortablePath;
use platform_io::fs_safety::PrivateDirectory;
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};
use storage_contract::history_window::HistoryWindowCancellation;
use storage_contract::working_tree::RootId;
use storage_contract::container_edit::{CommitMode, DraftRevision, SetCommitMode};
use storage_contract::{ContentHash, FileId, Principal};

static SERIAL: Mutex<()> = Mutex::new(());

const BODY: &[u8] = b"frozen actual FILE body";
struct Fixture {
    serial: MutexGuard<'static, ()>,
    temp: tempfile::TempDir,
    engine: Engine,
    repository: Repository,
    version: String,
    root: RootId,
}
impl Fixture {
    fn new() -> Self {
        let serial = SERIAL.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let system = temp.path().join("system");
        let disk = temp.path().join("disk");
        PrivateDirectory::create(&system).unwrap();
        PrivateDirectory::create(&disk).unwrap();
        let mut config = Config::development([disk]).unwrap();
        for disk in &mut config.disks {
            disk.level = 1;
            disk.upstream_slot_id = None;
        }
        let mut engine = Engine::initialize_directory_storage(
            &system,
            config,
            OperationId::generate().unwrap(),
        )
        .unwrap();
        let root = engine.directory_storage_roots().unwrap()[0].root_id;
        let repository = engine
            .create_repository(
                &storage_contract::RepositoryName::parse("source").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        let workspace = engine
            .create_workspace(&repository.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "original.txt", BODY)
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate().unwrap(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "real source".into(),
                request_digest: None,
            })
            .unwrap();
        let version = engine
            .list_tree(&repository.main_branch_id)
            .unwrap()
            .into_iter()
            .find(|entry| entry.path() == "original.txt")
            .unwrap()
            .version_id;
        Self {
            serial,
            temp,
            engine,
            repository,
            version,
            root,
        }
    }

    fn publish(&mut self, operation: OperationId) -> crate::Result<crate::OperationResult> {
        let verified = self
            .engine
            .prepare_directory_snapshot_source(
                self.repository.main_branch_id.parse().unwrap(),
                None,
                Default::default(),
                HistoryWindowCancellation::new(),
            )
            .unwrap()
            .verify(|_| Ok::<(), ()>(()))
            .ok()
            .unwrap();
        self.engine.initialize_directory_snapshot_from_source(
            self.root,
            verified,
            "admin",
            operation,
        )
    }

    fn cold(self) -> Self {
        let Self {
            serial,
            temp,
            engine,
            repository,
            version,
            root,
        } = self;
        drop(engine);
        let engine = Engine::open(&temp.path().join("system")).unwrap();
        Self {
            serial,
            temp,
            engine,
            repository,
            version,
            root,
        }
    }

    fn assert_body(&self, operation: OperationId) -> (u64, u64) {
        let entries = self.engine.list_initialized_directory(operation).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path.as_str(), "original.txt");
        assert_eq!(entries[0].file.unwrap().version_id.to_string(), self.version);
        let mut opened = self
            .engine
            .open_initialized_directory_file(
                operation,
                &PortablePath::parse("original.txt").unwrap(),
            )
            .unwrap();
        let metadata = opened.file.metadata().unwrap();
        let mut body = Vec::new();
        opened.file.read_to_end(&mut body).unwrap();
        assert_eq!(body, BODY);
        assert_eq!(opened.metadata.version_id.to_string(), self.version);
        assert_eq!(metadata.nlink(), 1);
        (metadata.dev(), metadata.ino())
    }

    fn manual_source(&mut self) -> FileId {
        let entry = self.engine.list_tree(&self.repository.main_branch_id).unwrap()
            .into_iter().find(|entry| entry.path() == "original.txt").unwrap();
        let file: FileId = entry.file_id.parse().unwrap();
        self.engine.set_container_commit_mode(
            &SetCommitMode {
                file,
                expected_generation: self.engine.container_edit(file).unwrap().generation(),
                mode: CommitMode::Manual,
            },
            &Principal::new("admin".into()).unwrap(),
            OperationId::generate().unwrap(),
        ).unwrap();
        file
    }

    fn save_source_draft(&mut self, file: FileId) -> DraftRevision {
        let entry = self.engine.list_tree(&self.repository.main_branch_id).unwrap()
            .into_iter().find(|entry| entry.path() == "original.txt").unwrap();
        let body = b"uncommitted source must remain readable";
        let source = self.temp.path().join("draft-upload");
        fs::write(&source, body).unwrap();
        let revision = DraftRevision {
            operation: OperationId::generate().unwrap(),
            expected_generation: self.engine.container_edit(file).unwrap().generation(),
            principal: Principal::new("admin".into()).unwrap(),
            base: self.engine.current_container_draft_base(
                file,
                self.repository.repository_id.parse().unwrap(),
                entry.entry_id.parse().unwrap(),
            ).unwrap(),
            content_hash: ContentHash::from_digest(blake3::hash(body)),
            size: body.len() as u64,
        };
        self.engine.save_container_draft(
            file, &revision, fs::File::open(&source).unwrap(),
        ).unwrap();
        revision
    }

    fn assert_source_draft(&self, file: FileId, revision: &DraftRevision) {
        let mut draft = self.engine.open_container_draft(file).unwrap().unwrap();
        assert_eq!(&draft.revision, revision);
        let mut body = Vec::new();
        draft.file.read_to_end(&mut body).unwrap();
        assert_eq!(body, b"uncommitted source must remain readable");
        let mut historical = self.engine.open_content(&self.version).unwrap();
        body.clear();
        historical.file.read_to_end(&mut body).unwrap();
        assert_eq!(body, BODY);
    }
}

type Image = BTreeMap<PathBuf, (Vec<u8>, u64, u64, u32)>;

fn image(root: &Path) -> Image {
    fn visit(base: &Path, path: &Path, output: &mut Image) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let bytes = if metadata.is_file() {
            fs::read(path).unwrap()
        } else {
            Vec::new()
        };
        output.insert(
            path.strip_prefix(base).unwrap().to_owned(),
            (bytes, metadata.dev(), metadata.ino(), metadata.mode()),
        );
        if metadata.is_dir() {
            for child in fs::read_dir(path).unwrap() {
                visit(base, &child.unwrap().path(), output);
            }
        }
    }
    let mut output = BTreeMap::new();
    visit(root, root, &mut output);
    output
}

fn repository_path(fixture: &Fixture) -> PathBuf {
    fixture
        .temp
        .path()
        .join("disk")
        .join(layout::COMMON_DATA_ROOT.as_path())
        .join(layout::SNAPSHOT_REPOSITORY.as_path())
}

#[test]
fn actual_file_source_cold_snapshot_is_independent_and_exact_retry_does_not_publish() {
    let mut fixture = Fixture::new();
    let versions = fixture.engine.state().versions.clone();
    let branches = fixture.engine.state().branches.clone();
    let commits = fixture.engine.state().commits.clone();
    let mut source = fixture.engine.open_content(&fixture.version).unwrap();
    let source_metadata = source.file.metadata().unwrap();
    let operation = OperationId::generate().unwrap();
    let published = fixture.publish(operation).unwrap();
    assert_eq!(
        fixture
            .engine
            .state()
            .operation_principals
            .get(&operation)
            .map(String::as_str),
        Some("admin"),
    );
    let copied = fixture.assert_body(operation);
    assert_ne!(copied, (source_metadata.dev(), source_metadata.ino()));
    let mut body = Vec::new();
    source.file.read_to_end(&mut body).unwrap();
    assert_eq!(body, BODY);
    drop(source);
    assert_eq!(fixture.engine.state().versions, versions);
    assert_eq!(fixture.engine.state().branches, branches);
    assert_eq!(fixture.engine.state().commits, commits);
    let mut fixture = fixture.cold();
    assert_eq!(
        fixture
            .engine
            .state()
            .operation_principals
            .get(&operation)
            .map(String::as_str),
        Some("admin"),
    );
    assert_eq!(fixture.assert_body(operation), copied);
    let state = fixture.engine.state().clone();
    let material = image(&repository_path(&fixture));
    let retried = fixture.publish(operation).unwrap();
    assert_eq!(retried.operation_id, published.operation_id);
    assert_eq!(retried.sequence, published.sequence);
    assert_eq!(fixture.engine.state(), &state);
    assert_eq!(image(&repository_path(&fixture)), material);
    let mut source = fixture.engine.open_content(&fixture.version).unwrap();
    let mut body = Vec::new();
    source.file.read_to_end(&mut body).unwrap();
    assert_eq!(body, BODY);
}

#[test]
fn unknown_repository_is_rejected_before_any_disk_or_system_mutation() {
    let mut fixture = Fixture::new();
    let repository = repository_path(&fixture);
    PrivateDirectory::create(&repository).unwrap();
    fs::write(repository.join("unknown-original"), b"must remain untouched").unwrap();
    let before = image(fixture.temp.path());
    let state = fixture.engine.state().clone();
    assert!(fixture.publish(OperationId::generate().unwrap()).is_err());
    assert_eq!(fixture.engine.state(), &state);
    assert_eq!(image(fixture.temp.path()), before);
}

#[test]
fn dirty_file_source_is_rejected_without_mutation_and_remains_readable_after_cold_open() {
    let mut fixture = Fixture::new();
    let file = fixture.manual_source();
    let versions = fixture.engine.state().versions.clone();
    let commits = fixture.engine.state().commits.clone();
    let branches = fixture.engine.state().branches.clone();
    let revision = fixture.save_source_draft(file);
    let operation = OperationId::generate().unwrap();
    for cold in [false, true] {
        if cold {
            fixture = fixture.cold();
        }
        let state = fixture.engine.state().clone();
        let before = image(fixture.temp.path());
        let error = fixture.publish(operation).unwrap_err();
        assert!(matches!(
            error,
            crate::Error::SnapshotInitialization(source)
                if matches!(*source, super::SnapshotInitializationError::UncommittedSource(id)
                    if id == file)
        ));
        assert_eq!(fixture.engine.state(), &state);
        assert_eq!(image(fixture.temp.path()), before);
        assert!(!repository_path(&fixture).exists());
        assert!(!fixture.engine.state().operations.contains_key(&operation));
        assert_eq!(fixture.engine.state().versions, versions);
        assert_eq!(fixture.engine.state().commits, commits);
        assert_eq!(fixture.engine.state().branches, branches);
        fixture.assert_source_draft(file, &revision);
    }
}

#[test]
fn clean_manual_source_initializes_and_later_dirty_draft_does_not_block_original_receipt() {
    let mut fixture = Fixture::new();
    let file = fixture.manual_source();
    let edit = fixture.engine.container_edit(file).unwrap();
    let versions = fixture.engine.state().versions.clone();
    let commits = fixture.engine.state().commits.clone();
    let branches = fixture.engine.state().branches.clone();
    let operation = OperationId::generate().unwrap();
    let published = fixture.publish(operation).unwrap();
    let copied = fixture.assert_body(operation);
    assert_eq!(fixture.engine.container_edit(file).unwrap(), edit);
    assert_eq!(fixture.engine.state().versions, versions);
    assert_eq!(fixture.engine.state().commits, commits);
    assert_eq!(fixture.engine.state().branches, branches);
    let revision = fixture.save_source_draft(file);
    let mut fixture = fixture.cold();
    let state = fixture.engine.state().clone();
    let before = image(fixture.temp.path());
    let retried = fixture.publish(operation).unwrap();
    assert_eq!(retried, published);
    assert_eq!(fixture.engine.state(), &state);
    assert_eq!(image(fixture.temp.path()), before);
    assert_eq!(fixture.assert_body(operation), copied);
    fixture.assert_source_draft(file, &revision);
}

#[test]
fn snapshot_payload_corruption_or_node_replacement_is_not_repaired_or_adopted() {
    for replace_node in [false, true] {
        let mut fixture = Fixture::new();
        let operation = OperationId::generate().unwrap();
        fixture.publish(operation).unwrap();
        let inode = fixture.assert_body(operation);
        let repository = repository_path(&fixture);
        let original = image(&repository);
        let relative = original
            .iter()
            .find(|(_, (_, dev, ino, _))| (*dev, *ino) == inode)
            .unwrap()
            .0
            .clone();
        let payload = repository.join(relative);
        if replace_node {
            // Payload parent is files; its parent is the immutable node directory.
            let node = payload.parent().unwrap().parent().unwrap();
            let retained = fixture.temp.path().join("retained-original-node");
            fs::rename(node, &retained).unwrap();
            PrivateDirectory::create(node).unwrap();
        } else {
            // Keep the exact inode and original mode so this proves complete
            // body verification, not merely replacement or permission refusal.
            let mode = fs::metadata(&payload).unwrap().mode();
            fs::set_permissions(&payload, fs::Permissions::from_mode(0o600)).unwrap();
            fs::write(&payload, vec![b'X'; BODY.len()]).unwrap();
            fs::set_permissions(&payload, fs::Permissions::from_mode(mode)).unwrap();
            let changed = fs::metadata(&payload).unwrap();
            assert_eq!((changed.dev(), changed.ino()), inode);
        }
        let changed = image(&repository);
        assert!(fixture.engine.list_initialized_directory(operation).is_err());
        assert!(
            fixture
                .engine
                .open_initialized_directory_file(
                    operation,
                    &PortablePath::parse("original.txt").unwrap(),
                )
                .is_err()
        );
        assert_eq!(image(&repository), changed);
        let system = fixture.temp.path().join("system");
        drop(fixture.engine);
        assert!(Engine::open(&system).is_err());
        assert_eq!(image(&repository), changed);
    }
}

#[test]
fn offline_disk_does_not_become_an_absent_repository_or_initialize_a_replacement() {
    let mut fixture = Fixture::new();
    let operation = OperationId::generate().unwrap();
    fixture.publish(operation).unwrap();
    let system = fixture.temp.path().join("system");
    let disk = fixture.temp.path().join("disk");
    let offline = fixture.temp.path().join("offline-disk");
    drop(fixture.engine);
    fs::rename(&disk, &offline).unwrap();
    let before = image(&offline);
    assert!(Engine::open(&system).is_err());
    assert!(!disk.exists());
    assert_eq!(image(&offline), before);
}

#[cfg(feature = "fault-injection")]
#[test]
fn decided_initialization_cold_recovery_keeps_original_operation_and_material() {
    for fault in [
        "after_accept_1",
        "after_finalize",
        "snapshot_after_material_install",
        "snapshot_after_material_proof",
        "after_publication_1",
    ] {
        let mut fixture = Fixture::new();
        let operation = OperationId::generate().unwrap();
        fixture.engine.inject_fault_once(fault);
        assert!(fixture.publish(operation).is_err(), "{fault}");
        let installed = match fault {
            "snapshot_after_material_install" | "snapshot_after_material_proof" => {
                let repository = repository_path(&fixture);
                let before = image(&repository);
                let proof = repository.join(layout::snapshot_init::DECISION.as_path());
                assert_eq!(proof.exists(), fault == "snapshot_after_material_proof");
                Some(before)
            }
            _ => None,
        };
        let mut fixture = fixture.cold();
        fixture.assert_body(operation);
        if let Some(installed) = installed {
            let after = image(&repository_path(&fixture));
            // Recovery may add only the decided proof. Every original entry,
            // body byte, inode and mode must survive the cold installation retry.
            for (path, original) in &installed {
                assert_eq!(after.get(path), Some(original), "{}", path.display());
            }
            let expected_added = usize::from(fault == "snapshot_after_material_install");
            assert_eq!(after.len(), installed.len() + expected_added);
        }
        assert_eq!(
            fixture.engine.state().operation_principals.get(&operation).map(String::as_str),
            Some("admin"),
        );
        let state = fixture.engine.state().clone();
        let material = image(&repository_path(&fixture));
        let result = fixture.publish(operation).unwrap();
        assert_eq!(result.operation_id, operation);
        assert_eq!(fixture.engine.state(), &state);
        assert_eq!(image(&repository_path(&fixture)), material);
    }

    // A retained FINALIZE is not permission to replace an unknown target or
    // reconstruct the original snapshot by adopting the target's path spelling.
    let mut fixture = Fixture::new();
    let operation = OperationId::generate().unwrap();
    fixture.engine.inject_fault_once("snapshot_after_material_install");
    assert!(fixture.publish(operation).is_err());
    let repository = repository_path(&fixture);
    let retained = fixture.temp.path().join("retained-original-repository");
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fs::rename(&repository, &retained).unwrap();
    PrivateDirectory::create(&repository).unwrap();
    let before = image(fixture.temp.path());
    let error = Engine::open(&system).err().unwrap();
    assert!(matches!(
        error,
        crate::Error::SnapshotInitialization(source)
            if matches!(*source, super::SnapshotInitializationError::Material(_))
    ));
    // Reject before historical replay may even replace a byte-identical FILE
    // cache. The complete image includes every inode, not only the unknown target.
    assert_eq!(image(fixture.temp.path()), before);
}
