use super::*;
use std::{collections::BTreeMap, fs};

fn fixture(count: usize) -> (tempfile::TempDir, Config, Engine) {
    let temporary = tempfile::tempdir().unwrap();
    let config =
        Config::development((0..count).map(|n| temporary.path().join(format!("disk-{n}"))))
            .unwrap();
    let mut engine = Engine::initialize(&temporary.path().join("system"), config.clone()).unwrap();
    engine
        .create_repository(
            &RepositoryName::parse("preserved").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    (temporary, config, engine)
}

fn disk_bytes(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, path: &Path, output: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if entry.file_type().unwrap().is_dir() {
                output.insert(relative, None);
                visit(root, &path, output);
            } else {
                output.insert(relative, Some(fs::read(path).unwrap()));
            }
        }
    }
    let mut output = BTreeMap::new();
    visit(root, root, &mut output);
    output
}

#[test]
fn membership_anchor_restore_uses_original_quorum_and_preserves_rejected_disk() {
    for damage in ["missing", "malformed", "different-membership"] {
        let (temporary, config, engine) = fixture(3);
        let expected = canonical_json(engine.state()).unwrap();
        drop(engine);
        let rejected = &config.disks[2];
        let anchor = rejected.root.join("metadata/pool-membership/config.json");
        match damage {
            "missing" => fs::remove_file(&anchor).unwrap(),
            "malformed" => fs::write(&anchor, b"{").unwrap(),
            _ => {
                let mut altered = config.clone();
                altered.disks[1].metadata_member = false;
                write_json(&anchor, &altered, true).unwrap();
            }
        }
        let before = disk_bytes(&rejected.root);
        let paths = config
            .disks
            .iter()
            .map(|disk| disk.root.clone())
            .collect::<Vec<_>>();
        let mut restored =
            Engine::restore_from_disks(&temporary.path().join("replacement"), &paths).unwrap();
        assert_eq!(canonical_json(restored.state()).unwrap(), expected);
        assert_eq!(restored.config.metadata_quorum(), 2);
        assert_eq!(restored.config.metadata_members().len(), 3);
        assert_eq!(restored.journals.len(), 2);
        assert!(restored.is_read_only());
        assert!(restored.disk_resources.available(rejected).is_none());
        let warnings = restored.take_recovery_warnings();
        assert_eq!(warnings.seals.len(), 1);
        assert_eq!(
            warnings.seals[0].context(),
            &crate::RecoverySealContext::MembershipAnchor {
                disk_id: rejected.disk_id.clone(),
                slot_id: rejected.slot_id.clone()
            }
        );
        assert_eq!(disk_bytes(&rejected.root), before, "{damage}");
    }
}

#[test]
fn membership_anchor_rejection_cannot_reduce_two_member_quorum() {
    let (temporary, config, engine) = fixture(2);
    drop(engine);
    fs::remove_file(
        config.disks[1]
            .root
            .join("metadata/pool-membership/config.json"),
    )
    .unwrap();
    let before = config
        .disks
        .iter()
        .map(|disk| disk_bytes(&disk.root))
        .collect::<Vec<_>>();
    let error = Engine::open(&temporary.path().join("system"))
        .err()
        .unwrap();
    assert_eq!(error.code(), ErrorCode::MetadataQuorumUnavailable);
    assert!(std::error::Error::source(&error).is_some());
    let paths = config
        .disks
        .iter()
        .map(|disk| disk.root.clone())
        .collect::<Vec<_>>();
    let replacement = temporary.path().join("replacement");
    assert_eq!(
        Engine::restore_from_disks(&replacement, &paths)
            .err()
            .unwrap()
            .code(),
        ErrorCode::ConfigConflict
    );
    assert!(!replacement.exists());
    for (disk, before) in config.disks.iter().zip(before) {
        assert_eq!(disk_bytes(&disk.root), before);
    }
}

#[test]
fn membership_anchor_restore_keeps_duplicate_inputs_visible() {
    let (temporary, config, engine) = fixture(3);
    drop(engine);
    let rejected = &config.disks[2];
    fs::remove_file(rejected.root.join("metadata/pool-membership/config.json")).unwrap();
    let mut paths = config
        .disks
        .iter()
        .map(|disk| disk.root.clone())
        .collect::<Vec<_>>();
    paths.push(rejected.root.clone());
    let replacement = temporary.path().join("replacement");
    let error = Engine::restore_from_disks(&replacement, &paths)
        .err()
        .unwrap();
    assert_eq!(error.code(), ErrorCode::ConfigConflict);
    assert!(error.message().contains("duplicate disk id"));
    assert!(!replacement.exists());
}

#[test]
fn membership_anchor_restore_cannot_hide_foreign_input_behind_missing_anchor() {
    let (temporary, config, engine) = fixture(3);
    let (foreign_temporary, foreign_config, foreign) = fixture(1);
    drop(engine);
    drop(foreign);
    let foreign_disk = &foreign_config.disks[0];
    fs::remove_file(
        foreign_disk
            .root
            .join("metadata/pool-membership/config.json"),
    )
    .unwrap();
    let before = disk_bytes(&foreign_disk.root);
    let paths = vec![
        config.disks[0].root.clone(),
        config.disks[1].root.clone(),
        foreign_disk.root.clone(),
    ];
    let replacement = temporary.path().join("replacement");
    assert_eq!(
        Engine::restore_from_disks(&replacement, &paths)
            .err()
            .unwrap()
            .code(),
        ErrorCode::ConfigConflict
    );
    assert!(!replacement.exists());
    assert_eq!(disk_bytes(&foreign_disk.root), before);
    drop(foreign_temporary);
}

#[test]
fn membership_anchor_restore_rejects_competing_self_declared_quorums() {
    let (temporary, config, engine) = fixture(3);
    drop(engine);
    let mut smaller = config.clone();
    smaller.disks = vec![config.disks[2].clone()];
    smaller.disks[0].level = 1;
    smaller.disks[0].upstream_slot_id = None;
    ValidatedConfig::try_from(smaller.clone()).unwrap();
    write_json(
        &config.disks[2]
            .root
            .join("metadata/pool-membership/config.json"),
        &smaller,
        true,
    )
    .unwrap();
    let paths = config
        .disks
        .iter()
        .map(|disk| disk.root.clone())
        .collect::<Vec<_>>();
    let before = config
        .disks
        .iter()
        .map(|disk| disk_bytes(&disk.root))
        .collect::<Vec<_>>();
    let replacement = temporary.path().join("replacement");
    let error = Engine::restore_from_disks(&replacement, &paths)
        .err()
        .unwrap();
    assert_eq!(error.code(), ErrorCode::ConfigConflict);
    assert!(error.message().contains("more than one pool configuration"));
    assert!(!replacement.exists());
    for (disk, before) in config.disks.iter().zip(before) {
        assert_eq!(disk_bytes(&disk.root), before);
    }
}

#[test]
fn membership_anchor_isolation_does_not_complete_unpublished_candidate() {
    let (temporary, config, mut engine) = fixture(3);
    let expected = canonical_json(engine.state()).unwrap();
    engine.inject_fault_once("after_accept_1");
    assert!(
        engine
            .create_repository(
                &RepositoryName::parse("pending").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap()
            )
            .is_err()
    );
    drop(engine);
    let rejected = &config.disks[2];
    fs::write(
        rejected.root.join("metadata/pool-membership/config.json"),
        b"{",
    )
    .unwrap();
    let before = config
        .disks
        .iter()
        .map(|disk| disk_bytes(&disk.root))
        .collect::<Vec<_>>();
    for preserve_pending in [false, true] {
        let recovered = if preserve_pending {
            Engine::open_for_recovery(&temporary.path().join("system"))
        } else {
            Engine::open(&temporary.path().join("system"))
        }
        .unwrap();
        assert!(recovered.is_read_only());
        assert_eq!(canonical_json(recovered.state()).unwrap(), expected);
        assert_eq!(recovered.journals.len(), 2);
        for (disk, before) in config.disks.iter().zip(&before) {
            assert_eq!(disk_bytes(&disk.root), *before);
        }
    }
}

#[test]
fn stricter_genesis_budget_keeps_the_saved_anchors_and_local_limits() {
    let (temporary, config, engine) = fixture(2);
    drop(engine);
    let system = temporary.path().join("system");
    let anchors = config
        .disks
        .iter()
        .map(|disk| disk.root.join("metadata/pool-membership/config.json"))
        .map(|path| {
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect::<Vec<_>>();
    let mut local = config.clone();
    local.max_retained_workspaces -= 1;
    local.max_retained_workspaces_per_principal -= 1;
    local.max_retained_workspace_metadata_bytes -= 1;
    local.max_retained_workspace_metadata_bytes_per_principal -= 1;
    write_json(&system.join("config.json"), &local, true).unwrap();
    let projected = fs::read(system.join("config.json")).unwrap();
    let local = ValidatedConfig::try_from(local).unwrap();
    let engine = Engine::open(&system).unwrap();
    assert_eq!(
        engine.config.effective_workspace_limits(),
        local.effective_workspace_limits(),
    );
    assert_eq!(engine.config.metadata_members().len(), 2);
    assert_eq!(engine.config.metadata_quorum(), 2);
    assert_eq!(fs::read(system.join("config.json")).unwrap(), projected);
    for (path, bytes) in anchors {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn stricter_local_budget_cannot_merge_different_saved_genesis_anchors() {
    let (temporary, config, engine) = fixture(2);
    drop(engine);
    let system = temporary.path().join("system");
    let mut changed = config.clone();
    changed.max_retained_workspaces -= 1;
    write_json(&system.join("config.json"), &changed, true).unwrap();
    write_json(
        &config.disks[0]
            .root
            .join("metadata/pool-membership/config.json"),
        &changed,
        true,
    )
    .unwrap();
    let before = disk_bytes(temporary.path());
    let error = Engine::open(&system).err().unwrap();
    assert_eq!(error.code(), ErrorCode::MetadataQuorumUnavailable);
    let Error::Context { source, .. } = &error else {
        panic!("fixed quorum failure lost the rejected anchor");
    };
    assert_eq!(source.code(), ErrorCode::ConfigConflict);
    assert_eq!(disk_bytes(temporary.path()), before);
}

#[test]
fn system_projection_cannot_raise_the_saved_genesis_budget() {
    let (temporary, mut config, engine) = fixture(1);
    drop(engine);
    let system = temporary.path().join("system");
    config.max_retained_workspaces += 1;
    write_json(&system.join("config.json"), &config, true).unwrap();
    let before = disk_bytes(temporary.path());
    let error = Engine::open(&system).err().unwrap();
    assert_eq!(error.code(), ErrorCode::MetadataQuorumUnavailable);
    let Error::Context { source, .. } = &error else {
        panic!("fixed quorum failure lost the rejected projection");
    };
    assert_eq!(source.code(), ErrorCode::ConfigConflict);
    assert_eq!(disk_bytes(temporary.path()), before);
}
