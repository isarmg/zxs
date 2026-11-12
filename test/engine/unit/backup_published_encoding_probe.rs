//! Real published-scope encoding checks on the existing serving fixture.
use super::*;
use std::{fs, path::Path};

struct PublishedEncodingObservation {
    reports: Vec<BackupPublishedScopeTargetReport>,
    sqlite: BTreeMap<PathBuf, Vec<u8>>,
}

fn observe_published_encodings(
    prepared: &PreparedBackupRegistry,
    registry: &published_registry::Plan,
    stop: &AtomicBool,
) -> Result<PublishedEncodingObservation> {
    let mut observed = PublishedEncodingObservation {
        reports: Vec::new(),
        sqlite: BTreeMap::new(),
    };
    let config = registry.effective_config();
    for (index, target) in prepared.targets.iter().enumerate() {
        let scopes = Scopes::capture(
            prepared, registry, index, stop,
        )?;
        observed.reports.push(scopes.report(&prepared.projection.targets[index].node.identity));
        assert!(observed.sqlite.insert(
            target.root.path().join("system/index.sqlite"), scopes.system().to_vec(),
        ).is_none());
        for disk in &config.disks {
            let identity = crate::DiskRegistrationIdentity {
                pool_id: config.pool_id.clone(),
                disk_id: disk.disk_id.clone(),
                slot_id: disk.slot_id.clone(),
                binding_generation: disk.binding_generation,
            };
            for (name, bytes) in scopes.disk(&identity)? {
                let path = target.root.path().join("disks").join(&disk.disk_id).join(name);
                assert!(observed.sqlite.insert(path, bytes.to_vec()).is_none());
            }
        }
        let report = observed.reports.last().unwrap();
        assert_eq!(report.configured_disks, config.disks.len());
        assert_eq!(report.databases, 1 + 2 * config.disks.len());
    }
    Ok(observed)
}

// Directory names and file bytes only; this is not a permissions/mtime proof.
fn published_fixture_tree(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let relative = path.strip_prefix(root).unwrap().to_path_buf();
        if metadata.is_dir() {
            assert!(entries.insert(relative, None).is_none());
            for entry in fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), entries);
            }
        } else {
            assert!(metadata.is_file());
            assert!(entries.insert(relative, Some(fs::read(path).unwrap())).is_none());
        }
    }
    let mut entries = BTreeMap::new();
    visit(root, root, &mut entries);
    entries
}

fn published_encoding_error(error: &crate::Error) -> (crate::ErrorCode, Vec<String>) {
    let mut chain = Vec::new();
    let mut next: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(error) = next {
        chain.push(error.to_string());
        next = error.source();
    }
    (error.code(), chain)
}

pub(in crate::engine::backup_preparation::native_control::databases::scoped::cutover::registration)
fn verify_published_encoding_reuse(
    root: &Path,
    prepared: &mut PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<()> {
    // Reuse this already installed real fixture, not a synthesized registry or
    // another worker. Preserve the original memo for all existing timing calls.
    let retained = prepared.scope_encoding.take();
    let before = published_fixture_tree(root);
    let registry = published_registry::Plan::capture(prepared, decision, expected, stop)?;
    let baseline = observe_published_encodings(prepared, &registry, stop)?;
    assert_eq!(baseline.reports.len(), 2);
    assert_eq!(published_fixture_tree(root), before);
    prepared.scope_encoding = Some(encoding_memo::ScopeEncodingMemo::new());
    for _ in 0..2 {
        let observed = observe_published_encodings(prepared, &registry, stop)?;
        assert_eq!(observed.reports, baseline.reports);
        assert_eq!(observed.sqlite, baseline.sqlite);
        assert_eq!(published_fixture_tree(root), before);
    }
    // Registry/state reads above remain real; only the original archive object
    // is damaged here, after a successful capture populated the shared memo.
    for index in 0..prepared.targets.len() {
        let path = prepared.targets[index].root.path().join("native-control/objects/snapshot")
            .join(format!("{}.json", prepared.task.selection().jobs()[index].snapshot));
        let original = fs::read(&path).unwrap();
        for bad in [b"{}".as_slice(), br#"{"a":1,"a":1}"#.as_slice()] {
            fs::write(&path, bad).unwrap();
            let damaged = published_fixture_tree(root);
            let warm = observe_published_encodings(prepared, &registry, stop).err().unwrap();
            assert_eq!(published_fixture_tree(root), damaged);
            let memo = prepared.scope_encoding.take();
            let plain = observe_published_encodings(prepared, &registry, stop).err().unwrap();
            assert_eq!(published_fixture_tree(root), damaged);
            prepared.scope_encoding = memo;
            assert_eq!(published_encoding_error(&warm), published_encoding_error(&plain));
            assert_eq!(published_fixture_tree(root), damaged);
        }
        fs::write(&path, original).unwrap();
    }
    let observed = observe_published_encodings(prepared, &registry, stop)?;
    assert_eq!(observed.reports, baseline.reports);
    assert_eq!(observed.sqlite, baseline.sqlite);
    assert_eq!(published_fixture_tree(root), before);
    prepared.scope_encoding = retained;
    Ok(())
}
