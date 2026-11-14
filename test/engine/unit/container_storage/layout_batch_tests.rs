//! Real held-root namespace moves and stopped-batch recovery.
use super::super::layout_locator::LayoutBinding;
use super::*;
use crate::disk::DiskRoot;
use std::os::unix::fs::{PermissionsExt, symlink};

fn identifier(prefix: &str, digit: char) -> String {
    format!("{prefix}_{}", digit.to_string().repeat(32))
}

fn binding() -> LayoutBinding {
    LayoutBinding::parse(&"a".repeat(64)).unwrap()
}

fn address(directory: &DirectoryId, name: &str) -> FileAddress {
    FileAddress::Directory {
        directory_id: directory.clone(),
        relative_path: HistoryRelative::parse(name).unwrap(),
    }
}

struct Fixture {
    _temporary: tempfile::TempDir,
    root: DiskRoot,
    history: PathBuf,
    history_id: String,
    root_id: DirectoryId,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = DiskRoot::open(temporary.path()).unwrap();
        let history_id = identifier("history", '1');
        let history = root.path().join("histories").join(&history_id);
        for path in [
            ".zxs-layout/staging",
            ".zxs-layout/directories",
            "directory-repositories",
        ] {
            root.private_dir(&history.join(path)).unwrap();
        }
        let root_id = DirectoryId::parse(&identifier("repo", '2')).unwrap();
        let fixture = Self {
            _temporary: temporary,
            root,
            history,
            history_id,
            root_id,
        };
        let source = fixture.root_path();
        fixture.root.private_dir(&source).unwrap();
        let path = relative(&fixture.anchor(), &source).unwrap();
        let intent = DirectoryIntent::new(
            &fixture.history_id,
            fixture.root_id.clone(),
            binding(),
            EntityInode::capture(&fixture.root, &source).unwrap(),
            path.clone(),
            path,
        )
        .unwrap();
        store_directory(&fixture.anchor(), &intent, None).unwrap();
        fixture
    }

    fn anchor(&self) -> HistoryAnchor<'_> {
        HistoryAnchor::new(&self.root, &self.history, &self.history_id).unwrap()
    }

    fn root_path(&self) -> PathBuf {
        self.history
            .join("directory-repositories")
            .join(self.root_id.as_str())
    }

    fn root_change(&self) -> DirectoryMove {
        let intent = load_directory(&self.anchor(), &self.root_id, &mut |_| Ok(())).unwrap();
        DirectoryMove {
            previous: Some(intent.clone()),
            intent,
            previous_parent: None,
            next_parent: None,
            retired: false,
        }
    }

    fn directory(
        &self,
        digit: char,
        from: &str,
        to: &str,
        previous_parent: &DirectoryId,
        next_parent: &DirectoryId,
    ) -> DirectoryMove {
        let id = DirectoryId::parse(&identifier("entry", digit)).unwrap();
        let source = self.root_path().join(from);
        self.root.private_dir(&source).unwrap();
        let from = relative(&self.anchor(), &source).unwrap();
        let previous = DirectoryIntent::new(
            &self.history_id,
            id.clone(),
            binding(),
            EntityInode::capture(&self.root, &source).unwrap(),
            from.clone(),
            from.clone(),
        )
        .unwrap();
        store_directory(&self.anchor(), &previous, None).unwrap();
        let intent = DirectoryIntent::new(
            &self.history_id,
            id,
            binding(),
            previous.inode(),
            from,
            relative(&self.anchor(), &self.root_path().join(to)).unwrap(),
        )
        .unwrap();
        DirectoryMove {
            intent,
            previous: Some(previous),
            previous_parent: Some(previous_parent.clone()),
            next_parent: Some(next_parent.clone()),
            retired: false,
        }
    }

    fn container(&self, source: &Path, file_id: &str) {
        self.root.private_dir(&source.join("versions")).unwrap();
        self.root
            .write_json(&source.join(super::super::OWNER_MARKER), &file_id, false)
            .unwrap();
        self.root
            .write_atomic(&source.join("versions/body.txt"), file_id.as_bytes(), false)
            .unwrap();
    }

    fn file(&self, digit: char, parent: &DirectoryId, name: &str, to: FileAddress) -> FileMove {
        let file_id = identifier("file", digit);
        let parent_intent = load_directory(&self.anchor(), parent, &mut |_| Ok(())).unwrap();
        let parent_path =
            locate_directory(&self.anchor(), &parent_intent, &mut |_| Ok(())).unwrap();
        let source = parent_path.join(name);
        self.container(&source, &file_id);
        self.root
            .private_dir(&self.history.join("file-containers").join(&file_id))
            .unwrap();
        let from = address(parent, name);
        let previous = FileIntent::new(
            &self.history_id,
            &file_id,
            binding(),
            EntityInode::capture(&self.root, &source).unwrap(),
            from.clone(),
            from.clone(),
        )
        .unwrap();
        store_file(&self.anchor(), &previous, None).unwrap();
        let intent = FileIntent::new(
            &self.history_id,
            &file_id,
            binding(),
            previous.inode(),
            from,
            to,
        )
        .unwrap();
        FileMove {
            intent,
            previous: Some(previous),
        }
    }

    fn stable_file(&self, digit: char, name: &str, to: FileAddress) -> FileMove {
        let file_id = identifier("file", digit);
        let source = self
            .history
            .join("file-containers")
            .join(&file_id)
            .join(name);
        self.container(&source, &file_id);
        let from = FileAddress::Stable {
            relative_path: StableRelative::parse(name).unwrap(),
        };
        let previous = FileIntent::new(
            &self.history_id,
            &file_id,
            binding(),
            EntityInode::capture(&self.root, &source).unwrap(),
            from.clone(),
            from.clone(),
        )
        .unwrap();
        store_file(&self.anchor(), &previous, None).unwrap();
        let intent = FileIntent::new(
            &self.history_id,
            &file_id,
            binding(),
            previous.inode(),
            from,
            to,
        )
        .unwrap();
        FileMove {
            intent,
            previous: Some(previous),
        }
    }

    fn assert_file(&self, change: &FileMove, expected: &Path) {
        let stored = load_file(&self.anchor(), change.intent.file_id(), &mut |_| Ok(())).unwrap();
        assert_eq!(stored.from(), stored.to());
        assert_eq!(
            locate_file(&self.anchor(), &stored, &mut |_| Ok(())).unwrap(),
            expected
        );
        assert_eq!(
            EntityInode::capture(&self.root, expected).unwrap(),
            change.intent.inode()
        );
        let owner: String = self
            .root
            .read_json_observed(&expected.join(super::super::OWNER_MARKER), &mut |_| Ok(()))
            .unwrap();
        assert_eq!(owner, change.intent.file_id());
        assert_eq!(
            self.root
                .read_bytes(&expected.join("versions/body.txt"), 1024)
                .unwrap(),
            owner.as_bytes()
        );
    }
}

#[test]
fn two_file_containers_swap_without_replacing_either_inode() {
    let fixture = Fixture::new();
    let left = fixture.file(
        'a',
        &fixture.root_id,
        "left",
        address(&fixture.root_id, "right"),
    );
    let right = fixture.file(
        'b',
        &fixture.root_id,
        "right",
        address(&fixture.root_id, "left"),
    );
    let batch = Batch {
        databases: BTreeMap::new(),
        directories: vec![fixture.root_change()],
        files: vec![left.clone(), right.clone()],
    };
    preflight(&fixture.anchor(), &batch, &[], &mut |_| Ok(())).unwrap();
    apply(&fixture.anchor(), &batch, &mut |_| Ok(())).unwrap();
    fixture.assert_file(&left, &fixture.root_path().join("right"));
    fixture.assert_file(&right, &fixture.root_path().join("left"));
}

#[test]
fn file_and_directory_with_nested_entities_exchange_names_as_whole_containers() {
    let fixture = Fixture::new();
    let directory = fixture.directory('3', "A", "B", &fixture.root_id, &fixture.root_id);
    let nested = fixture.directory(
        '4',
        "A/nested",
        "B/nested",
        directory.intent.directory_id(),
        directory.intent.directory_id(),
    );
    let inside = fixture.file(
        'a',
        nested.intent.directory_id(),
        "data",
        address(nested.intent.directory_id(), "data"),
    );
    let outside = fixture.file('b', &fixture.root_id, "B", address(&fixture.root_id, "A"));
    let batch = Batch {
        databases: BTreeMap::new(),
        directories: vec![fixture.root_change(), directory.clone(), nested.clone()],
        files: vec![inside.clone(), outside.clone()],
    };
    apply(&fixture.anchor(), &batch, &mut |_| Ok(())).unwrap();
    assert_eq!(
        EntityInode::capture(&fixture.root, &fixture.root_path().join("B")).unwrap(),
        directory.intent.inode()
    );
    assert_eq!(
        EntityInode::capture(&fixture.root, &fixture.root_path().join("B/nested")).unwrap(),
        nested.intent.inode()
    );
    fixture.assert_file(&inside, &fixture.root_path().join("B/nested/data"));
    fixture.assert_file(&outside, &fixture.root_path().join("A"));
}

#[test]
fn parent_rename_translates_child_locators_without_staging_the_child() {
    let fixture = Fixture::new();
    let parent = fixture.directory('3', "parent", "renamed", &fixture.root_id, &fixture.root_id);
    let child = fixture.directory(
        '4',
        "parent/child",
        "renamed/child",
        parent.intent.directory_id(),
        parent.intent.directory_id(),
    );
    let file = fixture.file(
        'a',
        child.intent.directory_id(),
        "data",
        address(child.intent.directory_id(), "data"),
    );
    let batch = Batch {
        databases: BTreeMap::new(),
        directories: vec![fixture.root_change(), parent.clone(), child.clone()],
        files: vec![file.clone()],
    };
    let child_stage = fixture.history.join(child.intent.staging().as_path());
    let mut saw_child_stage = false;
    apply(&fixture.anchor(), &batch, &mut |_| {
        if fixture.root.optional_metadata(&child_stage)?.is_some() {
            saw_child_stage = true;
        }
        Ok(())
    })
    .unwrap();
    assert!(!saw_child_stage);
    let stored = load_directory(&fixture.anchor(), child.intent.directory_id(), &mut |_| {
        Ok(())
    })
    .unwrap();
    let child_path = locate_directory(&fixture.anchor(), &stored, &mut |_| Ok(())).unwrap();
    assert_eq!(child_path, fixture.root_path().join("renamed/child"));
    assert_eq!(
        EntityInode::capture(&fixture.root, &child_path).unwrap(),
        child.intent.inode()
    );
    assert_eq!(
        EntityInode::capture(&fixture.root, &fixture.root_path().join("renamed")).unwrap(),
        parent.intent.inode()
    );
    fixture.assert_file(&file, &child_path.join("data"));
}

#[test]
fn preflight_rejects_unadmitted_future_file_id_without_layout_writes() {
    let fixture = Fixture::new();
    let anchor = fixture.anchor();
    let record = directory_locator_path(&anchor, &fixture.root_id);
    let original = fixture.root.read_bytes(&record, 64 * 1024).unwrap();
    let batch = Batch {
        databases: BTreeMap::new(),
        directories: vec![fixture.root_change()],
        files: Vec::new(),
    };
    let valid = FutureFile {
        file_id: identifier("file", 'a'),
        to: address(&fixture.root_id, "future"),
    };
    preflight(&anchor, &batch, std::slice::from_ref(&valid), &mut |_| Ok(())).unwrap();
    let invalid = FutureFile { file_id: identifier("repo", 'a'), ..valid };
    let error = preflight(&anchor, &batch, &[invalid], &mut |_| Ok(())).unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::InvalidId);
    assert_eq!(fixture.root.read_bytes(&record, 64 * 1024).unwrap(), original);
    assert!(fixture.root.optional_metadata(&fixture.root_path().join("future"))
        .unwrap().is_none());
}

#[test]
fn unknown_empty_directory_and_symlink_targets_are_rejected_without_record_writes() {
    for linked in [false, true] {
        let fixture = Fixture::new();
        let file = fixture.file(
            'a',
            &fixture.root_id,
            "source",
            address(&fixture.root_id, "blocked"),
        );
        let blocked = fixture.root_path().join("blocked");
        if linked {
            symlink(fixture.history.join("unrelated"), &blocked).unwrap();
        } else {
            fixture.root.private_dir(&blocked).unwrap();
        }
        let record = file_locator_path(&fixture.anchor(), file.intent.file_id()).unwrap();
        let bytes = fixture.root.read_bytes(&record, 64 * 1024).unwrap();
        let batch = Batch {
            databases: BTreeMap::new(),
            directories: vec![fixture.root_change()],
            files: vec![file.clone()],
        };
        assert!(preflight(&fixture.anchor(), &batch, &[], &mut |_| Ok(())).is_err());
        assert!(apply(&fixture.anchor(), &batch, &mut |_| Ok(())).is_err());
        assert_eq!(fixture.root.read_bytes(&record, 64 * 1024).unwrap(), bytes);
        assert_eq!(
            EntityInode::capture(&fixture.root, &fixture.root_path().join("source")).unwrap(),
            file.intent.inode()
        );
        assert!(fixture.root.optional_metadata(&blocked).unwrap().is_some());
    }
}

#[test]
fn replaced_source_inode_is_rejected_before_persisting_a_new_move() {
    let fixture = Fixture::new();
    let file = fixture.file(
        'a',
        &fixture.root_id,
        "source",
        address(&fixture.root_id, "target"),
    );
    let record = file_locator_path(&fixture.anchor(), file.intent.file_id()).unwrap();
    let bytes = fixture.root.read_bytes(&record, 64 * 1024).unwrap();
    let source = fixture.root_path().join("source");
    let detached = fixture.history.join("detached-container");
    fixture
        .root
        .rename_noreplace_synced(&source, &detached)
        .unwrap();
    fixture.container(&source, file.intent.file_id());
    let replacement = EntityInode::capture(&fixture.root, &source).unwrap();
    assert_ne!(replacement, file.intent.inode());
    let batch = Batch {
        databases: BTreeMap::new(),
        directories: vec![fixture.root_change()],
        files: vec![file.clone()],
    };
    assert!(apply(&fixture.anchor(), &batch, &mut |_| Ok(())).is_err());
    assert_eq!(fixture.root.read_bytes(&record, 64 * 1024).unwrap(), bytes);
    assert_eq!(
        EntityInode::capture(&fixture.root, &source).unwrap(),
        replacement
    );
    assert_eq!(
        EntityInode::capture(&fixture.root, &detached).unwrap(),
        file.intent.inode()
    );
    assert!(
        fixture
            .root
            .optional_metadata(&fixture.root_path().join("target"))
            .unwrap()
            .is_none()
    );
}

fn reload_batch(fixture: &Fixture, batch: &Batch) -> Batch {
    let anchor = fixture.anchor();
    let recorded: BTreeMap<_, _> = batch
        .directories
        .iter()
        .map(|change| {
            let id = change.intent.directory_id();
            (
                id.clone(),
                load_directory(&anchor, id, &mut |_| Ok(())).unwrap(),
            )
        })
        .collect();
    let sources: BTreeMap<_, _> = recorded
        .iter()
        .map(|(id, intent)| {
            (
                id.clone(),
                locate_directory(&anchor, intent, &mut |_| Ok(())).unwrap(),
            )
        })
        .collect();
    let directories = batch
        .directories
        .iter()
        .map(|change| {
            let id = change.intent.directory_id();
            let mut next = change.clone();
            next.previous = Some(recorded[id].clone());
            next.previous_parent = sources
                .iter()
                .find(|(_, path)| Some(path.as_path()) == sources[id].parent())
                .map(|(parent, _)| parent.clone());
            next.intent = DirectoryIntent::new(
                &fixture.history_id,
                id.clone(),
                change.intent.binding().clone(),
                change.intent.inode(),
                relative(&anchor, &sources[id]).unwrap(),
                change.intent.to().clone(),
            )
            .unwrap();
            next
        })
        .collect();
    let files = batch
        .files
        .iter()
        .map(|change| {
            let mut next = change.clone();
            next.previous =
                Some(load_file(&anchor, change.intent.file_id(), &mut |_| Ok(())).unwrap());
            next
        })
        .collect();
    Batch {
        directories,
        files,
        databases: batch.databases.clone(),
    }
}

#[test]
fn stopped_after_file_staging_reloads_exact_records_and_converges() {
    let fixture = Fixture::new();
    let left = fixture.file(
        'a',
        &fixture.root_id,
        "left",
        address(&fixture.root_id, "right"),
    );
    let right = fixture.file(
        'b',
        &fixture.root_id,
        "right",
        address(&fixture.root_id, "left"),
    );
    let batch = Batch {
        databases: BTreeMap::new(),
        directories: vec![fixture.root_change()],
        files: vec![left.clone(), right.clone()],
    };
    let stage = fixture
        .history
        .join(file_staging(left.intent.file_id()).unwrap().as_path());
    let error = apply(&fixture.anchor(), &batch, &mut |_| {
        if fixture.root.optional_metadata(&stage)?.is_some() {
            return Err(invalid("injected stop after a real staged rename"));
        }
        Ok(())
    });
    assert!(error.is_err());
    assert_eq!(
        EntityInode::capture(&fixture.root, &stage).unwrap(),
        left.intent.inode()
    );
    assert_eq!(
        EntityInode::capture(&fixture.root, &fixture.root_path().join("right")).unwrap(),
        right.intent.inode()
    );
    let reloaded = reload_batch(&fixture, &batch);
    apply(&fixture.anchor(), &reloaded, &mut |_| Ok(())).unwrap();
    fixture.assert_file(&left, &fixture.root_path().join("right"));
    fixture.assert_file(&right, &fixture.root_path().join("left"));
    assert!(fixture.root.optional_metadata(&stage).unwrap().is_none());
}

#[test]
fn prepared_root_with_its_final_basename_and_prepared_child_survive_cold_rebuild() {
    let fixture = Fixture::new();
    let root_id = DirectoryId::parse(&identifier("repo", '5')).unwrap();
    let child_id = DirectoryId::parse(&identifier("entry", '6')).unwrap();
    let preparation = fixture
        .history
        .join(".zxs-layout/prepared")
        .join(identifier("req", '9'));
    fixture.root.private_dir(&preparation).unwrap();
    let mut directories = vec![fixture.root_change()];
    for (id, next_parent, target) in [
        (
            root_id.clone(),
            None,
            format!("directory-repositories/{}", root_id.as_str()),
        ),
        (
            child_id.clone(),
            Some(root_id.clone()),
            format!("directory-repositories/{}/child", root_id.as_str()),
        ),
    ] {
        let source = preparation.join(id.as_str());
        fixture
            .root
            .create_private_directory_synced(&source)
            .unwrap();
        let intent = DirectoryIntent::new(
            &fixture.history_id,
            id,
            binding(),
            EntityInode::capture(&fixture.root, &source).unwrap(),
            relative(&fixture.anchor(), &source).unwrap(),
            HistoryRelative::parse(&target).unwrap(),
        )
        .unwrap();
        directories.push(DirectoryMove {
            intent,
            previous: None,
            previous_parent: None,
            next_parent,
            retired: false,
        });
    }
    let file = fixture.stable_file('a', "namespace/source", address(&child_id, "data"));
    let batch = Batch {
        databases: BTreeMap::new(),
        directories,
        files: vec![file.clone()],
    };
    let stage = fixture.history.join(
        super::super::layout_locator::directory_staging(&root_id)
            .unwrap()
            .as_path(),
    );
    assert!(
        apply(&fixture.anchor(), &batch, &mut |_| {
            if fixture.root.optional_metadata(&stage)?.is_some() {
                return Err(invalid("injected stop after prepared-root staging"));
            }
            Ok(())
        })
        .is_err()
    );
    let root_inode = batch.directories[1].intent.inode();
    assert_eq!(
        EntityInode::capture(&fixture.root, &stage).unwrap(),
        root_inode
    );
    let reloaded = reload_batch(&fixture, &batch);
    apply(&fixture.anchor(), &reloaded, &mut |_| Ok(())).unwrap();
    let final_root = fixture
        .history
        .join("directory-repositories")
        .join(root_id.as_str());
    assert_eq!(
        EntityInode::capture(&fixture.root, &final_root).unwrap(),
        root_inode
    );
    assert_eq!(
        EntityInode::capture(&fixture.root, &final_root.join("child")).unwrap(),
        batch.directories[2].intent.inode()
    );
    fixture.assert_file(&file, &final_root.join("child/data"));
}

#[test]
fn stopped_after_intent_writes_retains_traversable_sources_before_file_staging() {
    let fixture = Fixture::new();
    let parent = fixture.directory('3', "A", "B", &fixture.root_id, &fixture.root_id);
    let versions = fixture.directory(
        '4',
        "A/versions",
        "B/versions",
        parent.intent.directory_id(),
        parent.intent.directory_id(),
    );
    let body = fixture.directory(
        '5',
        "A/versions/body.txt",
        "B/versions/body.txt",
        versions.intent.directory_id(),
        versions.intent.directory_id(),
    );
    let inside = fixture.file(
        'a',
        body.intent.directory_id(),
        "data",
        address(body.intent.directory_id(), "data"),
    );
    let outside = fixture.file('b', &fixture.root_id, "B", address(&fixture.root_id, "A"));
    let batch = Batch {
        databases: BTreeMap::new(),
        directories: vec![fixture.root_change(), parent, versions, body],
        files: vec![inside.clone(), outside.clone()],
    };
    preflight(&fixture.anchor(), &batch, &[], &mut |_| Ok(())).unwrap();
    let mut stopped = false;
    assert!(
        apply(&fixture.anchor(), &batch, &mut |_| {
            let recorded = load_file(&fixture.anchor(), outside.intent.file_id(), &mut |_| Ok(()))?;
            if recorded == outside.intent {
                stopped = true;
                return Err(invalid(
                    "injected stop after durable intents before staging",
                ));
            }
            Ok(())
        })
        .is_err()
    );
    assert!(stopped);
    for change in &batch.directories {
        let recorded = load_directory(&fixture.anchor(), change.intent.directory_id(), &mut |_| {
            Ok(())
        })
        .unwrap();
        assert_eq!(recorded.from(), change.intent.from());
        assert_eq!(recorded.to(), change.intent.from());
        assert_eq!(recorded.inode(), change.intent.inode());
        assert_eq!(
            locate_directory(&fixture.anchor(), &recorded, &mut |_| Ok(())).unwrap(),
            fixture.history.join(change.intent.from().as_path())
        );
    }
    fixture.assert_file(
        &inside,
        &fixture.root_path().join("A/versions/body.txt/data"),
    );
    assert_eq!(
        locate_file(&fixture.anchor(), &outside.intent, &mut |_| Ok(())).unwrap(),
        fixture.root_path().join("B")
    );
    let reloaded = reload_batch(&fixture, &batch);
    apply(&fixture.anchor(), &reloaded, &mut |_| Ok(())).unwrap();
    fixture.assert_file(
        &inside,
        &fixture.root_path().join("B/versions/body.txt/data"),
    );
    fixture.assert_file(&outside, &fixture.root_path().join("A"));
}

#[test]
fn staged_ancestor_reloads_following_children_and_old_tails_stop_at_the_new_file() {
    let fixture = Fixture::new();
    let parent = fixture.directory('3', "A", "B", &fixture.root_id, &fixture.root_id);
    let versions = fixture.directory(
        '4',
        "A/versions",
        "B/versions",
        parent.intent.directory_id(),
        parent.intent.directory_id(),
    );
    let body = fixture.directory(
        '5',
        "A/versions/body.txt",
        "B/versions/body.txt",
        versions.intent.directory_id(),
        versions.intent.directory_id(),
    );
    let child = fixture.directory(
        '6',
        "A/versions/body.txt/child",
        "B/versions/body.txt/child",
        body.intent.directory_id(),
        body.intent.directory_id(),
    );
    let inside = fixture.file(
        'a',
        child.intent.directory_id(),
        "data",
        address(child.intent.directory_id(), "data"),
    );
    let outside = fixture.file('b', &fixture.root_id, "B", address(&fixture.root_id, "A"));
    let batch = Batch {
        databases: BTreeMap::new(),
        directories: vec![
            fixture.root_change(),
            parent.clone(),
            versions,
            body,
            child.clone(),
        ],
        files: vec![inside.clone(), outside.clone()],
    };
    let stage = fixture.history.join(parent.intent.staging().as_path());
    assert!(
        apply(&fixture.anchor(), &batch, &mut |_| {
            if fixture.root.optional_metadata(&stage)?.is_some() {
                return Err(invalid("injected stop after ancestor rename"));
            }
            Ok(())
        })
        .is_err()
    );
    let recorded_child =
        load_directory(&fixture.anchor(), child.intent.directory_id(), &mut |_| {
            Ok(())
        })
        .unwrap();
    let staged_child = stage.join("versions/body.txt/child");
    assert_eq!(
        locate_directory(&fixture.anchor(), &recorded_child, &mut |_| Ok(())).unwrap(),
        staged_child
    );
    assert_eq!(
        EntityInode::capture(&fixture.root, &staged_child).unwrap(),
        child.intent.inode()
    );
    let recorded_file =
        load_file(&fixture.anchor(), inside.intent.file_id(), &mut |_| Ok(())).unwrap();
    assert_eq!(
        locate_file(&fixture.anchor(), &recorded_file, &mut |_| Ok(())).unwrap(),
        staged_child.join("data")
    );
    let reloaded = reload_batch(&fixture, &batch);
    apply(&fixture.anchor(), &reloaded, &mut |_| Ok(())).unwrap();
    fixture.assert_file(
        &inside,
        &fixture.root_path().join("B/versions/body.txt/child/data"),
    );
    fixture.assert_file(&outside, &fixture.root_path().join("A"));
    assert!(
        fixture
            .root
            .optional_metadata(&fixture.root_path().join("A/versions/body.txt"))
            .unwrap()
            .unwrap()
            .is_file()
    );
    let again = reload_batch(&fixture, &batch);
    apply(&fixture.anchor(), &again, &mut |_| Ok(())).unwrap();
    fixture.assert_file(
        &inside,
        &fixture.root_path().join("B/versions/body.txt/child/data"),
    );
    fixture.assert_file(&outside, &fixture.root_path().join("A"));
}

#[test]
fn stable_exit_from_a_retired_directory_recovers_and_drops_its_directory_dependency() {
    let fixture = Fixture::new();
    let mut directory = fixture.directory(
        '3',
        "retiring",
        "unused",
        &fixture.root_id,
        &fixture.root_id,
    );
    directory.retired = true;
    directory.next_parent = None;
    directory.intent = DirectoryIntent::new(
        &fixture.history_id,
        directory.intent.directory_id().clone(),
        binding(),
        directory.intent.inode(),
        directory.intent.from().clone(),
        directory.intent.staging().clone(),
    )
    .unwrap();
    let to = FileAddress::Stable {
        relative_path: StableRelative::parse("namespace/archived").unwrap(),
    };
    let file = fixture.file('a', directory.intent.directory_id(), "data", to);
    let stable_parent = fixture
        .history
        .join("file-containers")
        .join(file.intent.file_id())
        .join("namespace");
    fixture.root.private_dir(&stable_parent).unwrap();
    let target = stable_parent.join("archived");
    let batch = Batch {
        databases: BTreeMap::new(),
        directories: vec![fixture.root_change(), directory.clone()],
        files: vec![file.clone()],
    };
    assert!(
        apply(&fixture.anchor(), &batch, &mut |_| {
            if fixture.root.optional_metadata(&target)?.is_some() {
                return Err(invalid("injected stop after stable exit rename"));
            }
            Ok(())
        })
        .is_err()
    );
    let retired = fixture.history.join(directory.intent.staging().as_path());
    assert_eq!(
        EntityInode::capture(&fixture.root, &retired).unwrap(),
        directory.intent.inode()
    );
    let reloaded = reload_batch(&fixture, &batch);
    apply(&fixture.anchor(), &reloaded, &mut |_| Ok(())).unwrap();
    fixture.assert_file(&file, &target);
    let locator = directory_locator_path(&fixture.anchor(), directory.intent.directory_id());
    fixture
        .root
        .rename_noreplace_synced(
            &locator,
            &fixture.history.join("detached-directory-record.json"),
        )
        .unwrap();
    let settled = load_file(&fixture.anchor(), file.intent.file_id(), &mut |_| Ok(())).unwrap();
    assert!(matches!(settled.from(), FileAddress::Stable { .. }));
    assert_eq!(
        locate_file(&fixture.anchor(), &settled, &mut |_| Ok(())).unwrap(),
        target
    );
}

#[test]
fn stable_target_below_its_own_container_is_rejected_before_record_writes() {
    let fixture = Fixture::new();
    let mut file = fixture.file(
        'a',
        &fixture.root_id,
        "source",
        address(&fixture.root_id, "target"),
    );
    let stage = fixture
        .history
        .join(file_staging(file.intent.file_id()).unwrap().as_path());
    fixture
        .root
        .rename_noreplace_synced(&fixture.root_path().join("source"), &stage)
        .unwrap();
    file.intent = FileIntent::new(
        &fixture.history_id,
        file.intent.file_id(),
        binding(),
        file.intent.inode(),
        file.intent.from().clone(),
        FileAddress::Stable {
            relative_path: StableRelative::parse(".zxs-layout-stage/child").unwrap(),
        },
    )
    .unwrap();
    let record = file_locator_path(&fixture.anchor(), file.intent.file_id()).unwrap();
    let bytes = fixture.root.read_bytes(&record, 64 * 1024).unwrap();
    let batch = Batch {
        databases: BTreeMap::new(),
        directories: vec![fixture.root_change()],
        files: vec![file.clone()],
    };
    assert!(preflight(&fixture.anchor(), &batch, &[], &mut |_| Ok(())).is_err());
    assert!(apply(&fixture.anchor(), &batch, &mut |_| Ok(())).is_err());
    assert_eq!(fixture.root.read_bytes(&record, 64 * 1024).unwrap(), bytes);
    assert_eq!(
        EntityInode::capture(&fixture.root, &stage).unwrap(),
        file.intent.inode()
    );
}

#[test]
fn topology_uses_the_smallest_ready_id_and_excludes_retired_directories() {
    let fixture = Fixture::new();
    let late = fixture.directory('9', "late", "late", &fixture.root_id, &fixture.root_id);
    let early_child = fixture.directory(
        '1',
        "late/child",
        "late/child",
        late.intent.directory_id(),
        late.intent.directory_id(),
    );
    let sibling = fixture.directory(
        '8',
        "sibling",
        "sibling",
        &fixture.root_id,
        &fixture.root_id,
    );
    let first = fixture.directory('4', "first", "first", &fixture.root_id, &fixture.root_id);
    let next_child = fixture.directory(
        '2',
        "first/child",
        "first/child",
        first.intent.directory_id(),
        first.intent.directory_id(),
    );
    let mut retired =
        fixture.directory('3', "retired", "unused", &fixture.root_id, &fixture.root_id);
    retired.retired = true;
    retired.next_parent = None;
    retired.intent = DirectoryIntent::new(
        &fixture.history_id,
        retired.intent.directory_id().clone(),
        binding(),
        retired.intent.inode(),
        retired.intent.from().clone(),
        retired.intent.staging().clone(),
    )
    .unwrap();
    let batch = Batch {
        databases: BTreeMap::new(),
        directories: vec![
            early_child.clone(),
            late.clone(),
            retired,
            sibling.clone(),
            first.clone(),
            next_child.clone(),
            fixture.root_change(),
        ],
        files: Vec::new(),
    };
    let sources = batch
        .directories
        .iter()
        .map(|change| {
            let source = locate_directory(
                &fixture.anchor(),
                change.previous.as_ref().unwrap(),
                &mut |_| Ok(()),
            )
            .unwrap();
            (change.intent.directory_id().clone(), source)
        })
        .collect();
    let order = directory_geometry(&directory_changes(&batch).unwrap(), &sources).unwrap();
    assert_eq!(
        order,
        vec![
            fixture.root_id.clone(),
            first.intent.directory_id().clone(),
            next_child.intent.directory_id().clone(),
            sibling.intent.directory_id().clone(),
            late.intent.directory_id().clone(),
            early_child.intent.directory_id().clone(),
        ]
    );
    let mut wrong_parent = batch.clone();
    wrong_parent.directories[0].previous_parent = None;
    assert!(directory_geometry(&directory_changes(&wrong_parent).unwrap(), &sources).is_err());
}
