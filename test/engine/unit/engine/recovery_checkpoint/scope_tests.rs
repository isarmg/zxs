use super::*;
use std::{fs, sync::Arc};

fn journals(engine: &Engine) -> BTreeMap<String, Vec<(Record, String)>> {
    engine
        .members()
        .iter()
        .map(|disk| {
            let held = engine.disk_resources.get(disk).unwrap();
            (
                disk.disk_id.clone(),
                scan_configured_at(&held.root, disk, &engine.config)
                    .unwrap()
                    .records,
            )
        })
        .collect()
}

fn prefixes(engine: &Engine) -> BTreeMap<String, BTreeSet<String>> {
    journals(engine)
        .into_iter()
        .map(|(disk, records)| (disk, records.into_iter().map(|(_, hash)| hash).collect()))
        .collect()
}

fn write_checkpoint(engine: &Engine, checkpoint: Checkpoint) {
    let envelope = Envelope {
        hash: object_hash("replay-checkpoint", &checkpoint).unwrap(),
        checkpoint,
    };
    for disk in engine.members() {
        write_json(&path(&disk), &envelope, true).unwrap();
    }
}

#[test]
fn checkpoint_scope_rejects_rehashed_authority_io_voters_and_foreign_tips() {
    let (_temp, root, config) = setup();
    let engine = reopen(&root).0;
    let envelope: Envelope = read_json(&path(&config.disks[0])).unwrap();
    let expected = envelope.checkpoint;
    let state = canonical_json(engine.state()).unwrap();
    let original_journals = journals(&engine);
    let mut prefixes = prefixes(&engine);
    assert_eq!(
        load(&engine, &prefixes, &Records::new()).unwrap(),
        Some(expected.clone())
    );

    let mut changed = Vec::new();
    let mut authority = expected.clone();
    authority.context.authority = CheckpointAuthority::Generation {
        hash: "a".repeat(64),
    };
    changed.push(authority);
    let mut scope = expected.clone();
    scope.context.io_disk_ids.remove(&config.disks[0].disk_id);
    changed.push(scope);
    let mut voters = expected.clone();
    voters
        .context
        .metadata_disk_ids
        .remove(&config.disks[0].disk_id);
    changed.push(voters);
    let mut tips = expected.clone();
    let tip = tips.tips.remove(&config.disks[0].disk_id).unwrap();
    let foreign = new_id("disk").unwrap();
    tips.tips.insert(foreign.clone(), tip.clone());
    prefixes.insert(foreign, BTreeSet::from([tip]));
    changed.push(tips);
    let mut coverage = expected.clone();
    let entries = coverage.coverage.remove(&config.disks[0].disk_id).unwrap();
    coverage.coverage.insert(new_id("disk").unwrap(), entries);
    changed.push(coverage);

    for checkpoint in changed {
        // Equal, self-consistent files on every voter still cannot redefine
        // the independently admitted generation, I/O scope or exact voters.
        write_checkpoint(&engine, checkpoint);
        assert!(load(&engine, &prefixes, &Records::new()).unwrap().is_none());
        assert_eq!(canonical_json(engine.state()).unwrap(), state);
        assert_eq!(journals(&engine), original_journals);
    }
}

#[test]
fn checkpoint_scope_restricted_io_is_not_a_smaller_metadata_membership() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let mut config =
        Config::development((0..3).map(|index| temp.path().join(format!("disk-{index}")))).unwrap();
    config.disks[2].metadata_member = false;
    let mut engine = Engine::initialize(&root, config.clone()).unwrap();
    engine
        .create_repository(
            &RepositoryName::parse("scope").unwrap(),
            "P01",
            3,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    drop(engine);
    let mut engine = reopen(&root).0;
    let scope = CheckpointScope::admit(&engine).unwrap().unwrap();
    assert_eq!(scope.context.io_disk_ids.len(), 3);
    assert_eq!(scope.context.metadata_disk_ids.len(), 2);
    let members = scope.members.clone();
    drop(scope);
    let quorum = engine.config.metadata_quorum();
    let old_state = canonical_json(engine.state()).unwrap();
    let target = &config.disks[2];
    let held = Arc::clone(engine.disk_resources.get(target).unwrap());
    let seal = fs::read(target.root.join("disk.json")).unwrap();
    drop(held.root.io_binding().unwrap().begin_quiesce().unwrap());
    held.root.io_binding().unwrap().closed_proof().unwrap();
    let allowed = config.disks[..2]
        .iter()
        .map(|disk| disk.disk_id.clone())
        .collect();
    engine.config = engine.config.clone().restrict_io_disks(allowed).unwrap();
    assert_eq!(
        canonical_json(&engine.members()).unwrap(),
        canonical_json(&members).unwrap()
    );
    assert_eq!(engine.config.metadata_quorum(), quorum);
    assert!(CheckpointScope::admit(&engine).unwrap().is_none());
    assert!(!engine.blocked);
    // Poison an active member's optional file as well: the fallback must be
    // selected before opening any checkpoint, not after probing a stopped root.
    let marker = path(&members[0]);
    fs::write(&marker, b"not checkpoint bytes").unwrap();
    assert!(
        load(&engine, &BTreeMap::new(), &Records::new())
            .unwrap()
            .is_none()
    );
    save(&engine, &BTreeMap::new()).unwrap();
    assert_eq!(fs::read(marker).unwrap(), b"not checkpoint bytes");
    assert_eq!(fs::read(target.root.join("disk.json")).unwrap(), seal);
    assert_eq!(canonical_json(engine.state()).unwrap(), old_state);
}

#[test]
fn checkpoint_scope_generation_cannot_be_recreated_from_the_same_config_json() {
    let (temp, root, config) = setup();
    let mut engine = reopen(&root).0;
    let mut target = config.clone();
    let additions =
        Config::development((0..4).map(|index| temp.path().join(format!("added-{index}"))))
            .unwrap();
    let disk = additions.disks.into_iter().nth(3).unwrap();
    private_dir(&disk.root).unwrap();
    target.disks.push(disk.clone());
    engine
        .begin_membership_handoff(target, "admin", OperationId::generate().unwrap())
        .unwrap();
    let archive = engine.build_managed_membership_control_archive().unwrap();
    engine
        .enroll_pending_members(
            &archive.archive_path,
            &archive.root_hash,
            vec![crate::PendingMemberDirectory {
                disk_id: disk.disk_id.clone(),
                directory: fs::File::open(&disk.root).unwrap(),
            }],
        )
        .unwrap();
    engine.resume_membership_joint_decision().unwrap();
    engine.activate_pending_membership().unwrap();
    let generation = engine.config.generation_hash().unwrap().to_owned();
    let scope = CheckpointScope::admit(&engine).unwrap().unwrap();
    assert_eq!(
        scope.context.authority,
        CheckpointAuthority::Generation { hash: generation }
    );
    let config_hash = scope.context.config_hash.clone();
    drop(scope);
    let prefixes = prefixes(&engine);
    let added_root = &engine.disk_resources.get(&disk).unwrap().root;
    assert!(
        metadata(added_root, Path::new("histories"))
            .unwrap()
            .is_none()
    );
    save(&engine, &journals(&engine)).unwrap();
    let checkpoint = load(&engine, &prefixes, &Records::new()).unwrap().unwrap();
    assert!(
        checkpoint.coverage[&disk.disk_id]
            .iter()
            .any(|entry| { entry.path == "histories" && matches!(entry.value, Covered::Absent) })
    );
    assert!(
        metadata(added_root, Path::new("histories"))
            .unwrap()
            .is_none()
    );
    let state = canonical_json(engine.state()).unwrap();
    let admitted = engine.config.clone();
    // The serialized Config deliberately has no admitted Generation. Schema
    // validation and equal roots/voters must not recreate that authority.
    engine.config = decode_strict(&canonical_json(&engine.config).unwrap()).unwrap();
    assert_eq!(
        object_hash("replay-config", &engine.config).unwrap(),
        config_hash
    );
    assert!(engine.config.generation().is_none());
    assert!(load(&engine, &prefixes, &Records::new()).unwrap().is_none());
    assert_eq!(canonical_json(engine.state()).unwrap(), state);
    engine.config = admitted;
    assert!(load(&engine, &prefixes, &Records::new()).unwrap().is_some());

    // Covering absence is not permission to ignore future contents. Even an
    // empty new directory requires ordinary replay under the same authority.
    engine
        .disk_resources
        .get(&disk)
        .unwrap()
        .root
        .private_dir(&disk.root.join("histories"))
        .unwrap();
    assert!(load(&engine, &prefixes, &Records::new()).unwrap().is_none());
    assert_eq!(canonical_json(engine.state()).unwrap(), state);
}

#[test]
fn checkpoint_scope_missing_context_falls_back_without_erasing_state_or_history() {
    let (_temp, root, config) = setup();
    let engine = reopen(&root).0;
    let state = canonical_json(engine.state()).unwrap();
    let old_journals = journals(&engine);
    let mut old: json_codec::Value =
        decode_strict(&fs::read(path(&config.disks[0])).unwrap()).unwrap();
    let checkpoint = old["checkpoint"].as_object_mut().unwrap();
    let context = checkpoint.remove("context").unwrap();
    checkpoint.insert("config_hash".into(), context["config_hash"].clone());
    let bytes = canonical_json(&old).unwrap();
    for disk in &config.disks {
        fs::write(path(disk), &bytes).unwrap();
    }
    drop(engine);
    let (mut engine, decoded) = reopen(&root);
    assert!(decoded >= 6);
    assert_eq!(canonical_json(engine.state()).unwrap(), state);
    assert_eq!(journals(&engine), old_journals);
    let warnings = engine.take_recovery_warnings();
    assert!(std::error::Error::source(&warnings.checkpoint_load.unwrap()).is_some());
    assert!(warnings.checkpoint_save.is_none());
    drop(engine);
    assert_eq!(reopen(&root).1, 1);
}

#[test]
fn checkpoint_scope_exit_never_skips_d3_or_reopens_the_stopped_directory() {
    for action in [
        crate::MembershipExitAction::PlannedOffline,
        crate::MembershipExitAction::Deregister,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("system");
        let mut config =
            Config::development((0..3).map(|index| temp.path().join(format!("disk-{index}"))))
                .unwrap();
        for disk in &mut config.disks {
            disk.level = 1;
            disk.upstream_slot_id = None;
        }
        let mut engine = Engine::initialize(&root, config.clone()).unwrap();
        let mut branches = Vec::new();
        for index in 0..2 {
            let repository = engine
                .create_repository(
                    &RepositoryName::parse(&format!("scope-{index}")).unwrap(),
                    &config.disks[index].slot_id,
                    1,
                    "admin",
                    OperationId::generate().unwrap(),
                )
                .unwrap();
            save_file(&mut engine, &repository.main_branch_id, "body", b"retained");
            branches.push(repository.main_branch_id);
        }
        drop(engine);
        let mut engine = reopen(&root).0;
        let target = &config.disks[0];
        let held = Arc::clone(engine.disk_resources.get(target).unwrap());
        let commits = canonical_json(&engine.state.commits).unwrap();
        let versions = canonical_json(&engine.state.versions).unwrap();
        engine
            .begin_disk_exit(
                &target.disk_id,
                action,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        let archive = engine.build_managed_membership_control_archive().unwrap();
        engine
            .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
            .unwrap();
        engine.resume_membership_joint_decision().unwrap();
        let tail = held.root.admit_io().unwrap();
        assert_eq!(
            engine.activate_pending_disk_exit().unwrap_err().code(),
            ErrorCode::DiskUnavailable
        );
        assert!(engine.config.generation().is_none());
        assert!(!engine.config.excludes_io_disk(&target.disk_id));
        assert!(engine.pending_disk_exit.is_some());
        for disk in engine
            .members()
            .iter()
            .filter(|disk| disk.disk_id != target.disk_id)
        {
            fs::write(path(disk), b"must not probe during the stopped tail").unwrap();
        }
        assert!(CheckpointScope::admit(&engine).unwrap().is_none());
        assert!(
            load(&engine, &BTreeMap::new(), &Records::new())
                .unwrap()
                .is_none()
        );
        save(&engine, &BTreeMap::new()).unwrap();
        drop(tail);
        engine.activate_pending_disk_exit().unwrap();
        held.root.io_binding().unwrap().closed_proof().unwrap();
        assert!(engine.disk_resources.available(target).is_none());
        assert!(engine.config.excludes_io_disk(&target.disk_id));
        assert_eq!(canonical_json(&engine.state.commits).unwrap(), commits);
        assert_eq!(canonical_json(&engine.state.versions).unwrap(), versions);
        let expected_entries = usize::from(action == crate::MembershipExitAction::PlannedOffline);
        assert_eq!(
            engine.list_tree(&branches[0]).unwrap().len(),
            expected_entries
        );
        let seal = fs::read(target.root.join("disk.json")).unwrap();
        let stopped_checkpoint = fs::read(path(target)).unwrap();
        // Corrupt active optional files to prove neither admitted nor absent
        // current views allow even a checkpoint read during an Exit generation.
        for disk in engine.members() {
            fs::write(path(&disk), b"must replay exact D3").unwrap();
        }
        for with_view in [true, false] {
            let view = (!with_view).then(|| engine.current_namespaces.take());
            assert!(CheckpointScope::admit(&engine).unwrap().is_none());
            assert!(
                load(&engine, &BTreeMap::new(), &Records::new())
                    .unwrap()
                    .is_none()
            );
            save(&engine, &BTreeMap::new()).unwrap();
            if let Some(view) = view {
                engine.current_namespaces = view;
            }
        }
        save_file(&mut engine, &branches[1], "after-exit", b"new current view");
        assert!(CheckpointScope::admit(&engine).unwrap().is_none());
        let after = canonical_json(engine.state()).unwrap();
        drop(engine);
        let (engine, decoded) = reopen(&root);
        assert!(
            decoded >= 2,
            "D3 and the ordinary successor must both replay"
        );
        assert_eq!(canonical_json(engine.state()).unwrap(), after);
        assert_eq!(
            engine.list_tree(&branches[0]).unwrap().len(),
            expected_entries
        );
        assert_eq!(engine.list_tree(&branches[1]).unwrap().len(), 2);
        assert_eq!(fs::read(target.root.join("disk.json")).unwrap(), seal);
        assert_eq!(fs::read(path(target)).unwrap(), stopped_checkpoint);
        assert!(held.root.directory(&target.root).is_err());
    }
}
