//! Codex audit isolated into bounded public-API cases, not fabricated offline State.
use super::*;

fn rename(fixture: &mut Fixture, from: &str, to: &str, id: OperationId) -> Result<OperationResult> {
    fixture.engine.file_namespace(
        &fixture.repository.repository_id,
        NamespaceOperation::parse(NamespaceOperationKind::Rename, from, Some(to)).unwrap(),
        "admin",
        id,
    )
}

fn fixture() -> Fixture {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::Builder::new()
        .permissions(PermissionsExt::from_mode(0o700))
        .tempdir()
        .unwrap();
    let names = [
        "disk-0", "disk-1", "disk-2", "p-two", "b-two", "c-two", "spare",
    ];
    let mut config = Config::development(names.iter().map(|name| temp.path().join(name))).unwrap();
    for (index, level, upstream) in [
        (3, 1, None),
        (4, 2, Some("D04")),
        (5, 3, Some("D05")),
        (6, 1, None),
    ] {
        config.disks[index].level = level;
        config.disks[index].upstream_slot_id = upstream.map(str::to_owned);
        config.disks[index].metadata_member = false;
    }
    let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
    let repository = engine
        .create_repository(
            &RepositoryName::parse("Repository").unwrap(),
            "P01",
            1,
            "admin",
            operation(),
        )
        .unwrap();
    let mut fixture = Fixture {
        temp,
        config,
        engine,
        repository,
    };
    fixture.exit(6, MembershipExitAction::Deregister);
    fixture
        .engine
        .update_placement_rule(PlacementRuleUpdate {
            rule_id: &storage_engine::new_id("rule").unwrap(),
            repository_id: &fixture.repository.repository_id,
            path_prefix: "root/second",
            primary_slot_id: "D04",
            expected_generation: 0,
            principal: "admin",
            operation_id: operation(),
        })
        .unwrap();
    fixture
}

#[test]
fn recursive_follow_inherited_chain_has_only_its_own_path_bindings() {
    let mut fixture = fixture();
    fixture.save("root/first/seed", b"seed");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let configured = fixture.configure("root", &capture, 0, operation()).unwrap();
    let inherited = fixture.save("root/first/inherited", b"inherited route only");
    assert_eq!(
        fixture.engine.state().file_backup_paths[&inherited.file_id].len(),
        2
    );
    let moved = rename(
        &mut fixture,
        "root/first/inherited",
        "root/first/moved",
        operation(),
    );
    eprintln!(
        "CHAIN AUDIT: inherited all-online rename = {:?}",
        moved.as_ref().map(|r| &r.kind)
    );
    moved.expect("an inherited file need not invent unused chain path keys");
    assert_eq!(
        fixture.engine.state().file_backup_paths[&inherited.file_id].len(),
        2
    );
    fixture.assert_body_paths(&inherited, "root/first/moved", b"inherited route only");
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    fixture.assert_body_paths(&inherited, "root/first/moved", b"inherited route only");
    assert_eq!(
        fixture.engine.operation(configured.operation_id).unwrap(),
        configured
    );
}

#[test]
fn recursive_follow_keeps_unrelated_stopped_chains_independent() {
    let mut fixture = fixture();
    let first = fixture.save("root/first/item", b"healthy first chain");
    let second = fixture.save("root/second/item", b"other chain");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let configured = fixture.configure("root", &capture, 0, operation()).unwrap();
    assert_eq!(
        fixture.engine.state().placements[&first.version_id].route_slot_ids,
        ["P01", "B01", "C01"]
    );
    assert_eq!(
        fixture.engine.state().placements[&second.version_id].route_slot_ids,
        ["D04", "D05", "D06"]
    );
    eprintln!("CHAIN AUDIT: begin unrelated non-metadata D05 stop");
    fixture
        .engine
        .begin_disk_exit(
            &fixture.config.disks[4].disk_id,
            MembershipExitAction::PlannedOffline,
            "admin",
            operation(),
        )
        .unwrap();
    eprintln!("CHAIN AUDIT: fence written; build archive");
    let archive = fixture
        .engine
        .build_managed_membership_control_archive()
        .unwrap();
    eprintln!("CHAIN AUDIT: archive built; enroll");
    fixture
        .engine
        .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
        .unwrap();
    eprintln!("CHAIN AUDIT: enrolled; joint decision");
    fixture.engine.resume_membership_joint_decision().unwrap();
    eprintln!("CHAIN AUDIT: joint decision; activate exit");
    fixture.engine.activate_pending_disk_exit().unwrap();
    eprintln!("CHAIN AUDIT: actual stopped registry installed");
    let stopped = files(&fixture.config.disks[4].root);
    let before = fixture.engine.state().clone();
    let required = rename(
        &mut fixture,
        "root/second/item",
        "root/second/no-move",
        operation(),
    )
    .unwrap_err();
    assert!(matches!(
        required.code(),
        ErrorCode::PlacementInvalid | ErrorCode::SlotNotFound
    ));
    assert_eq!(fixture.engine.state().sequence, before.sequence);
    assert_eq!(
        fixture.engine.state().file_backup_paths,
        before.file_backup_paths
    );
    let moved = rename(
        &mut fixture,
        "root/first/item",
        "root/first/moved",
        operation(),
    );
    eprintln!(
        "CHAIN AUDIT: healthy chain rename with unrelated stop = {:?}",
        moved.as_ref().map(|r| &r.kind)
    );
    assert_eq!(files(&fixture.config.disks[4].root), stopped);
    let moved = moved.expect("an unrelated stopped data chain must not freeze a healthy file");
    fixture.assert_body_paths(&first, "root/first/moved", b"healthy first chain");
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    assert_eq!(
        fixture.engine.operation(configured.operation_id).unwrap(),
        configured
    );
    assert_eq!(
        rename(
            &mut fixture,
            "root/first/item",
            "root/first/moved",
            moved.operation_id
        )
        .unwrap(),
        moved
    );
    fixture.assert_body_paths(&first, "root/first/moved", b"healthy first chain");
    assert_eq!(files(&fixture.config.disks[4].root), stopped);
}
