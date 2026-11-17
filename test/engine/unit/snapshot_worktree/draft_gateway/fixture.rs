use crate::persist::layout;
use crate::snapshot_worktree::drafts::{DirectoryDraftIntent, PendingDirectoryDrafts};
use crate::{CommitRequest, Config, Engine, OperationResult, ReadFile};
use name_policy::{Name, PortablePath};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use storage_contract::file_draft::{
    CommitMode, DraftCommand, DraftLocation, WorkingExpectation, WorkingSelection,
};
use storage_contract::snapshot_worktree::{InitializeSnapshot, SnapshotHandle};
use storage_contract::{ContentHash, Mode, OperationId, Principal};

pub(super) const FIRST_ORIGINAL: &[u8] = b"first immutable directory original";
pub(super) const SECOND_ORIGINAL: &[u8] = b"second immutable directory original";
pub(super) const FIRST_DRAFT: &[u8] = b"the explicitly selected shared working body";
pub(super) const SECOND_DRAFT: &[u8] = b"an unselected shared draft must not leak into the node";
pub(super) const LATER_SECOND_DRAFT: &[u8] = b"a later unselected save is still independently kept";
const PATHS: [&str; 2] = ["first.txt", "second.txt"];

pub(super) struct Fixture {
    pub(super) temporary: tempfile::TempDir,
    pub(super) system: PathBuf,
    disks: [PathBuf; 2],
    pub(super) targets: [DraftLocation; 2],
    pub(super) source: SnapshotHandle,
}

impl Fixture {
    pub(super) fn new() -> (Self, Engine) {
        Self::with_nested_directory(false)
    }

    pub(super) fn with_nested_directory(nested: bool) -> (Self, Engine) {
        let temporary = tempfile::tempdir().unwrap();
        let system = temporary.path().join("system");
        let disks = [temporary.path().join("owner"), temporary.path().join("other")];
        for path in [&system, &disks[0], &disks[1]] {
            fs::create_dir(path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
            assert_eq!(fs::read_dir(path).unwrap().count(), 0);
        }
        let config = Config::independent_roots(Mode::Development, [
            (Name::parse("Draft gateway owner").unwrap(), disks[0].clone()),
            (Name::parse("Draft gateway other").unwrap(), disks[1].clone()),
        ]).unwrap();
        let slot = config.disks[0].slot_id.clone();
        let mut engine = Engine::initialize(&system, config).unwrap();
        let repository = engine.create_disk_root(
            &slot, "admin", OperationId::generate().unwrap(),
        ).unwrap();
        if nested {
            engine.file_namespace(
                &repository.repository_id,
                crate::NamespaceOperation::Mkdir {
                    path: PortablePath::parse("Folder/Child").unwrap(),
                },
                "admin",
                OperationId::generate().unwrap(),
            ).unwrap();
        }
        let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
        for (path, body) in PATHS.into_iter().zip([FIRST_ORIGINAL, SECOND_ORIGINAL]) {
            engine.stage_bytes(&workspace.workspace_id, path, body).unwrap();
        }
        if nested {
            engine.stage_bytes(
                &workspace.workspace_id, "Folder/Child/existing.txt", b"unchanged nested original",
            ).unwrap();
        }
        engine.commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id.clone(),
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head.clone(),
            fencing_token: workspace.fencing_token,
            message: "real immutable directory inputs".into(),
            request_digest: None,
        }).unwrap();
        let entries = engine.list_tree(&repository.main_branch_id).unwrap();
        let targets = PATHS.map(|path| {
            let entry = entries.iter().find(|entry| entry.path() == path).unwrap();
            let selected = engine.file_context(&repository.repository_id, path).unwrap();
            assert_ne!(selected.branch_id, repository.main_branch_id);
            assert_eq!(engine.state().branches[&selected.branch_id].file_id.as_deref(),
                Some(entry.file_id.as_str()));
            DraftLocation {
                repository: repository.repository_id.parse().unwrap(),
                path: PortablePath::parse(path).unwrap(),
                entry: entry.entry_id.parse().unwrap(),
                file: entry.file_id.parse().unwrap(),
            }
        });
        let context = engine.snapshot_root_context(
            repository.repository_id.parse().unwrap(),
        ).unwrap();
        let request = InitializeSnapshot {
            host: context.host,
            source: context.source,
            operation_id: OperationId::generate().unwrap(),
        };
        let source = engine.initialize_directory_snapshot(
            &request, &"admin".parse().unwrap(),
        ).unwrap();
        let fixture = Self { temporary, system, disks, targets, source };
        (fixture, engine)
    }

    pub(super) fn intent<'a>(
        &self,
        selected: &'a [WorkingSelection],
        principal: &'a Principal,
    ) -> DirectoryDraftIntent<'a> {
        DirectoryDraftIntent {
            source: self.source,
            selected,
            operation: OperationId::generate().unwrap(),
            principal,
            message: "only the explicit selected working set",
        }
    }

    pub(super) fn snapshot_path(&self) -> PathBuf {
        self.disks[0].join(layout::COMMON_DATA_ROOT.as_path())
            .join(layout::SNAPSHOT_REPOSITORY.as_path())
    }

    pub(super) fn draft_path(&self, engine: &Engine, target: &DraftLocation) -> PathBuf {
        let record = &engine.state().file_containers[&target.file];
        let payload = record.working.draft().unwrap().1.payload_id();
        self.disks[0].join(layout::COMMON_DATA_ROOT.as_path())
            .join(record.namespace.name.as_str())
            .join(layout::container::DRAFTS.as_path())
            .join(layout::container::draft_payload(payload).as_path())
    }

    pub(super) fn media(&self) -> [BTreeMap<PathBuf, Entity>; 3] {
        [inventory(&self.system), inventory(&self.disks[0]), inventory(&self.disks[1])]
    }

    pub(super) fn expected_view(&self, selected: &[u8]) -> Vec<(PortablePath, Vec<u8>)> {
        vec![
            (self.targets[0].path.clone(), selected.to_vec()),
            (self.targets[1].path.clone(), SECOND_ORIGINAL.to_vec()),
        ]
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Entity {
    device: u64,
    inode: u64,
    mode: u32,
    links: u64,
    body: Option<Vec<u8>>,
}

pub(super) fn inventory(root: &Path) -> BTreeMap<PathBuf, Entity> {
    fn visit(root: &Path, path: &Path, output: &mut BTreeMap<PathBuf, Entity>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        assert!(!metadata.file_type().is_symlink());
        let body = if metadata.is_dir() {
            None
        } else {
            assert!(metadata.is_file());
            Some(fs::read(path).unwrap())
        };
        output.insert(path.strip_prefix(root).unwrap().to_path_buf(), Entity {
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
            links: metadata.nlink(),
            body,
        });
        if metadata.is_dir() {
            let mut children = fs::read_dir(path).unwrap()
                .map(|entry| entry.unwrap().path()).collect::<Vec<_>>();
            children.sort();
            for child in children {
                visit(root, &child, output);
            }
        }
    }
    let mut output = BTreeMap::new();
    visit(root, root, &mut output);
    output
}

pub(super) fn write_draft(
    engine: &mut Engine, target: &DraftLocation, body: &[u8],
) -> OperationResult {
    let principal = "admin".parse().unwrap();
    let working = engine.file_working_state(target).unwrap();
    if working.draft().is_none() {
        let mode = DraftCommand::SetMode {
            target: target.clone(), expected: working.revision(), mode: CommitMode::Manual.into(),
        };
        engine.change_file_working(
            &mode, &principal, OperationId::generate().unwrap(),
        ).unwrap();
    }
    let command = DraftCommand::Write {
        controller: storage_contract::file_draft::SaveController::Manual {},
        target: target.clone(),
        expected: engine.file_working_state(target).unwrap().revision(),
        base: engine.file_draft_base(target).unwrap(),
        content_hash: ContentHash::from_digest(blake3::hash(body)),
        logical_size: body.len() as u64,
    };
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(body).unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    let source: ReadFile = file.into();
    engine.write_file_draft(
        &command, &principal, OperationId::generate().unwrap(), source,
    ).unwrap()
}

pub(super) fn selection(engine: &Engine, target: &DraftLocation) -> WorkingSelection {
    WorkingSelection {
        target: target.clone(),
        expected: WorkingExpectation::from_working(
            &engine.file_working_state(target).unwrap(),
        ).unwrap(),
    }
}

pub(super) fn read_draft(engine: &Engine, target: &DraftLocation) -> Vec<u8> {
    let mut bytes = Vec::new();
    engine.open_file_draft(target).unwrap().read_to_end(&mut bytes).unwrap();
    bytes
}

pub(super) fn read_view(pending: &PendingDirectoryDrafts) -> Vec<(PortablePath, Vec<u8>)> {
    pending.files().map(|entry| {
        let mut bytes = Vec::new();
        entry.open().unwrap().read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes.len() as u64, entry.logical_size());
        (entry.path().clone(), bytes)
    }).collect()
}
