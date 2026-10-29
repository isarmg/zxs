//! Recovery must authenticate the complete published closure before opening.
use crate::journal::{Snapshot, load_authenticated_object, load_object, object_path};
use crate::{Config, Engine, Error, ErrorCode};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use storage_contract::{
    EventDirectoryEntry, NamespaceOperation, NamespaceOperationKind, OperationDirectoryEntry,
    OperationId, RepositoryName, canonical_json,
};

const TRUNCATED: &[u8] =
    include_bytes!("../../../fixtures/metadata_adversarial/recovery/truncated-object.json");

#[derive(Clone, Copy, Debug)]
enum Target {
    Snapshot,
    Namespace,
    Events,
    Operations,
}

#[derive(Clone, Copy, Debug)]
enum Damage {
    Missing,
    Digest,
    Truncated,
}

struct Fixture {
    temp: tempfile::TempDir,
    root: PathBuf,
    config: Config,
    kind: &'static str,
    hash: String,
    original: Vec<u8>,
}

fn fixture(target: Target) -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let config = Config::development([
        temp.path().join("a"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    let mut engine = Engine::initialize(&root, config.clone()).unwrap();
    let repository = engine
        .create_repository(
            &RepositoryName::parse("adversarial").unwrap(),
            "P01",
            1,
            "admin",
            "op_00000000000000000000000000000001".parse().unwrap(),
        )
        .unwrap();
    let operation_id: OperationId = "op_00000000000000000000000000000002".parse().unwrap();
    engine
        .file_namespace(
            &repository.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "retained", None).unwrap(),
            "admin",
            operation_id,
        )
        .unwrap();
    assert_eq!(engine.list_tree(&repository.main_branch_id).unwrap().len(), 1);
    let snapshot_hash = engine.snapshot.as_ref().unwrap();
    let snapshot: Snapshot = load_object(&config.disks[0], "snapshot", snapshot_hash).unwrap();
    let (kind, hash) = match target {
        Target::Snapshot => ("snapshot", snapshot_hash.clone()),
        Target::Namespace => {
            let head = &engine.state().branches[&repository.main_branch_id].head_commit_id;
            let reference = &engine.state().commits[head].namespace.root;
            (reference.kind.as_str(), reference.hash.clone())
        }
        Target::Events | Target::Operations => {
            let reference = match target {
                Target::Events => &snapshot.events,
                Target::Operations => &snapshot.maps["operation_log"],
                Target::Snapshot | Target::Namespace => unreachable!(),
            };
            let directory = crate::metadata_map::read_map(reference, |kind, hash| {
                load_authenticated_object(&config.disks[0], kind, hash)
            })
            .unwrap();
            let value = directory.values().last().unwrap().clone();
            if matches!(target, Target::Events) {
                let entry: EventDirectoryEntry = json_codec::from_value(value).unwrap();
                (storage_contract::EVENT_SEGMENT_KIND, entry.hash)
            } else {
                let entry: OperationDirectoryEntry = json_codec::from_value(value).unwrap();
                (storage_contract::OPERATION_SEGMENT_KIND, entry.hash)
            }
        }
    };
    let original = std::fs::read(object_path(&config.disks[0], kind, &hash)).unwrap();
    drop(engine);
    // Establish that the unchanged persisted graph reopens successfully. This
    // also creates a legitimate checkpoint: damage must invalidate its coverage.
    let reopened = Engine::open(&root).unwrap();
    assert_eq!(
        reopened.list_tree(&repository.main_branch_id).unwrap().len(),
        1
    );
    drop(reopened);
    Fixture {
        temp,
        root,
        config,
        kind,
        hash,
        original,
    }
}

fn corrupt_digest(kind: &str, bytes: &[u8]) -> Vec<u8> {
    let mut value: json_codec::Value = storage_contract::decode_strict(bytes).unwrap();
    match kind {
        "snapshot" => value["sequence"] = u64::MAX.to_string().into(),
        "namespace-page" => value["entries"][0]["entry_id"] = "changed-identity".into(),
        "event-segment" => value["events"][0]["kind"] = "changed-kind".into(),
        "operation-segment" => value["records"][0]["principal"] = "changed-principal".into(),
        other => panic!("unexpected fixture object kind: {other}"),
    }
    canonical_json(&value).unwrap()
}

fn damage(fixture: &Fixture, damage: Damage) {
    for (member, disk) in fixture.config.disks.iter().enumerate() {
        let path = object_path(disk, fixture.kind, &fixture.hash);
        match damage {
            Damage::Missing => {
                // Withhold the whole object outside the pool; keep the original
                // bytes available to the test instead of deleting evidence.
                let held = fixture.temp.path().join(format!("withheld-{member}.json"));
                std::fs::rename(path, held).unwrap();
            }
            Damage::Digest => {
                let changed = corrupt_digest(fixture.kind, &std::fs::read(&path).unwrap());
                std::fs::write(path, changed).unwrap();
            }
            Damage::Truncated => std::fs::write(path, TRUNCATED).unwrap(),
        }
    }
}

fn collect_authority(directory: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            collect_authority(&path, out);
        } else {
            assert!(kind.is_file(), "fixture has an unexpected file type: {path:?}");
            let name = entry.file_name();
            // Checkpoints and writer locks are acceleration/coordination files,
            // not original publication authority.
            if name != "replay-checkpoint.json" && !name.to_string_lossy().ends_with(".lock") {
                out.insert(path.clone(), std::fs::read(path).unwrap());
            }
        }
    }
}

fn authority(fixture: &Fixture) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let config = fixture.root.join("config.json");
    files.insert(config.clone(), std::fs::read(config).unwrap());
    for disk in &fixture.config.disks {
        let seal = disk.root.join("disk.json");
        files.insert(seal.clone(), std::fs::read(seal).unwrap());
        collect_authority(&disk.root.join("metadata"), &mut files);
    }
    files
}

fn assert_error(error: &Error, damage: Damage) {
    match damage {
        Damage::Missing | Damage::Digest => {
            assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
            let reason = match damage {
                Damage::Missing => "immutable object is missing",
                Damage::Digest => "immutable object digest differs",
                Damage::Truncated => unreachable!(),
            };
            assert_eq!(error.message(), reason);
        }
        Damage::Truncated => {
            assert_eq!(error.code(), ErrorCode::InvalidJson);
            assert!(matches!(error, Error::Contract(storage_contract::Error::Json { .. })));
            assert!(std::error::Error::source(error).is_some());
        }
    }
}

fn rejects_and_preserves(target: Target) {
    for fault in [Damage::Missing, Damage::Digest, Damage::Truncated] {
        let fixture = fixture(target);
        damage(&fixture, fault);
        let original_authority = authority(&fixture);
        let error = Engine::open(&fixture.root)
            .err()
            .unwrap_or_else(|| panic!("{target:?}/{fault:?}: invalid published closure opened"));
        assert_error(&error, fault);
        assert_eq!(
            authority(&fixture),
            original_authority,
            "{target:?}/{fault:?}: recovery changed original authority"
        );
        for (member, disk) in fixture.config.disks.iter().enumerate() {
            let path = object_path(disk, fixture.kind, &fixture.hash);
            if matches!(fault, Damage::Missing) {
                assert_eq!(std::fs::read(&path).unwrap_err().kind(), std::io::ErrorKind::NotFound);
                assert_eq!(
                    std::fs::read(fixture.temp.path().join(format!("withheld-{member}.json")))
                        .unwrap(),
                    fixture.original,
                );
            }
        }
    }
}

#[test]
fn missing_or_corrupt_namespace_replicas_never_reset_a_published_pool() {
    rejects_and_preserves(Target::Namespace);
}

#[test]
fn missing_or_corrupt_snapshot_replicas_preserve_the_original_authority() {
    rejects_and_preserves(Target::Snapshot);
}

#[test]
fn missing_or_corrupt_event_segments_preserve_the_original_authority() {
    rejects_and_preserves(Target::Events);
}

#[test]
fn missing_or_corrupt_operation_segments_preserve_the_original_authority() {
    rejects_and_preserves(Target::Operations);
}
