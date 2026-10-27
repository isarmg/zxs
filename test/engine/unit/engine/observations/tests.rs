use super::*;

#[test]
fn missing_member_with_empty_mountpoint_never_reports_host_capacity() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let config = Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ]);
    let engine = Engine::initialize(&root.path().join("system"), config.clone()).unwrap();
    let plan = engine.prepare_disk_observation();
    let member = &config.disks[0];
    std::fs::rename(&member.root, root.path().join("removed-member")).unwrap();
    std::fs::create_dir(&member.root).unwrap();
    std::fs::set_permissions(&member.root, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(rustix::fs::statvfs(&member.root).unwrap().f_blocks > 0);
    let observed = plan
        .sample()
        .into_iter()
        .find(|disk| disk.status.slot_id == member.slot_id)
        .unwrap();
    assert!(!observed.status.online);
    assert_eq!(observed.status.available_bytes, 0);
    assert!(observed.capacity.is_none());
    assert!(engine.disk_capacity(&member.slot_id).is_err());
}

#[test]
fn observation_owns_snapshot_and_probes_without_engine() {
    let root = tempfile::tempdir().unwrap();
    let config = Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ]);
    let engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    let plan = engine.prepare_disk_observation();
    let expected = engine.disk_status().unwrap();
    assert!(
        engine
            .disk_capacity(&expected[0].slot_id.to_ascii_lowercase())
            .unwrap()
            .0
            > 0
    );
    assert_eq!(
        engine.disk_capacity("missing").unwrap_err().code(),
        storage_contract::ErrorCode::SlotNotFound
    );
    drop(engine);
    let observed = plan.sample();
    assert_eq!(observed.len(), expected.len());
    for (observed, expected) in observed.iter().zip(&expected) {
        assert_eq!(observed.status.slot_id, expected.slot_id);
        assert_eq!(observed.status.online, expected.online);
        assert_eq!(observed.status.used_bytes, expected.used_bytes);
        let capacity = observed.capacity.unwrap();
        assert!(capacity.0 > 0);
        assert_eq!(observed.status.available_bytes, capacity.1);
    }
}

#[test]
fn accounting_snapshot_preserves_purge_and_duplicate_route_semantics() {
    let root = tempfile::tempdir().unwrap();
    let config = Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ]);
    let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Accounting").unwrap(),
            "P01",
            3,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    let source = root.path().join("source");
    std::fs::write(&source, b"snapshot bytes").unwrap();
    engine
        .stage_file(&workspace.workspace_id, "sample.txt", &source)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap();
    for placement in std::sync::Arc::make_mut(&mut engine.state)
        .placements
        .values_mut()
    {
        placement.route_slot_ids.push("P01".into());
    }
    let plan = engine.prepare_disk_observation();
    for disk in plan.status().unwrap() {
        assert_eq!(disk.used_bytes, engine.used_bytes(&disk.slot_id));
    }
    let state = std::sync::Arc::make_mut(&mut engine.state);
    for version in state.versions.keys() {
        state
            .content_states
            .insert(version.clone(), storage_contract::ContentState::Purged);
    }
    assert!(
        engine
            .prepare_disk_observation()
            .status()
            .unwrap()
            .iter()
            .all(|disk| disk.used_bytes == 0)
    );
    assert!(
        plan.status()
            .unwrap()
            .iter()
            .any(|disk| disk.used_bytes > 0)
    );
}

#[test]
#[should_panic(expected = "invariant: every placement references an existing version")]
fn observation_exposes_dangling_internal_placement() {
    let root = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([
            root.path().join("d1"),
            root.path().join("d2"),
            root.path().join("d3"),
        ]),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Accounting").unwrap(),
            "P01",
            3,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "body", b"bytes")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap();
    let id = engine.state.versions.keys().next().unwrap().clone();
    std::sync::Arc::make_mut(&mut engine.state)
        .versions
        .remove(&id);
    engine.prepare_disk_observation();
}

#[test]
fn metadata_probe_observes_current_identity_without_engine_or_capacity_accounting() {
    let root = tempfile::tempdir().unwrap();
    let config = Config::development([root.path().join("member")]);
    let engine = Engine::initialize(&root.path().join("system"), config.clone()).unwrap();
    let probe = engine.prepare_metadata_member_probe();
    drop(engine);
    assert_eq!(probe.member_count(), 1);
    probe.verify().unwrap();
    let member = &config.disks[0].root;
    let removed = root.path().join("removed");
    std::fs::rename(member, &removed).unwrap();
    let error = probe.verify().unwrap_err();
    assert!(std::error::Error::source(&error).is_some());
    std::fs::create_dir(member).unwrap();
    assert!(probe.verify().is_err());
    // Even a copied, otherwise valid seal must not bless a replacement root.
    std::fs::copy(removed.join("disk.json"), member.join("disk.json")).unwrap();
    assert_eq!(
        probe.verify().unwrap_err().code(),
        ErrorCode::DiskIdentityMismatch
    );
    std::fs::remove_file(member.join("disk.json")).unwrap();
    std::fs::remove_dir(member).unwrap();
    std::fs::rename(&removed, member).unwrap();
    probe.verify().unwrap();
    std::fs::write(member.join("disk.json"), b"broken seal").unwrap();
    assert!(probe.verify().is_err());
}

#[test]
fn metadata_probe_ignores_nonmembers_and_unrelated_historical_placements() {
    let root = tempfile::tempdir().unwrap();
    let mut config = Config::development([root.path().join("member"), root.path().join("backup")]);
    config.disks[1].metadata_member = false;
    let mut engine = Engine::initialize(&root.path().join("system"), config.clone()).unwrap();
    // Deliberately dangling internal sentinels: observing identity must not touch
    // placement/version accounting. This is not a valid large-pool benchmark.
    let state = std::sync::Arc::make_mut(&mut engine.state);
    for n in 0..10_000 {
        let version_id = format!("unrelated-{n}");
        state.placements.insert(
            version_id.clone(),
            storage_contract::Placement {
                version_id,
                primary_slot_id: "P01".into(),
                target_copies: 1,
                route_slot_ids: vec!["P01".into()],
                topology_generation: 1,
                binding_generations: BTreeMap::new(),
            },
        );
    }
    let probe = engine.prepare_metadata_member_probe();
    assert_eq!(probe.member_count(), 1);
    std::fs::rename(&config.disks[1].root, root.path().join("removed-backup")).unwrap();
    probe.verify().unwrap();
}
