//! Finite locator recovery without widening a held history/package boundary.
use super::*;
use std::{fs, os::unix::fs::symlink};

fn identifier(prefix: &str, digit: char) -> String {
    format!("{prefix}_{}", digit.to_string().repeat(32))
}
fn binding() -> LayoutBinding {
    LayoutBinding::parse(&"a".repeat(64)).unwrap()
}
fn relative(value: &str) -> HistoryRelative {
    HistoryRelative::parse(value).unwrap()
}
fn directory_address(directory: &DirectoryId, path: &str) -> FileAddress {
    FileAddress::Directory {
        directory_id: directory.clone(),
        relative_path: relative(path),
    }
}

struct Fixture {
    _temporary: tempfile::TempDir,
    root: DiskRoot,
    history: PathBuf,
    history_id: String,
    file_id: String,
    directory_id: DirectoryId,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = DiskRoot::open(temporary.path()).unwrap();
        let history_id = identifier("history", '1');
        let history = root.path().join("histories").join(&history_id);
        root.private_dir(&history).unwrap();
        Self {
            _temporary: temporary,
            root,
            history,
            history_id,
            file_id: identifier("file", '2'),
            directory_id: DirectoryId::parse(&identifier("entry", '3')).unwrap(),
        }
    }
    fn anchor(&self) -> HistoryAnchor<'_> {
        HistoryAnchor::new(&self.root, &self.history, &self.history_id).unwrap()
    }
    fn container(&self, path: &Path, file: &str) {
        self.root.private_dir(&path.join("versions")).unwrap();
        self.root
            .write_json(&path.join(OWNER_MARKER), &file, false)
            .unwrap();
        self.root
            .write_atomic(&path.join("versions/body.txt"), b"preserved", false)
            .unwrap();
    }
    fn directory(&self, from: &str, to: &str) -> DirectoryIntent {
        let path = self.history.join(from);
        self.root.private_dir(&path).unwrap();
        let intent = DirectoryIntent::new(
            &self.history_id,
            self.directory_id.clone(),
            binding(),
            EntityInode::capture(&self.root, &path).unwrap(),
            relative(from),
            relative(to),
        )
        .unwrap();
        store_directory(&self.anchor(), &intent, None).unwrap();
        intent
    }
    fn file_intent(&self, source: &Path, from: FileAddress, to: FileAddress) -> FileIntent {
        FileIntent::new(
            &self.history_id,
            &self.file_id,
            binding(),
            EntityInode::capture(&self.root, source).unwrap(),
            from,
            to,
        )
        .unwrap()
    }
    fn move_directory(&self, from: &Path, to: &Path) {
        self.root.private_dir(to.parent().unwrap()).unwrap();
        self.root
            .rename_noreplace_destination_first_synced(from, to)
            .unwrap();
    }
}

fn stored_directory_tree(fixture: &Fixture) -> BTreeMap<DirectoryId, DirectoryIntent> {
    let parent = fixture.directory("namespace/parent", "namespace/parent");
    let mut intents = BTreeMap::from([(fixture.directory_id.clone(), parent)]);
    for (digit, path) in [
        ('4', "namespace/parent/child"),
        ('5', "namespace/parent/child/grandchild"),
    ] {
        let directory_id = DirectoryId::parse(&identifier("entry", digit)).unwrap();
        let absolute = fixture.history.join(path);
        fixture.root.private_dir(&absolute).unwrap();
        let intent = DirectoryIntent::new(
            &fixture.history_id,
            directory_id.clone(),
            binding(),
            EntityInode::capture(&fixture.root, &absolute).unwrap(),
            relative(path),
            relative(path),
        )
        .unwrap();
        store_directory(&fixture.anchor(), &intent, None).unwrap();
        intents.insert(directory_id, intent);
    }
    intents
}

fn assert_directory_tree(
    fixture: &Fixture,
    intents: &BTreeMap<DirectoryId, DirectoryIntent>,
    prefix: &str,
) {
    let anchor = fixture.anchor();
    for (digit, suffix) in [('3', ""), ('4', "/child"), ('5', "/child/grandchild")] {
        let directory_id = DirectoryId::parse(&identifier("entry", digit)).unwrap();
        let expected = fixture.history.join(format!("{prefix}{suffix}"));
        let intent = &intents[&directory_id];
        assert_eq!(
            locate_directory(&anchor, intent, &mut |_| Ok(())).unwrap(),
            expected
        );
        assert!(intent.inode().matches(&fixture.root, &expected).unwrap());
        assert_eq!(
            load_directory(&anchor, &directory_id, &mut |_| Ok(())).unwrap(),
            *intent
        );
    }
}

#[test]
fn history_anchor_accepts_its_held_history_and_rejects_escape() {
    let fixture = Fixture::new();
    let held_history = DiskRoot::open(&fixture.history).unwrap();
    let anchor = HistoryAnchor::new(&held_history, &fixture.history, &fixture.history_id).unwrap();
    assert_eq!(anchor.path(), fixture.history);
    assert!(HistoryAnchor::new(&held_history, fixture.root.path(), &fixture.history_id).is_err());
    assert!(
        HistoryAnchor::new(
            &fixture.root,
            &fixture.history.join("../outside"),
            &fixture.history_id,
        )
        .is_err()
    );
    let link = fixture.root.path().join("linked-history");
    symlink(&fixture.history, &link).unwrap();
    assert!(HistoryAnchor::new(&fixture.root, &link, &fixture.history_id).is_err());
}

#[test]
fn addresses_and_binding_reject_noncanonical_inputs() {
    for path in ["", ".", "..", "../a", "a/../b", "a/./b", "a//b", "/a", "a/"] {
        assert!(HistoryRelative::parse(path).is_err(), "accepted {path:?}");
        assert!(StableRelative::parse(path).is_err(), "accepted {path:?}");
    }
    for digest in ["", "abc", &"A".repeat(64), &"g".repeat(64)] {
        assert!(LayoutBinding::parse(digest).is_err());
    }
    assert!(DirectoryId::parse(&identifier("repo", 'a')).is_ok());
    assert!(DirectoryId::parse(&identifier("entry", 'b')).is_ok());
    assert!(DirectoryId::parse(&identifier("file", 'c')).is_err());
}

#[test]
fn retained_directory_ids_are_deterministic_and_accept_only_verified_base_ids() {
    for prefix in ["repo", "entry"] {
        let base = DirectoryId::parse(&identifier(prefix, 'a')).unwrap();
        let retained = DirectoryId::retained(&base);
        assert_eq!(retained.as_str(), format!("retained_{}", base.as_str()));
        assert_eq!(DirectoryId::parse(retained.as_str()).unwrap(), retained);
        assert_eq!(DirectoryId::retained(&retained), retained);
        assert!(DirectoryId::parse(&format!("retained_{}", retained.as_str())).is_err());
    }
    for value in [
        "retained_anything".to_owned(),
        format!("retained_{}", identifier("file", 'a')),
        format!("retained_{}", identifier("entry", 'G')),
        format!("retained_{}/child", identifier("entry", 'a')),
    ] {
        assert!(DirectoryId::parse(&value).is_err(), "accepted {value}");
    }
}

#[test]
fn file_follows_source_stage_and_target_without_adopting_a_foreign_occupant() {
    let fixture = Fixture::new();
    fixture.directory("namespace", "namespace");
    let source = fixture
        .history
        .join("file-containers")
        .join(&fixture.file_id)
        .join("namespace/old.txt");
    fixture.container(&source, &fixture.file_id);
    let intent = fixture.file_intent(
        &source,
        FileAddress::Stable {
            relative_path: StableRelative::parse("namespace/old.txt").unwrap(),
        },
        directory_address(&fixture.directory_id, "new.txt"),
    );
    let anchor = fixture.anchor();
    store_file(&anchor, &intent, None).unwrap();
    assert_eq!(
        locate_file(&anchor, &intent, &mut |_| Ok(())).unwrap(),
        source
    );
    let inode = fs::metadata(source.join("versions/body.txt"))
        .unwrap()
        .ino();
    let stage = fixture
        .history
        .join(file_staging(&fixture.file_id).unwrap().as_path());
    fixture.move_directory(&source, &stage);
    assert_eq!(
        locate_file(&anchor, &intent, &mut |_| Ok(())).unwrap(),
        stage
    );
    let target = fixture.history.join("namespace/new.txt");
    fixture.move_directory(&stage, &target);
    fixture.container(&source, &identifier("file", '4'));
    assert_eq!(
        locate_file(&anchor, &intent, &mut |_| Ok(())).unwrap(),
        target
    );
    assert_eq!(
        fs::metadata(target.join("versions/body.txt"))
            .unwrap()
            .ino(),
        inode
    );
    assert_eq!(
        fs::read(target.join("versions/body.txt")).unwrap(),
        b"preserved"
    );
    store_file(&anchor, &intent, None).unwrap();
    assert_eq!(
        load_file(&anchor, &fixture.file_id, &mut |_| Ok(())).unwrap(),
        intent
    );
    sync_retry_tails(
        &anchor,
        &[
            relative(&format!(
                "file-containers/{}/namespace/old.txt",
                fixture.file_id
            )),
            file_staging(&fixture.file_id).unwrap(),
            relative("namespace/new.txt"),
        ],
        &mut |_| Ok(()),
    )
    .unwrap();
}

#[test]
fn legal_initial_container_names_are_never_overwritten_as_locator_metadata() {
    for name in [".zxs-layout-locator.json", FILE_STAGE] {
        let fixture = Fixture::new();
        fixture.directory("namespace", "namespace");
        let source = fixture
            .history
            .join("file-containers")
            .join(&fixture.file_id)
            .join(name);
        fixture.container(&source, &fixture.file_id);
        let inode = EntityInode::capture(&fixture.root, &source).unwrap();
        let body_inode = fs::metadata(source.join("versions/body.txt"))
            .unwrap()
            .ino();
        let intent = fixture.file_intent(
            &source,
            FileAddress::Stable {
                relative_path: StableRelative::parse(name).unwrap(),
            },
            directory_address(&fixture.directory_id, name),
        );
        let anchor = fixture.anchor();
        store_file(&anchor, &intent, None).unwrap();
        let locator = file_locator_path(&anchor, &fixture.file_id).unwrap();
        assert_eq!(
            locator,
            fixture
                .history
                .join(".zxs-layout/files")
                .join(format!("{}.json", fixture.file_id))
        );
        assert!(locator.is_file());
        assert!(inode.matches(&fixture.root, &source).unwrap());
        assert_eq!(
            locate_file(&anchor, &intent, &mut |_| Ok(())).unwrap(),
            source
        );
        assert_eq!(
            load_file(&anchor, &fixture.file_id, &mut |_| Ok(())).unwrap(),
            intent
        );
        store_file(&anchor, &intent, None).unwrap();
        assert_eq!(
            fs::metadata(source.join("versions/body.txt"))
                .unwrap()
                .ino(),
            body_inode
        );
        assert_eq!(
            fs::read(source.join("versions/body.txt")).unwrap(),
            b"preserved"
        );
    }
}

#[test]
fn whole_directory_move_keeps_file_relative_addresses_valid() {
    let fixture = Fixture::new();
    let directory = fixture.directory("namespace/child", "namespace/renamed");
    let source = fixture.history.join("namespace/child/file.txt");
    fixture.container(&source, &fixture.file_id);
    let address = directory_address(&fixture.directory_id, "file.txt");
    let file = fixture.file_intent(&source, address.clone(), address);
    let anchor = fixture.anchor();
    store_file(&anchor, &file, None).unwrap();
    let stage = fixture.history.join(directory.staging().as_path());
    fixture.move_directory(&fixture.history.join(directory.from().as_path()), &stage);
    assert_eq!(
        locate_directory(&anchor, &directory, &mut |_| Ok(())).unwrap(),
        stage
    );
    assert_eq!(
        locate_file(&anchor, &file, &mut |_| Ok(())).unwrap(),
        stage.join("file.txt")
    );
    let target = fixture.history.join(directory.to().as_path());
    fixture.move_directory(&stage, &target);
    fixture
        .root
        .private_dir(&fixture.history.join(directory.from().as_path()))
        .unwrap();
    assert_eq!(
        locate_directory(&anchor, &directory, &mut |_| Ok(())).unwrap(),
        target
    );
    assert_eq!(
        locate_file(&anchor, &file, &mut |_| Ok(())).unwrap(),
        target.join("file.txt")
    );
    assert_eq!(
        load_file(&anchor, &fixture.file_id, &mut |_| Ok(())).unwrap(),
        file
    );
}

#[test]
fn child_prefix_updates_follow_parent_placement_without_guessing_ancestor_stages() {
    let fixture = Fixture::new();
    let parent = fixture.directory("namespace/parent", "namespace/renamed");
    let child_id = DirectoryId::parse(&identifier("entry", '4')).unwrap();
    let child_source = fixture.history.join("namespace/parent/child");
    fixture.root.private_dir(&child_source).unwrap();
    let child_inode = EntityInode::capture(&fixture.root, &child_source).unwrap();
    let child = DirectoryIntent::new(
        &fixture.history_id,
        child_id.clone(),
        binding(),
        child_inode,
        relative("namespace/parent/child"),
        relative("namespace/renamed/child"),
    )
    .unwrap();
    let anchor = fixture.anchor();
    store_directory(&anchor, &child, None).unwrap();
    let source = child_source.join("file.txt");
    fixture.container(&source, &fixture.file_id);
    let address = directory_address(&child_id, "file.txt");
    let file = fixture.file_intent(&source, address.clone(), address);
    store_file(&anchor, &file, None).unwrap();
    let stage = fixture.history.join(parent.staging().as_path());
    fixture.move_directory(&fixture.history.join(parent.from().as_path()), &stage);
    assert!(locate_directory(&anchor, &child, &mut |_| Ok(())).is_err());
    assert!(locate_file(&anchor, &file, &mut |_| Ok(())).is_err());
    let target = fixture.history.join(parent.to().as_path());
    fixture.move_directory(&stage, &target);
    let child_target = target.join("child");
    assert_eq!(
        locate_directory(&anchor, &child, &mut |_| Ok(())).unwrap(),
        child_target
    );
    assert!(child_inode.matches(&fixture.root, &child_target).unwrap());
    assert_eq!(
        locate_file(&anchor, &file, &mut |_| Ok(())).unwrap(),
        child_target.join("file.txt")
    );
}

#[test]
fn file_directory_addresses_require_one_name_but_stable_addresses_can_be_nested() {
    let fixture = Fixture::new();
    fixture.directory("namespace", "namespace");
    let source = fixture.history.join("namespace/a/b");
    fixture.container(&source, &fixture.file_id);
    let inode = EntityInode::capture(&fixture.root, &source).unwrap();
    for (from, to) in [
        (
            directory_address(&fixture.directory_id, "a/b"),
            directory_address(&fixture.directory_id, "b"),
        ),
        (
            directory_address(&fixture.directory_id, "b"),
            directory_address(&fixture.directory_id, "a/b"),
        ),
    ] {
        assert!(
            FileIntent::new(
                &fixture.history_id,
                &fixture.file_id,
                binding(),
                inode,
                from,
                to,
            )
            .is_err()
        );
    }
    assert!(
        FileIntent::new(
            &fixture.history_id,
            &fixture.file_id,
            binding(),
            inode,
            FileAddress::Stable {
                relative_path: StableRelative::parse("namespace/a/b").unwrap()
            },
            directory_address(&fixture.directory_id, "b"),
        )
        .is_ok()
    );
}

#[test]
fn directory_file_can_return_to_its_own_stable_backup_namespace() {
    let fixture = Fixture::new();
    fixture.directory("namespace", "namespace");
    let source = fixture.history.join("namespace/a");
    fixture.container(&source, &fixture.file_id);
    let intent = fixture.file_intent(
        &source,
        directory_address(&fixture.directory_id, "a"),
        FileAddress::Stable {
            relative_path: StableRelative::parse("namespace/backup/prefix/a").unwrap(),
        },
    );
    let anchor = fixture.anchor();
    store_file(&anchor, &intent, None).unwrap();
    let stage = fixture
        .history
        .join(file_staging(&fixture.file_id).unwrap().as_path());
    fixture.move_directory(&source, &stage);
    assert_eq!(
        locate_file(&anchor, &intent, &mut |_| Ok(())).unwrap(),
        stage
    );
    let target = fixture
        .history
        .join("file-containers")
        .join(&fixture.file_id)
        .join("namespace/backup/prefix/a");
    fixture.move_directory(&stage, &target);
    assert_eq!(
        locate_file(&anchor, &intent, &mut |_| Ok(())).unwrap(),
        target
    );
    assert_eq!(
        load_file(&anchor, &fixture.file_id, &mut |_| Ok(())).unwrap(),
        intent
    );
}

#[test]
fn same_file_at_a_different_inode_is_never_a_retry_success() {
    let fixture = Fixture::new();
    fixture.directory("namespace", "namespace");
    let source = fixture.history.join("namespace/a");
    fixture.container(&source, &fixture.file_id);
    let intent = fixture.file_intent(
        &source,
        directory_address(&fixture.directory_id, "a"),
        directory_address(&fixture.directory_id, "b"),
    );
    fixture.container(&fixture.history.join("namespace/b"), &fixture.file_id);
    assert!(locate_file(&fixture.anchor(), &intent, &mut |_| Ok(())).is_err());
    fs::remove_file(source.join(OWNER_MARKER)).unwrap();
    assert!(locate_file(&fixture.anchor(), &intent, &mut |_| Ok(())).is_err());
}

#[test]
fn expected_inode_with_wrong_or_missing_owner_is_rejected() {
    let fixture = Fixture::new();
    fixture.directory("namespace", "namespace");
    let source = fixture.history.join("namespace/a");
    fixture.container(&source, &fixture.file_id);
    let address = directory_address(&fixture.directory_id, "a");
    let intent = fixture.file_intent(&source, address.clone(), address);
    fixture
        .root
        .write_json(&source.join(OWNER_MARKER), &identifier("file", '4'), true)
        .unwrap();
    assert!(locate_file(&fixture.anchor(), &intent, &mut |_| Ok(())).is_err());
    fs::remove_file(source.join(OWNER_MARKER)).unwrap();
    assert!(locate_file(&fixture.anchor(), &intent, &mut |_| Ok(())).is_err());
}

#[test]
fn missing_locator_cannot_be_replaced_using_an_admitted_previous_record() {
    let fixture = Fixture::new();
    fixture.directory("namespace", "namespace");
    let source = fixture.history.join("namespace/a");
    fixture.container(&source, &fixture.file_id);
    let address = directory_address(&fixture.directory_id, "a");
    let intent = fixture.file_intent(&source, address.clone(), address);
    let anchor = fixture.anchor();
    assert!(load_file(&anchor, &fixture.file_id, &mut |_| Ok(())).is_err());
    store_file(&anchor, &intent, None).unwrap();
    let path = file_locator_path(&anchor, &fixture.file_id).unwrap();
    fs::remove_file(&path).unwrap();
    assert!(store_file(&anchor, &intent, Some(&intent)).is_err());
    assert!(!path.exists());
}

#[test]
fn strict_record_admission_rejects_unknown_fields_paths_staging_and_identity() {
    let fixture = Fixture::new();
    fixture.directory("namespace", "namespace");
    let source = fixture.history.join("namespace/a");
    fixture.container(&source, &fixture.file_id);
    let address = directory_address(&fixture.directory_id, "a");
    let intent = fixture.file_intent(&source, address.clone(), address);
    let raw = storage_contract::canonical_json(&RawFileIntent::from(&intent)).unwrap();
    let value: json_codec::Value = json_codec::from_slice(&raw).unwrap();
    let anchor = fixture.anchor();
    let path = file_locator_path(&anchor, &fixture.file_id).unwrap();
    for (pointer, bad) in [
        ("unknown", json_codec::json!(true)),
        ("staging", json_codec::json!("arbitrary-stage")),
        ("history_id", json_codec::json!(identifier("history", '8'))),
        ("file_id", json_codec::json!(identifier("file", '8'))),
    ] {
        let mut damaged = value.clone();
        damaged[pointer] = bad;
        fixture.root.write_json(&path, &damaged, true).unwrap();
        assert!(
            load_file(&anchor, &fixture.file_id, &mut |_| Ok(())).is_err(),
            "accepted {pointer}"
        );
    }
    let mut damaged = value.clone();
    damaged["from"]["relative_path"] = json_codec::json!("../escape");
    fixture.root.write_json(&path, &damaged, true).unwrap();
    assert!(load_file(&anchor, &fixture.file_id, &mut |_| Ok(())).is_err());
    let mut damaged = value.clone();
    damaged["to"]["relative_path"] = json_codec::json!("ordinary/child");
    fixture.root.write_json(&path, &damaged, true).unwrap();
    assert!(load_file(&anchor, &fixture.file_id, &mut |_| Ok(())).is_err());
    let mut damaged = value;
    damaged["source_inode"]["inode"] = json_codec::json!("01");
    fixture.root.write_json(&path, &damaged, true).unwrap();
    assert!(load_file(&anchor, &fixture.file_id, &mut |_| Ok(())).is_err());
    let text = String::from_utf8(raw).unwrap();
    let duplicate = format!(
        "{},\"file_id\":\"{}\"}}",
        text.strip_suffix('}').unwrap(),
        fixture.file_id,
    );
    fixture
        .root
        .write_atomic(&path, duplicate.as_bytes(), true)
        .unwrap();
    assert!(load_file(&anchor, &fixture.file_id, &mut |_| Ok(())).is_err());
}

#[test]
fn directory_metadata_rejects_wrong_key_traversal_and_noncanonical_stage() {
    let fixture = Fixture::new();
    let intent = fixture.directory("namespace", "namespace");
    let raw = storage_contract::canonical_json(&RawDirectoryIntent::from(&intent)).unwrap();
    let value: json_codec::Value = json_codec::from_slice(&raw).unwrap();
    let anchor = fixture.anchor();
    let path = directory_locator_path(&anchor, &fixture.directory_id);
    for (field, bad) in [
        ("directory_id", json_codec::json!(identifier("entry", '9'))),
        ("from", json_codec::json!("../escape")),
        ("staging", json_codec::json!("arbitrary-stage")),
    ] {
        let mut damaged = value.clone();
        damaged[field] = bad;
        fixture.root.write_json(&path, &damaged, true).unwrap();
        assert!(load_directory(&anchor, &fixture.directory_id, &mut |_| Ok(())).is_err());
    }
}

#[test]
fn hardlinked_or_symbolic_locator_json_is_never_loaded_or_replaced() {
    let fixture = Fixture::new();
    fixture.directory("namespace", "namespace");
    let source = fixture.history.join("namespace/a");
    fixture.container(&source, &fixture.file_id);
    let address = directory_address(&fixture.directory_id, "a");
    let intent = fixture.file_intent(&source, address.clone(), address);
    let anchor = fixture.anchor();
    store_file(&anchor, &intent, None).unwrap();
    let path = file_locator_path(&anchor, &fixture.file_id).unwrap();
    assert_eq!(
        path.parent().unwrap(),
        fixture.history.join(".zxs-layout/files")
    );
    let other = fixture.history.join("linked-record.json");
    fs::hard_link(&path, &other).unwrap();
    assert!(load_file(&anchor, &fixture.file_id, &mut |_| Ok(())).is_err());
    assert!(store_file(&anchor, &intent, Some(&intent)).is_err());
    fs::remove_file(&path).unwrap();
    symlink(&other, &path).unwrap();
    assert!(load_file(&anchor, &fixture.file_id, &mut |_| Ok(())).is_err());
    assert!(store_file(&anchor, &intent, Some(&intent)).is_err());
}

#[test]
fn links_at_candidates_and_owner_markers_are_rejected() {
    let fixture = Fixture::new();
    fixture.directory("namespace", "namespace");
    let source = fixture.history.join("namespace/a");
    fixture.container(&source, &fixture.file_id);
    let intent = fixture.file_intent(
        &source,
        directory_address(&fixture.directory_id, "a"),
        directory_address(&fixture.directory_id, "b"),
    );
    let target = fixture.history.join("namespace/b");
    symlink(&source, &target).unwrap();
    assert!(locate_file(&fixture.anchor(), &intent, &mut |_| Ok(())).is_err());
    fs::remove_file(target).unwrap();
    let other = fixture.history.join("linked-owner.json");
    fs::hard_link(source.join(OWNER_MARKER), &other).unwrap();
    assert!(locate_file(&fixture.anchor(), &intent, &mut |_| Ok(())).is_err());
}

#[test]
fn metadata_replacement_requires_exact_previous_intent_and_keeps_inode() {
    let fixture = Fixture::new();
    let old = fixture.directory("namespace", "namespace");
    let next = DirectoryIntent::new(
        &fixture.history_id,
        fixture.directory_id.clone(),
        LayoutBinding::parse(&"b".repeat(64)).unwrap(),
        old.inode(),
        relative("namespace"),
        relative("next"),
    )
    .unwrap();
    let anchor = fixture.anchor();
    assert!(store_directory(&anchor, &next, None).is_err());
    store_directory(&anchor, &next, Some(&old)).unwrap();
    store_directory(&anchor, &next, Some(&old)).unwrap();
    assert_eq!(
        load_directory(&anchor, &fixture.directory_id, &mut |_| Ok(())).unwrap(),
        next
    );
    assert_eq!(
        locate_directory(&anchor, &next, &mut |_| Ok(())).unwrap(),
        fixture.history.join("namespace")
    );
}

#[test]
fn locator_observation_can_cancel_before_using_a_candidate() {
    let fixture = Fixture::new();
    let intent = fixture.directory("namespace", "namespace");
    let result = locate_directory(&fixture.anchor(), &intent, &mut |_| {
        Err(invalid("cancelled observation"))
    });
    assert!(result.is_err());
    assert!(fixture.history.join("namespace").is_dir());
}

#[test]
fn physical_addresses_allow_internal_prefixes_with_explicit_byte_and_depth_limits() {
    assert!(HistoryRelative::parse(&vec!["ordinary"; 40].join("/")).is_ok());
    assert!(HistoryRelative::parse(&vec!["ordinary"; 64].join("/")).is_ok());
    assert!(HistoryRelative::parse(&vec!["ordinary"; 65].join("/")).is_err());
    let mut components = vec!["a".repeat(63); 64];
    components[63].push('a');
    let exact = components.join("/");
    assert_eq!(exact.len(), 4096);
    assert!(HistoryRelative::parse(&exact).is_ok());
    assert!(HistoryRelative::parse(&format!("{exact}b")).is_err());
}

#[test]
fn relocating_parent_preserves_child_grandchild_and_file_entities() {
    let fixture = Fixture::new();
    let mut intents = stored_directory_tree(&fixture);
    let grandchild_id = DirectoryId::parse(&identifier("entry", '5')).unwrap();
    let source = fixture
        .history
        .join("namespace/parent/child/grandchild/file.txt");
    fixture.container(&source, &fixture.file_id);
    let address = directory_address(&grandchild_id, "file.txt");
    let file = fixture.file_intent(&source, address.clone(), address);
    let anchor = fixture.anchor();
    store_file(&anchor, &file, None).unwrap();
    let body_inode = fs::metadata(source.join("versions/body.txt"))
        .unwrap()
        .ino();
    relocate_directory(
        &anchor,
        &mut intents,
        &fixture.directory_id,
        relative("namespace/renamed"),
        &mut |_| Ok(()),
    )
    .unwrap();
    assert_directory_tree(&fixture, &intents, "namespace/renamed");
    let target = fixture
        .history
        .join("namespace/renamed/child/grandchild/file.txt");
    assert_eq!(
        locate_file(&anchor, &file, &mut |_| Ok(())).unwrap(),
        target
    );
    assert_eq!(
        fs::metadata(target.join("versions/body.txt"))
            .unwrap()
            .ino(),
        body_inode
    );
    assert_eq!(
        fs::read(target.join("versions/body.txt")).unwrap(),
        b"preserved"
    );
    assert!(!fixture.history.join("namespace/parent").exists());
}

#[test]
fn parent_stage_then_target_updates_every_following_prefix_and_same_target_only_syncs() {
    let fixture = Fixture::new();
    let mut intents = stored_directory_tree(&fixture);
    let anchor = fixture.anchor();
    let stage = directory_staging(&fixture.directory_id).unwrap();
    let stage_absolute = fixture.history.join(stage.as_path());
    fixture
        .root
        .private_dir(stage_absolute.parent().unwrap())
        .unwrap();
    relocate_directory(
        &anchor,
        &mut intents,
        &fixture.directory_id,
        stage.clone(),
        &mut |_| Ok(()),
    )
    .unwrap();
    assert_directory_tree(&fixture, &intents, stage.as_str());
    relocate_directory(
        &anchor,
        &mut intents,
        &fixture.directory_id,
        relative("namespace/final"),
        &mut |_| Ok(()),
    )
    .unwrap();
    assert_directory_tree(&fixture, &intents, "namespace/final");
    assert!(!stage_absolute.exists());
    let settled = intents.clone();
    relocate_directory(
        &anchor,
        &mut intents,
        &fixture.directory_id,
        relative("namespace/final"),
        &mut |_| Ok(()),
    )
    .unwrap();
    assert_eq!(intents, settled);
    assert_directory_tree(&fixture, &intents, "namespace/final");
}

#[test]
fn relocation_preserves_an_unknown_destination_that_exists_before_intent_writes() {
    let fixture = Fixture::new();
    let mut intents = stored_directory_tree(&fixture);
    let anchor = fixture.anchor();
    let destination = relative("namespace/unknown");
    let unknown = fixture.history.join(destination.as_path());
    fixture.root.private_dir(&unknown).unwrap();
    fixture
        .root
        .write_atomic(&unknown.join("do-not-touch"), b"unknown bytes", false)
        .unwrap();
    let unknown_inode = EntityInode::capture(&fixture.root, &unknown).unwrap();
    assert!(
        relocate_directory(
            &anchor,
            &mut intents,
            &fixture.directory_id,
            destination.clone(),
            &mut |_| Ok(()),
        )
        .is_err()
    );
    assert_directory_tree(&fixture, &intents, "namespace/parent");
    assert_eq!(intents[&fixture.directory_id].to(), &destination);
    assert!(unknown_inode.matches(&fixture.root, &unknown).unwrap());
    assert_eq!(
        fs::read(unknown.join("do-not-touch")).unwrap(),
        b"unknown bytes"
    );
}

#[test]
fn relocation_preserves_a_destination_created_after_all_intents_are_durable() {
    let fixture = Fixture::new();
    let mut intents = stored_directory_tree(&fixture);
    let anchor = fixture.anchor();
    let destination = relative("namespace/late-unknown");
    let unknown = fixture.history.join(destination.as_path());
    let expected = [
        (fixture.directory_id.clone(), destination.clone()),
        (
            DirectoryId::parse(&identifier("entry", '4')).unwrap(),
            relative("namespace/late-unknown/child"),
        ),
        (
            DirectoryId::parse(&identifier("entry", '5')).unwrap(),
            relative("namespace/late-unknown/child/grandchild"),
        ),
    ];
    let mut injected = false;
    let result = relocate_directory(
        &anchor,
        &mut intents,
        &fixture.directory_id,
        destination,
        &mut |_| {
            if !injected {
                let mut ready = true;
                for (directory_id, to) in &expected {
                    if load_directory(&anchor, directory_id, &mut |_| Ok(()))?.to() != to {
                        ready = false;
                        break;
                    }
                }
                if ready {
                    fixture.root.private_dir(&unknown)?;
                    fixture.root.write_atomic(
                        &unknown.join("do-not-touch"),
                        b"late unknown",
                        false,
                    )?;
                    injected = true;
                }
            }
            Ok(())
        },
    );
    assert!(injected);
    assert!(result.is_err());
    assert_directory_tree(&fixture, &intents, "namespace/parent");
    assert_eq!(
        fs::read(unknown.join("do-not-touch")).unwrap(),
        b"late unknown"
    );
}

#[test]
fn mismatched_intermediate_child_intent_stops_parent_before_any_new_intent_write() {
    let fixture = Fixture::new();
    let mut intents = stored_directory_tree(&fixture);
    let anchor = fixture.anchor();
    let child_id = DirectoryId::parse(&identifier("entry", '4')).unwrap();
    let previous = intents[&child_id].clone();
    fixture
        .root
        .private_dir(&fixture.history.join("namespace/foreign"))
        .unwrap();
    let damaged = DirectoryIntent::new(
        &fixture.history_id,
        child_id.clone(),
        previous.binding().clone(),
        previous.inode(),
        relative("namespace/foreign"),
        relative("namespace/foreign"),
    )
    .unwrap();
    store_directory(&anchor, &damaged, Some(&previous)).unwrap();
    intents.insert(child_id, damaged);
    let before = intents.clone();
    let records = intents
        .keys()
        .map(|directory_id| {
            let path = directory_locator_path(&anchor, directory_id);
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect::<Vec<_>>();
    assert!(
        relocate_directory(
            &anchor,
            &mut intents,
            &fixture.directory_id,
            relative("namespace/renamed"),
            &mut |_| Ok(()),
        )
        .is_err()
    );
    assert_eq!(intents, before);
    for (path, bytes) in records {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    assert!(
        fixture
            .history
            .join("namespace/parent/child/grandchild")
            .is_dir()
    );
    assert!(!fixture.history.join("namespace/renamed").exists());
}

#[test]
fn partial_intent_failure_keeps_successful_map_updates_and_never_renames_early() {
    let fixture = Fixture::new();
    let mut intents = stored_directory_tree(&fixture);
    let anchor = fixture.anchor();
    let destination = relative("namespace/renamed");
    let mut stopped = false;
    let result = relocate_directory(
        &anchor,
        &mut intents,
        &fixture.directory_id,
        destination.clone(),
        &mut |_| {
            if load_directory(&anchor, &fixture.directory_id, &mut |_| Ok(()))?.to() == &destination
            {
                stopped = true;
                return Err(invalid("stop after the first durable intent"));
            }
            Ok(())
        },
    );
    assert!(stopped);
    assert!(result.is_err());
    assert_directory_tree(&fixture, &intents, "namespace/parent");
    assert_eq!(intents[&fixture.directory_id].to(), &destination);
    let child_id = DirectoryId::parse(&identifier("entry", '4')).unwrap();
    assert_eq!(intents[&child_id].to(), &relative("namespace/parent/child"));
    assert!(!fixture.history.join("namespace/renamed").exists());
    relocate_directory(
        &anchor,
        &mut intents,
        &fixture.directory_id,
        destination,
        &mut |_| Ok(()),
    )
    .unwrap();
    assert_directory_tree(&fixture, &intents, "namespace/renamed");
}

#[test]
fn every_translated_descendant_path_is_admitted_before_the_first_intent_write() {
    let fixture = Fixture::new();
    let mut intents = stored_directory_tree(&fixture);
    let anchor = fixture.anchor();
    let before = intents.clone();
    let destination = relative(&vec!["target"; 64].join("/"));
    assert!(
        relocate_directory(
            &anchor,
            &mut intents,
            &fixture.directory_id,
            destination,
            &mut |_| Ok(()),
        )
        .is_err()
    );
    assert_eq!(intents, before);
    assert_directory_tree(&fixture, &intents, "namespace/parent");
}
