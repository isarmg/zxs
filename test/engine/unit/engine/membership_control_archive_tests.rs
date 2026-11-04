use super::*;
use std::fs;

fn fixture() -> (tempfile::TempDir, Config, Engine, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let config =
        Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
    for name in ["first", "second"] {
        engine
            .create_repository(
                &RepositoryName::parse(name).unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
    }
    let mut target = config.clone();
    target.disks[2].metadata_member = false;
    engine
        .begin_membership_handoff(target, "admin", OperationId::generate().unwrap())
        .unwrap();
    let destination = temp.path().join("archive");
    private_dir(&destination).unwrap();
    (temp, config, engine, destination)
}
fn bytes(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(path: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path, out);
            } else {
                out.insert(path.clone(), fs::read(path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(path, &mut out);
    out
}
fn source_bytes(config: &Config) -> BTreeMap<PathBuf, Vec<u8>> {
    config
        .disks
        .iter()
        .flat_map(|disk| bytes(&disk.root))
        .collect()
}
fn store(path: &Path) -> ArchiveStore {
    ArchiveStore {
        root: DiskRoot::open(path).unwrap(),
    }
}
fn verify(
    path: &Path,
    reference: &MembershipControlArchive,
) -> Result<MembershipArchiveInspection> {
    verify_membership_control_archive(path, &reference.root_hash, &reference.transition_hash)
}
fn first_publication(engine: &Engine, config: &Config) -> (Candidate, Publication) {
    let records = scan(&config.disks[0]).unwrap().records;
    let record = &records
        .iter()
        .find(|(record, _)| record.record_type == RecordType::Publication)
        .unwrap()
        .0;
    let candidate = verified_object_at(
        &engine.disk_resources,
        &engine.members(),
        "candidate",
        &record.candidate_hash,
    )
    .unwrap();
    let publication = verified_object_at(
        &engine.disk_resources,
        &engine.members(),
        "publication",
        &record.object_hash,
    )
    .unwrap();
    (candidate, publication)
}

#[test]
fn membership_control_archive_is_independent_and_preserves_original_bytes() {
    let (_temp, config, mut engine, path) = fixture();
    let before = source_bytes(&config);
    let inspected = engine.inspect_membership_metadata_archive().unwrap();
    let archive = engine.build_membership_control_archive(&path).unwrap();
    assert_eq!(source_bytes(&config), before);
    let store = store(&path);
    for disk in &config.disks {
        let directory = store.member_directory(disk);
        assert_eq!(
            fs::read(directory.join("journal.bin")).unwrap(),
            before[&log_path(disk)]
        );
        assert_eq!(
            fs::read(directory.join("disk.json")).unwrap(),
            before[&disk.root.join("disk.json")]
        );
        assert_eq!(
            fs::read(directory.join("config.json")).unwrap(),
            before[&disk.root.join("metadata/pool-membership/config.json")]
        );
        for (record, _) in scan(disk).unwrap().records {
            if record.record_type == RecordType::Publication {
                for (kind, hash) in [
                    ("candidate", &record.candidate_hash),
                    ("publication", &record.object_hash),
                ] {
                    assert_eq!(
                        fs::read(store.object_path(kind, hash).unwrap()).unwrap(),
                        fs::read(object_path(disk, kind, hash)).unwrap()
                    );
                }
                let publication: Publication = verified_object_at(
                    &engine.disk_resources,
                    &engine.members(),
                    "publication",
                    &record.object_hash,
                )
                .unwrap();
                let seal = verified_publication_seal_at(
                    &engine.disk_resources,
                    &engine.members(),
                    &verified_object_at(
                        &engine.disk_resources,
                        &engine.members(),
                        "candidate",
                        &record.candidate_hash,
                    )
                    .unwrap(),
                    &publication,
                )
                .unwrap();
                assert_eq!(
                    fs::read(
                        store
                            .object_path(SEAL_KIND, &object_hash(SEAL_KIND, &seal).unwrap())
                            .unwrap()
                    )
                    .unwrap(),
                    fs::read(publication_seal_path(disk, &record.candidate_hash)).unwrap()
                );
            }
        }
    }
    assert!(!path.join("journal.bin").exists());
    assert_eq!(verify(&path, &archive).unwrap(), inspected);
    assert_eq!(inspected.published_candidates, 2);
    drop(engine);
    for disk in &config.disks {
        fs::rename(&disk.root, disk.root.with_extension("unavailable")).unwrap();
    }
    let before_archive = bytes(&path);
    assert_eq!(verify(&path, &archive).unwrap(), inspected);
    assert_eq!(bytes(&path), before_archive);
}

#[test]
fn membership_control_archive_same_intent_retry_is_identical() {
    let (_temp, config, mut engine, path) = fixture();
    let source = source_bytes(&config);
    let archive = engine.build_membership_control_archive(&path).unwrap();
    let first = bytes(&path);
    let mut reordered = config.clone();
    reordered.disks.reverse();
    engine.config = ValidatedConfig::try_from(reordered).unwrap();
    assert_eq!(
        engine.build_membership_control_archive(&path).unwrap(),
        archive
    );
    assert_eq!(bytes(&path), first);
    assert_eq!(source_bytes(&config), source);
    assert!(
        engine
            .create_repository(
                &RepositoryName::parse("blocked").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap()
            )
            .is_err()
    );
}

#[test]
fn membership_control_archive_interruptions_keep_partial_work_and_resume() {
    for point in [
        "after_membership_archive_journals",
        "after_membership_archive_metadata",
        "before_membership_archive_complete",
    ] {
        let (temp, config, mut engine, path) = fixture();
        let source = source_bytes(&config);
        engine.inject_fault_once(point);
        assert_eq!(
            engine
                .build_membership_control_archive(&path)
                .unwrap_err()
                .code(),
            ErrorCode::InjectedFailure
        );
        assert!(!path.join(ROOT_FILE).exists());
        if point == "before_membership_archive_complete" {
            fs::write(
                path.join(".state-atomic-0123456789abcdef0123456789abcdef.tmp"),
                b"uncertain final-root staging",
            )
            .unwrap();
        }
        let partial = bytes(&path);
        assert!(partial.keys().any(|path| path.ends_with("journal.bin")));
        drop(engine);
        let mut engine = Engine::open(&temp.path().join("system")).unwrap();
        let archive = engine.build_membership_control_archive(&path).unwrap();
        for (path, bytes) in partial {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
        assert_eq!(verify(&path, &archive).unwrap().published_candidates, 2);
        assert_eq!(source_bytes(&config), source);
    }
}

#[test]
fn membership_control_archive_io_failure_cannot_publish_completion() {
    let (_temp, config, mut engine, path) = fixture();
    let source = source_bytes(&config);
    engine.inject_fault_once("after_membership_archive_journals");
    assert!(engine.build_membership_control_archive(&path).is_err());
    fs::write(path.join("objects"), b"injected directory I/O conflict").unwrap();
    let partial = bytes(&path);
    assert!(engine.build_membership_control_archive(&path).is_err());
    assert!(!path.join(ROOT_FILE).exists());
    assert_eq!(bytes(&path), partial);
    assert_eq!(source_bytes(&config), source);
    // Remove only this test's injected obstruction, not production retry cleanup.
    fs::remove_file(path.join("objects")).unwrap();
    let archive = engine.build_membership_control_archive(&path).unwrap();
    verify(&path, &archive).unwrap();
}

#[test]
fn membership_control_archive_rejects_missing_old_graph_and_independent_finalize() {
    for damage in ["old-page", "finalize", "history", "control-auth", "seal"] {
        let (_temp, config, mut engine, path) = fixture();
        let (candidate, publication) = first_publication(&engine, &config);
        let snapshot: Snapshot = verified_object_at(
            &engine.disk_resources,
            &engine.members(),
            "snapshot",
            &candidate.snapshot_hash,
        )
        .unwrap();
        let archive = engine.build_membership_control_archive(&path).unwrap();
        let source = source_bytes(&config);
        let store = store(&path);
        let (kind, hash) = match damage {
            "old-page" => {
                let mut pages = Vec::new();
                crate::metadata_map::visit_objects(
                    &snapshot.maps["repositories"],
                    |kind, hash| store.load(kind, hash),
                    |object| {
                        if object.kind() == "state-page" {
                            pages.push(object.hash().to_owned());
                        }
                        Ok(())
                    },
                )
                .unwrap();
                ("state-page", pages.remove(0))
            }
            "finalize" => (
                "finalize",
                object_hash("finalize", &publication.finalize).unwrap(),
            ),
            "history" => (
                candidate.history_roots.kind().domain(),
                candidate.history_roots.hash().to_owned(),
            ),
            "control-auth" => (
                "state-control-auth",
                snapshot.control_auth.hash().to_owned(),
            ),
            _ => {
                let seal = verified_publication_seal_at(
                    &engine.disk_resources,
                    &engine.members(),
                    &candidate,
                    &publication,
                )
                .unwrap();
                (SEAL_KIND, object_hash(SEAL_KIND, &seal).unwrap())
            }
        };
        fs::remove_file(store.object_path(kind, &hash).unwrap()).unwrap();
        let before_archive = bytes(&path);
        assert!(verify(&path, &archive).is_err(), "{damage}");
        assert_eq!(bytes(&path), before_archive);
        assert_eq!(source_bytes(&config), source);
    }
}

#[test]
fn membership_control_archive_rejects_changed_logs_identity_and_trusted_binding() {
    for damage in [
        "prefix",
        "truncated",
        "identity",
        "expected-root",
        "expected-transition",
    ] {
        let (_temp, config, mut engine, path) = fixture();
        let mut archive = engine.build_membership_control_archive(&path).unwrap();
        let source = source_bytes(&config);
        let store = store(&path);
        let directory = store.member_directory(&config.disks[0]);
        match damage {
            "prefix" => {
                let path = directory.join("journal.bin");
                let mut bytes = fs::read(&path).unwrap();
                bytes[0] ^= 1;
                fs::write(path, bytes).unwrap();
            }
            "truncated" => {
                let path = directory.join("journal.bin");
                let mut bytes = fs::read(&path).unwrap();
                bytes.pop();
                fs::write(path, bytes).unwrap();
            }
            "identity" => {
                let path = directory.join("disk.json");
                let mut seal = decode_seal(&fs::read(&path).unwrap()).unwrap();
                seal.binding_generation += 1;
                fs::write(path, canonical_json(&seal).unwrap()).unwrap();
            }
            "expected-root" => archive.root_hash = "0".repeat(64),
            _ => archive.transition_hash = "0".repeat(64),
        }
        let before_archive = bytes(&path);
        assert!(verify(&path, &archive).is_err(), "{damage}");
        assert_eq!(bytes(&path), before_archive);
        assert_eq!(source_bytes(&config), source);
    }
}

#[test]
fn membership_control_archive_rejects_foreign_or_unbound_destination_without_changes() {
    for mode in [
        "foreign",
        "orphan-staging",
        "conflicting-binding",
        "symlink",
    ] {
        let (_temp, _config, mut engine, path) = fixture();
        match mode {
            "foreign" => {
                fs::write(path.join("keep.txt"), b"unrelated data").unwrap();
            }
            "orphan-staging" => {
                fs::write(
                    path.join(".state-atomic-0123456789abcdef0123456789abcdef.tmp"),
                    b"unconfirmed initial marker",
                )
                .unwrap();
            }
            "conflicting-binding" => {
                store(&path)
                    .root
                    .write_json(
                        &path.join(BINDING_FILE),
                        &ArchiveBinding {
                            format: Format::FrozenControlMetadata,
                            transition_hash: "0".repeat(64),
                        },
                        false,
                    )
                    .unwrap();
            }
            _ => {
                std::os::unix::fs::symlink("missing", path.join(BINDING_FILE)).unwrap();
            }
        }
        if mode == "symlink" {
            assert!(engine.build_membership_control_archive(&path).is_err());
            assert!(
                fs::symlink_metadata(path.join(BINDING_FILE))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
        } else {
            let before = bytes(&path);
            assert!(engine.build_membership_control_archive(&path).is_err());
            assert_eq!(bytes(&path), before);
        }
    }
}

#[test]
fn membership_control_archive_rejects_source_overlap_and_missing_fence() {
    let (_temp, config, mut engine, _path) = fixture();
    let nested = config.disks[0].root.join("empty-archive");
    private_dir(&nested).unwrap();
    let before = source_bytes(&config);
    assert!(engine.build_membership_control_archive(&nested).is_err());
    assert_eq!(source_bytes(&config), before);
    engine
        .membership_fence
        .as_mut()
        .unwrap()
        .receipts
        .pop_first();
    assert!(
        engine
            .build_membership_control_archive(&nested.with_extension("other"))
            .is_err()
    );
}

#[test]
fn membership_control_archive_manifest_uses_bounded_metadata_pages() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("archive");
    private_dir(&directory).unwrap();
    let store = store(&directory);
    let rows = (0..4096)
        .map(|n| (format!("candidate-{n:05}"), "0".repeat(64).into()))
        .collect();
    let root = store.map(rows).unwrap();
    assert_eq!(root.kind(), crate::metadata_map::ReferenceKind::Index);
    let decoded =
        crate::metadata_map::read_map(&root, |kind, hash| store.load(kind, hash)).unwrap();
    assert_eq!(decoded.len(), 4096);
    for (_, bytes) in bytes(temp.path()) {
        assert!(bytes.len() <= MAX_METADATA);
    }
}

#[test]
fn membership_control_archive_empty_genesis_and_nontrivial_metadata_prefix() {
    let temp = tempfile::tempdir().unwrap();
    let config =
        Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
    let mut target = config.clone();
    target.disks[2].metadata_member = false;
    engine
        .begin_membership_handoff(target, "admin", OperationId::generate().unwrap())
        .unwrap();
    let path = temp.path().join("archive");
    private_dir(&path).unwrap();
    let archive = engine.build_membership_control_archive(&path).unwrap();
    drop(engine);
    for disk in config.disks {
        fs::rename(&disk.root, disk.root.with_extension("unavailable")).unwrap();
    }
    assert_eq!(verify(&path, &archive).unwrap().published_candidates, 0);
    let store = store(&path);
    let map = store
        .map(
            (0..4096)
                .map(|n| (format!("key-{n}"), json_codec::Value::Null))
                .collect(),
        )
        .unwrap();
    let mut index: crate::metadata_map::MetadataIndex = json_codec::from_value(
        store
            .load("metadata-index", map.hash())
            .unwrap()
            .into_value(),
    )
    .unwrap();
    index.prefix = "f".into();
    let wrong = PreparedMetadataObject::new("metadata-index", &index, MAX_METADATA).unwrap();
    store
        .object_bytes(wrong.kind(), &wrong.hash, wrong.bytes())
        .unwrap();
    let wrong: Reference =
        json_codec::from_value(json_codec::json!({"kind":"metadata-index", "hash":wrong.hash}))
            .unwrap();
    assert!(crate::metadata_map::read_map(&wrong, |kind, hash| store.load(kind, hash)).is_err());
}

#[test]
fn membership_control_archive_preserves_reprotect_plan_and_original_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let config =
        Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
    let system = temp.path().join("system");
    let mut engine = Engine::initialize(&system, config.clone()).unwrap();
    let repo = engine
        .create_repository(
            &RepositoryName::parse("recovery").unwrap(),
            "P01",
            3,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "lost.txt", b"fixture body")
        .unwrap();
    let operation = OperationId::generate().unwrap();
    engine.inject_fault_once("after_accept_2");
    assert!(
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: operation,
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "pending".into(),
                request_digest: None
            })
            .is_err()
    );
    let last = scan(&config.disks[0]).unwrap().records.pop().unwrap().0;
    let candidate: Candidate = verified_object_at(
        &engine.disk_resources,
        &engine.members(),
        "candidate",
        &last.candidate_hash,
    )
    .unwrap();
    let version = &candidate.versions[0];
    for slot in &version.placement.route_slot_ids {
        let disk = engine.disk(slot).unwrap();
        let directory = version_dir(
            disk,
            &version.version.history_tree_id,
            &version.version.version_id,
        );
        fs::remove_file(
            crate::container_storage::content_path(&directory, &version.version).unwrap(),
        )
        .unwrap();
    }
    for workspace in engine.workspaces.values() {
        for source in workspace.files.values() {
            fs::remove_file(&source.source_path).unwrap();
        }
    }
    drop(engine);
    let mut engine = Engine::open_for_recovery(&system).unwrap();
    let plan = engine
        .prepare_degraded_recovery(operation, "admin", "fixture permanent loss")
        .unwrap();
    engine
        .commit_degraded_recovery(&plan.recovery_id, &plan.digest, "admin", true)
        .unwrap();
    let mut target = config.clone();
    target.disks[2].metadata_member = false;
    engine
        .begin_membership_handoff(target, "admin", OperationId::generate().unwrap())
        .unwrap();
    let path = temp.path().join("archive");
    private_dir(&path).unwrap();
    let source = source_bytes(&config);
    let archive = engine.build_membership_control_archive(&path).unwrap();
    assert_eq!(source_bytes(&config), source);
    let record = scan(&config.disks[0])
        .unwrap()
        .records
        .into_iter()
        .find(|(record, _)| record.record_type == RecordType::Reprotect)
        .unwrap()
        .0;
    let store = store(&path);
    assert_eq!(
        fs::read(
            store
                .object_path("recovery-plan", &record.object_hash)
                .unwrap()
        )
        .unwrap(),
        fs::read(object_path(
            &config.disks[0],
            "recovery-plan",
            &record.object_hash
        ))
        .unwrap()
    );
    drop(engine);
    for disk in config.disks {
        fs::rename(&disk.root, disk.root.with_extension("unavailable")).unwrap();
    }
    assert_eq!(verify(&path, &archive).unwrap().published_candidates, 2);
    fs::remove_file(
        store
            .object_path("recovery-plan", &record.object_hash)
            .unwrap(),
    )
    .unwrap();
    assert!(verify(&path, &archive).is_err());
}

#[test]
fn membership_control_archive_rejects_renamed_source_or_system_ancestor() {
    for system in [false, true] {
        let (temp, config, mut engine, _path) = fixture();
        let original = if system {
            temp.path().join("system")
        } else {
            config.disks[0].root.clone()
        };
        let moved = original.with_extension("moved");
        fs::rename(&original, &moved).unwrap();
        let destination = moved.join("empty-archive");
        private_dir(&destination).unwrap();
        let before = bytes(&moved);
        assert!(
            engine
                .build_membership_control_archive(&destination)
                .is_err()
        );
        assert_eq!(bytes(&moved), before);
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
        // A replacement at the old name is not the held admission either.
        private_dir(&original).unwrap();
        assert!(
            engine
                .build_membership_control_archive(&destination)
                .is_err()
        );
        assert_eq!(bytes(&moved), before);
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
    }
}

#[test]
fn membership_control_archive_cannot_replace_admitted_fence_with_its_own_summary() {
    let (_temp, config, mut engine, path) = fixture();
    let mut archive = engine.build_membership_control_archive(&path).unwrap();
    let original = source_bytes(&config);
    let store = store(&path);
    let disk = &config.disks[0];
    let journal_path = store.member_directory(disk).join("journal.bin");
    let mut reader = JournalFrameReader::new(fs::File::open(&journal_path).unwrap());
    let retained = loop {
        let before = reader.valid_bytes;
        let (record, _) = reader.next_record().unwrap().unwrap();
        if record.record_type == RecordType::MembershipFence {
            break before;
        }
    };
    drop(reader);
    fs::OpenOptions::new()
        .write(true)
        .open(&journal_path)
        .unwrap()
        .set_len(retained)
        .unwrap();
    let mut root: ArchiveRoot = decode_strict(&fs::read(path.join(ROOT_FILE)).unwrap()).unwrap();
    let mut members =
        crate::metadata_map::read_map(&root.members, |kind, hash| store.load(kind, hash)).unwrap();
    let mut member: ArchivedMember =
        json_codec::from_value(members.remove(&disk.disk_id).unwrap()).unwrap();
    member.journal.bytes = retained;
    member.journal.content_hash = content_hash(&fs::read(&journal_path).unwrap());
    // A self-generated accurate digest of a shortened log cannot replace the
    // previously admitted fence Receipt bound by the transition.
    members.insert(disk.disk_id.clone(), json_codec::to_value(member).unwrap());
    root.members = store.map(members).unwrap();
    archive.root_hash = object_hash(ROOT_KIND, &root).unwrap();
    fs::write(path.join(ROOT_FILE), canonical_json(&root).unwrap()).unwrap();
    let before = bytes(&path);
    assert!(verify(&path, &archive).is_err());
    assert_eq!(bytes(&path), before);
    assert_eq!(source_bytes(&config), original);
}
