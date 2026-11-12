//! Safety boundary only: an unimplemented topology replacement cannot become
//! a future-only policy edit or an ordinary membership addition. These are not
//! successful migration tests and must not be reported as that capability.
use storage_engine as engine_api;
#[path = "../support/container_tags.rs"]
mod container_tags_fixture;
use super::*;
use std::{io::Read, os::unix::fs::DirBuilderExt, path::PathBuf};
use storage_engine::{
    BranchName, DeleteFileRequest, FileDeletionTarget, PendingMemberDirectory, ValidatedConfig,
    history_window::{HistoryFileId, HistoryRepositoryId},
};

#[derive(Debug, PartialEq, Eq)]
enum StoredEntry {
    Directory,
    File(Vec<u8>),
}

fn inventory(root: &Path) -> BTreeMap<PathBuf, StoredEntry> {
    fn visit(root: &Path, at: &Path, result: &mut BTreeMap<PathBuf, StoredEntry>) {
        for entry in fs::read_dir(at).unwrap() {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            let key = path.strip_prefix(root).unwrap().to_path_buf();
            if metadata.is_dir() {
                result.insert(key, StoredEntry::Directory);
                visit(root, &path, result);
            } else {
                assert!(metadata.is_file(), "fixture must not contain special files");
                result.insert(key, StoredEntry::File(fs::read(path).unwrap()));
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

fn body(engine: &Engine, version: &str) -> Vec<u8> {
    let mut opened = engine.open_content(version).unwrap();
    let mut bytes = Vec::new();
    opened.file.read_to_end(&mut bytes).unwrap();
    bytes
}

fn replacement(fixture: &Fixture, root: &Path, same_slot: bool) -> Config {
    let mut target = fixture.engine.config().clone();
    let old = target.disks.remove(1);
    let mut new = old;
    new.disk_id = storage_engine::new_id("disk").unwrap();
    if same_slot {
        new.binding_generation += 1;
    } else {
        new.slot_id = "B02".into();
        new.binding_generation = 1;
    }
    new.root = root.into();
    target
        .disks
        .iter_mut()
        .find(|disk| disk.slot_id == "C01")
        .unwrap()
        .upstream_slot_id = Some(new.slot_id.clone());
    target.disks.push(new);
    // This is a structurally valid replacement, not an invalid-topology test.
    ValidatedConfig::try_from(target.clone()).unwrap();
    target
}

fn reject_replacement(fixture: &mut Fixture, with_directory: bool, same_slot: bool) {
    let destination = fixture.temp.path().join(format!(
        "replacement-held-{with_directory}-same-slot-{same_slot}"
    ));
    let target = replacement(fixture, &destination, same_slot);
    let directories = if with_directory {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&destination)
            .unwrap();
        vec![PendingMemberDirectory {
            disk_id: target.disks.last().unwrap().disk_id.clone(),
            directory: fs::File::open(&destination).unwrap(),
        }]
    } else {
        Vec::new()
    };
    let before = storage_engine::canonical_json(fixture.engine.state()).unwrap();
    let disk_contents = fixture
        .config
        .disks
        .iter()
        .map(|disk| inventory(&disk.root))
        .collect::<Vec<_>>();
    let id = operation();
    let result = if with_directory {
        fixture
            .engine
            .begin_membership_handoff_with_directories(target, "admin", id, directories)
    } else {
        fixture.engine.begin_membership_handoff(target, "admin", id)
    };
    assert_eq!(result.unwrap_err().code(), ErrorCode::ConfigConflict);
    assert!(fixture.engine.membership_fence_status().is_none());
    assert!(!fixture.engine.is_read_only());
    assert!(!fixture.engine.state().operations.contains_key(&id));
    assert_eq!(
        storage_engine::canonical_json(fixture.engine.state()).unwrap(),
        before
    );
    for (disk, expected) in fixture.config.disks.iter().zip(disk_contents) {
        assert_eq!(inventory(&disk.root), expected);
    }
    if with_directory {
        assert!(inventory(&destination).is_empty());
    } else {
        assert!(!destination.exists());
    }
}

#[test]
fn initial_topology_replacement_is_not_a_future_only_addition() {
    let mut fixture = Fixture::initial(true, 3);
    let old = fixture.save("file", b"retained old body");
    let latest = fixture.save("file", b"current body");
    for same_slot in [false, true] {
        for with_directory in [false, true] {
            reject_replacement(&mut fixture, with_directory, same_slot);
        }
    }
    let before = fixture.engine.state().clone();
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    assert_eq!(fixture.engine.state(), &before);
    assert_eq!(body(&fixture.engine, &old.version_id), b"retained old body");
    assert_eq!(body(&fixture.engine, &latest.version_id), b"current body");
    fixture.save("after-rejection", b"old authority still writable");
}

#[test]
fn current_replacement_rejection_preserves_hidden_history_tags_tree_and_receipts() {
    let mut fixture = Fixture::new(true);
    let old = fixture.save("docs/report", b"retained old body");
    let latest = fixture.save("docs/report", b"current body");
    let branch = fixture
        .engine
        .create_file_branch(
            &old.file_id,
            &old.version_id,
            &BranchName::parse("retained").unwrap(),
            "admin",
            operation(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    let head = fixture.engine.file_branch_head(&branch).unwrap();
    let tag_receipt = container_tags_fixture::edit(&mut fixture.engine, &old.file_id, &["must-migrate".into()], &[], "admin", operation())
        .unwrap();
    let hidden = fixture.save("docs/hidden", b"deleted does not mean purged");
    let repository = HistoryRepositoryId::parse(&fixture.repository.repository_id).unwrap();
    let file = HistoryFileId::parse(&hidden.file_id).unwrap();
    let path = name_policy::LookupPath::parse("docs/hidden").unwrap();
    let deletion_receipt = fixture
        .engine
        .soft_delete_file(DeleteFileRequest {
            target: FileDeletionTarget {
                repository_id: &repository,
                file_id: &file,
                path: &path,
            },
            principal: "admin",
            operation_id: operation(),
        })
        .unwrap();
    fixture
        .engine
        .file_namespace(
            &fixture.repository.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "docs/empty", None).unwrap(),
            "admin",
            operation(),
        )
        .unwrap();
    let scope = fixture
        .engine
        .backup_configuration(&fixture.repository.repository_id, "docs")
        .unwrap();
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let backup_receipt = fixture
        .engine
        .configure_recursive_backup_tree(RecursiveBackupConfigurationRequest {
            repository_id: &fixture.repository.repository_id,
            path: "docs",
            expected_scope_id: &scope.scope.id,
            target_copies: 3,
            expected_generation: 0,
            capture: &capture,
            principal: "admin",
            operation_id: operation(),
        })
        .unwrap();
    let rows = fixture.engine.published_backup_directories("B01").unwrap();
    assert!(
        rows.iter()
            .any(|row| row.path().as_str().ends_with("/empty"))
    );
    let before = fixture.engine.state().clone();
    for same_slot in [false, true] {
        for with_directory in [false, true] {
            reject_replacement(&mut fixture, with_directory, same_slot);
        }
    }
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    assert_eq!(fixture.engine.state(), &before);
    assert_eq!(
        fixture.engine.published_backup_directories("B01").unwrap(),
        rows
    );
    assert_eq!(
        container_tags_fixture::tags(&fixture.engine, &old.file_id),
        ["must-migrate".to_owned()].into_iter().collect()
    );
    assert_eq!(body(&fixture.engine, &old.version_id), b"retained old body");
    assert_eq!(body(&fixture.engine, &latest.version_id), b"current body");
    assert_eq!(
        body(&fixture.engine, &hidden.version_id),
        b"deleted does not mean purged"
    );
    for receipt in [tag_receipt, deletion_receipt, backup_receipt] {
        assert_eq!(
            fixture.engine.operation(receipt.operation_id).unwrap(),
            receipt
        );
    }
    fixture.save("after-rejection", b"current authority still writable");
}
