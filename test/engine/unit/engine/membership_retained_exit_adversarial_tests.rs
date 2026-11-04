//! Independent attacks on actual retained journals, archive bytes, stop barriers
//! and portable decisions. Hashing altered metadata is not a state proof.
use super::*;
use crate::metadata_map::{Encoder, ProofMeasure, Reference};
use std::{fs, io::Read};

struct RetainedFixture {
    temp: tempfile::TempDir,
    engine: Engine,
    original: Config,
    operation: OperationId,
    version: String,
}

impl RetainedFixture {
    fn online() -> Self {
        Self::with_voters(3)
    }

    fn with_voters(voters: usize) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let original =
            Config::development((0..voters).map(|n| temp.path().join(format!("disk-{n}"))))
                .unwrap();
        let mut engine = Engine::initialize(&temp.path().join("system"), original.clone()).unwrap();
        let repository = engine
            .create_repository(
                &RepositoryName::parse("retained-adversarial").unwrap(),
                "P01",
                3,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        let workspace = engine
            .create_workspace(&repository.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(
                &workspace.workspace_id,
                "body.txt",
                b"retained exit evidence",
            )
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate().unwrap(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: String::new(),
                request_digest: None,
            })
            .unwrap();
        let version = engine.list_tree(&repository.main_branch_id).unwrap()[0]
            .version_id
            .clone();
        Self {
            temp,
            engine,
            original,
            operation: OperationId::generate().unwrap(),
            version,
        }
    }

    fn parked(&self) -> PathBuf {
        self.temp.path().join("absent-original")
    }

    fn lose_target(self) -> Self {
        let parked = self.parked();
        drop(self.engine);
        fs::rename(&self.original.disks[0].root, parked).unwrap();
        let engine = Engine::open(&self.temp.path().join("system")).unwrap();
        Self { engine, ..self }
    }

    fn missing() -> Self {
        Self::online().lose_target()
    }

    fn begin(&mut self) -> Result<MembershipFenceStatus> {
        self.engine.begin_disk_exit(
            &self.original.disks[0].disk_id,
            MembershipExitAction::Deregister,
            "admin",
            self.operation,
        )
    }

    fn controls(&mut self) -> MembershipControlArchive {
        let fence = self.begin().unwrap();
        assert_eq!((fence.original_members, fence.original_quorum), (3, 2));
        assert_eq!(fence.confirmed_members, 2);
        let archive = self
            .engine
            .build_managed_membership_control_archive()
            .unwrap();
        self.engine
            .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
            .unwrap();
        let joint = self.engine.resume_membership_joint_decision().unwrap();
        assert_eq!((joint.original_members, joint.original_quorum), (3, 2));
        assert_eq!((joint.target_members, joint.target_quorum), (2, 2));
        assert_eq!(joint.union_members, 3);
        assert_eq!(joint.certificate_quorum_counts(), (2, 2));
        archive
    }

    fn reopen(self) -> Self {
        drop(self.engine);
        let engine = Engine::open(&self.temp.path().join("system")).unwrap();
        Self { engine, ..self }
    }

    fn resume(self) -> Self {
        drop(self.engine);
        let engine =
            Engine::resume_membership_activation(&self.temp.path().join("system")).unwrap();
        Self { engine, ..self }
    }

    fn retained_bytes(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        self.original.disks[1..]
            .iter()
            .flat_map(|disk| file_bytes(&disk.root))
            .collect()
    }

    fn assert_body(&self) {
        let mut body = Vec::new();
        self.engine
            .open_content(&self.version)
            .unwrap()
            .file
            .read_to_end(&mut body)
            .unwrap();
        assert_eq!(body, b"retained exit evidence");
    }
}

fn file_bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut entries = BTreeMap::new();
    for item in fs::read_dir(root).unwrap() {
        let path = item.unwrap().path();
        if path.is_dir() {
            entries.extend(file_bytes(&path));
        } else {
            entries.insert(path.clone(), fs::read(path).unwrap());
        }
    }
    entries
}

fn object_file(archive: &Path, kind: &str, hash: &str) -> PathBuf {
    archive
        .join("objects")
        .join(kind)
        .join(&hash[..2])
        .join(format!("{hash}.json"))
}

fn archive_object(
    archive: &Path,
    kind: &'static str,
    hash: &str,
) -> Result<AuthenticatedMetadataObject> {
    AuthenticatedMetadataObject::authenticate(
        kind,
        hash,
        decode_strict(&fs::read(object_file(archive, kind, hash))?)?,
    )
}

fn archive_map(
    archive: &Path,
    reference: &json_codec::Value,
) -> json_codec::Map<String, json_codec::Value> {
    let reference: Reference = json_codec::from_value(reference.clone()).unwrap();
    crate::metadata_map::read_map(&reference, |kind, hash| archive_object(archive, kind, hash))
        .unwrap()
}

fn replace_archive_map(
    archive: &Path,
    root: &mut json_codec::Value,
    field: &str,
    values: json_codec::Map<String, json_codec::Value>,
) {
    let mut encoder = Encoder::new(MAX_METADATA);
    let reference = encoder
        .map_root(values, ProofMeasure::Plain)
        .unwrap()
        .reference()
        .clone();
    for object in encoder.finish() {
        let path = object_file(archive, object.kind(), &object.hash);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, object.bytes()).unwrap();
    }
    root[field] = json_codec::to_value(reference).unwrap();
}

fn republish_archive_root(archive: &mut MembershipControlArchive, root: &json_codec::Value) {
    archive.root_hash = object_hash("membership-control-archive", root).unwrap();
    fs::write(
        archive.archive_path.join("archive-root.json"),
        canonical_json(root).unwrap(),
    )
    .unwrap();
}

#[test]
fn retained_exit_rejects_real_unpublished_finalize_and_partial_publication() {
    for point in ["after_finalize", "after_publication_2"] {
        let mut fixture = RetainedFixture::online();
        let boundary = fixture.engine.state.sequence;
        fixture.engine.inject_fault_once(point);
        let error = fixture
            .engine
            .create_repository(
                &RepositoryName::parse("unfinished-decision").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            Error::Context { code: ErrorCode::InDoubt, retryable: true, source, .. }
                if source.code() == ErrorCode::InjectedFailure
        ));
        assert!(fixture.original.disks[1..].iter().all(|disk| {
            scan(disk).unwrap().records.iter().any(|(record, _)| {
                record.sequence > boundary && record.record_type == RecordType::Finalize
            })
        }));
        drop(fixture.engine);
        fs::rename(
            &fixture.original.disks[0].root,
            fixture.temp.path().join("absent-original"),
        )
        .unwrap();
        fixture.engine = Engine::open_for_recovery(&fixture.temp.path().join("system")).unwrap();
        let before = fixture.retained_bytes();
        assert!(fixture.begin().is_err(), "{point}");
        assert!(fixture.engine.membership_fence_status().is_none());
        assert_eq!(fixture.retained_bytes(), before, "{point}");
    }
}

#[test]
fn retained_exit_requires_every_target_not_merely_an_online_majority() {
    // Three surviving roots satisfy both old N5/Q3 and new N4/Q3, but
    // the fourth target has not supplied its complete current source.
    let fixture = RetainedFixture::with_voters(5).lose_target();
    let retained = fixture.original.disks[4].root.clone();
    drop(fixture.engine);
    fs::rename(retained, fixture.temp.path().join("absent-retained")).unwrap();
    let engine = Engine::open_for_recovery(&fixture.temp.path().join("system")).unwrap();
    let mut fixture = RetainedFixture { engine, ..fixture };
    let before = file_bytes(&fixture.original.disks[2].root);
    assert!(fixture.begin().is_err());
    assert!(fixture.engine.membership_fence_status().is_none());
    assert_eq!(file_bytes(&fixture.original.disks[2].root), before);
    assert_eq!(fixture.engine.config.metadata_members().len(), 5);
    assert_eq!(fixture.engine.config.metadata_quorum(), 3);
}

#[test]
fn retained_exit_old_membership_stays_fenced_when_absent_voter_returns() {
    let mut fixture = RetainedFixture::missing();
    fixture.begin().unwrap();
    let transition = fixture
        .engine
        .membership_fence_status()
        .unwrap()
        .transition_hash;
    drop(fixture.engine);
    fs::rename(
        fixture.temp.path().join("absent-original"),
        &fixture.original.disks[0].root,
    )
    .unwrap();
    fixture.engine = Engine::open(&fixture.temp.path().join("system")).unwrap();
    assert_eq!(fixture.engine.config.metadata_members().len(), 3);
    assert_eq!(fixture.engine.config.metadata_quorum(), 2);
    assert_eq!(
        fixture
            .engine
            .membership_fence_status()
            .unwrap()
            .transition_hash,
        transition
    );
    let before = fixture
        .original
        .disks
        .iter()
        .flat_map(|disk| file_bytes(&disk.root))
        .collect::<BTreeMap<_, _>>();
    assert!(
        fixture
            .engine
            .create_repository(
                &RepositoryName::parse("stale-original-writer").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap(),
            )
            .is_err()
    );
    assert_eq!(
        fixture
            .original
            .disks
            .iter()
            .flat_map(|disk| file_bytes(&disk.root))
            .collect::<BTreeMap<_, _>>(),
        before
    );
    assert!(
        !scan(&fixture.original.disks[0])
            .unwrap()
            .records
            .iter()
            .any(|(record, _)| record.record_type == RecordType::MembershipFence)
    );
}

#[test]
fn retained_exit_unopened_target_waits_for_old_io_domain_across_restart() {
    let fixture = RetainedFixture::online();
    let disk = fixture.original.disks[0].clone();
    let old = Arc::clone(fixture.engine.disk_resources.get(&disk).unwrap());
    let binding = old.root.io_binding().unwrap().clone();
    let tail = old.root.admit_io().unwrap();
    let original_authority = fixture.engine.config.clone();
    let mut alternate = old.seal.clone();
    alternate.inode = alternate.inode.checked_add(1).unwrap();
    let other = DiskIoBinding::new(&alternate, &original_authority).unwrap();
    let other_tail = other.admit().unwrap();
    let mut fixture = fixture.lose_target();
    let target_bytes = file_bytes(&fixture.parked());
    assert!(fixture.engine.disk_resources.available(&disk).is_none());
    fixture.controls();
    assert_eq!(
        fixture
            .engine
            .activate_pending_disk_exit()
            .unwrap_err()
            .code(),
        ErrorCode::DiskUnavailable
    );
    let status = fixture.engine.disk_exit_status().unwrap().unwrap();
    assert_eq!(status.phase, DiskExitPhase::Activating);
    assert!(
        fixture
            .engine
            .recover_disk_exit_stop(&status.target, fixture.operation, &status.transition_hash)
            .is_ok()
    );
    assert!(
        !fixture
            .engine
            .state
            .operations
            .contains_key(&fixture.operation)
    );
    assert!(binding.closed_proof().is_err());
    drop(fixture.engine);
    // Ordinary open cannot adopt the still-predecessor seals. Explicit resume
    // is the entry that may promote them, but must first close every old domain.
    assert!(Engine::open(&fixture.temp.path().join("system")).is_err());
    assert!(
        matches!(Engine::resume_membership_activation(&fixture.temp.path().join("system")), Err(error) if error.code() == ErrorCode::DiskUnavailable)
    );
    drop(binding.begin_quiesce().unwrap());
    drop(tail);
    binding.closed_proof().unwrap();
    assert!(Engine::open(&fixture.temp.path().join("system")).is_err());
    assert!(
        matches!(Engine::resume_membership_activation(&fixture.temp.path().join("system")), Err(error) if error.code() == ErrorCode::DiskUnavailable)
    );
    drop(other.begin_quiesce().unwrap());
    drop(other_tail);
    other.closed_proof().unwrap();
    std::os::unix::fs::symlink(&disk.root, &disk.root).unwrap();
    fixture.engine =
        Engine::resume_membership_activation(&fixture.temp.path().join("system")).unwrap();
    let result = fixture.engine.activate_pending_disk_exit().unwrap();
    assert_eq!(result.operation_id, fixture.operation);
    assert_eq!(
        fixture.engine.disk_exit_status().unwrap().unwrap().phase,
        DiskExitPhase::Published
    );
    assert_eq!(file_bytes(&fixture.parked()), target_bytes);
    assert!(
        fs::symlink_metadata(&disk.root)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    alternate.inode = alternate.inode.checked_add(1).unwrap();
    assert!(
        matches!(DiskIoBinding::new(&alternate, &original_authority), Err(error) if error.code() == ErrorCode::DiskUnavailable)
    );
    fixture.assert_body();
    // A later exit must carry the first logical prohibition, rather than
    // retaining only the current operation's newly closed target.
    let next_operation = OperationId::generate().unwrap();
    fixture
        .engine
        .begin_disk_exit(
            &fixture.original.disks[1].disk_id,
            MembershipExitAction::PlannedOffline,
            "admin",
            next_operation,
        )
        .unwrap();
    let archive = fixture
        .engine
        .build_managed_membership_control_archive()
        .unwrap();
    fixture
        .engine
        .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
        .unwrap();
    fixture.engine.resume_membership_joint_decision().unwrap();
    fixture.engine.activate_pending_disk_exit().unwrap();
    let fixture = fixture.reopen();
    assert_eq!(
        fixture
            .engine
            .config
            .generation()
            .unwrap()
            .portable
            .bootstrap_stops()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(fixture.engine.state.operations[&fixture.operation], result);
    assert!(
        fixture
            .engine
            .state
            .operations
            .contains_key(&next_operation)
    );
    assert!(
        matches!(DiskIoBinding::new(&alternate, &original_authority), Err(error) if error.code() == ErrorCode::DiskUnavailable)
    );
    assert!(binding.admit().is_err());
    fixture.assert_body();
    assert_eq!(file_bytes(&fixture.parked()), target_bytes);
}

#[test]
fn retained_exit_partial_preparation_is_not_selected_without_the_durable_stop() {
    let mut fixture = RetainedFixture::missing();
    let archive = fixture.controls();
    let first = &fixture.original.disks[1];
    let ledger = fixture.temp.path().join("system/bootstrap-discovery");
    let before = file_bytes(&ledger);
    fixture
        .engine
        .inject_fault_once(&format!("after_disk_exit_prepared_{}", first.disk_id));
    assert_eq!(
        fixture
            .engine
            .activate_pending_disk_exit()
            .unwrap_err()
            .code(),
        ErrorCode::InjectedFailure
    );
    assert!(runtime::prepared_authority_path(first, &archive.transition_hash).is_file());
    assert_eq!(file_bytes(&ledger), before);
    let mut fixture = fixture.reopen();
    assert!(fixture.engine.config.generation().is_none());
    assert_eq!(
        fixture.engine.disk_exit_status().unwrap().unwrap().phase,
        DiskExitPhase::Fenced
    );
    assert!(
        !fixture
            .engine
            .state
            .operations
            .contains_key(&fixture.operation)
    );
    fixture.controls();
    fixture.engine.activate_pending_disk_exit().unwrap();
    fixture.assert_body();
}

#[test]
fn retained_exit_partial_marker_uses_complete_prepared_authority_without_target_probe() {
    let mut fixture = RetainedFixture::missing();
    fixture.controls();
    fixture.engine.inject_fault_once(&format!(
        "after_disk_exit_marker_{}",
        fixture.original.disks[1].disk_id
    ));
    assert_eq!(
        fixture
            .engine
            .activate_pending_disk_exit()
            .unwrap_err()
            .code(),
        ErrorCode::InjectedFailure
    );
    assert!(
        fixture.original.disks[1]
            .root
            .join(GENERATION_FILE)
            .is_file()
    );
    assert!(
        !fixture.original.disks[2]
            .root
            .join(GENERATION_FILE)
            .exists()
    );
    let expected = fixture.engine.config.generation_hash().unwrap().to_owned();
    std::os::unix::fs::symlink(
        &fixture.original.disks[0].root,
        &fixture.original.disks[0].root,
    )
    .unwrap();
    let mut fixture = fixture.resume();
    assert_eq!(
        fixture.engine.config.generation_hash(),
        Some(expected.as_str())
    );
    fixture.engine.activate_pending_disk_exit().unwrap();
    assert_eq!(
        fixture.engine.disk_exit_status().unwrap().unwrap().phase,
        DiskExitPhase::Published
    );
    fixture.assert_body();
}

#[test]
fn retained_exit_missing_prepared_authority_after_stop_cannot_fall_back_to_genesis() {
    let mut fixture = RetainedFixture::missing();
    let archive = fixture.controls();
    fixture.engine.inject_fault_once("after_disk_exit_stop");
    assert_eq!(
        fixture
            .engine
            .activate_pending_disk_exit()
            .unwrap_err()
            .code(),
        ErrorCode::InjectedFailure
    );
    let path =
        runtime::prepared_authority_path(&fixture.original.disks[1], &archive.transition_hash);
    let saved = fs::read(&path).unwrap();
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(
        &fixture.original.disks[0].root,
        &fixture.original.disks[0].root,
    )
    .unwrap();
    drop(fixture.engine);
    assert!(Engine::open(&fixture.temp.path().join("system")).is_err());
    assert!(Engine::resume_membership_activation(&fixture.temp.path().join("system")).is_err());
    fs::write(&path, saved).unwrap();
    fixture.engine =
        Engine::resume_membership_activation(&fixture.temp.path().join("system")).unwrap();
    fixture.engine.activate_pending_disk_exit().unwrap();
    fixture.assert_body();
}

#[test]
fn retained_exit_rereads_each_publication_prefix_before_bypassing_write_block() {
    let mut fixture = RetainedFixture::missing();
    let disk = &fixture.original.disks[2];
    let path = log_path(&fixture.engine.config, disk);
    let before_damage = fs::read(&path).unwrap();
    let mut reader = JournalFrameReader::new(before_damage.as_slice());
    let mut offset = 0;
    while let Some((record, _)) = reader.next_record().unwrap() {
        if record.record_type == RecordType::Publication
            && record.sequence == fixture.engine.state.sequence
        {
            break;
        }
        offset = reader.valid_bytes;
    }
    assert!(offset > 0 && offset < before_damage.len() as u64);
    fs::write(&path, &before_damage[..offset as usize]).unwrap();
    let before = fixture.retained_bytes();
    assert!(fixture.begin().is_err());
    assert!(fixture.engine.membership_fence_status().is_none());
    assert_eq!(fixture.retained_bytes(), before);
}

#[test]
fn retained_exit_archive_rehashing_cannot_replace_actual_identity_or_fence_tail() {
    for damage in [
        "different-seal",
        "wrong-tail",
        "truncated-fence",
        "empty-members",
        "empty-seals",
    ] {
        let mut fixture = RetainedFixture::missing();
        let mut archive = fixture.controls();
        let retained_before = fixture.retained_bytes();
        let target_before = file_bytes(&fixture.parked());
        let mut root: json_codec::Value =
            decode_strict(&fs::read(archive.archive_path.join("archive-root.json")).unwrap())
                .unwrap();
        let mut members = archive_map(&archive.archive_path, &root["members"]);
        let disk = &fixture.original.disks[1];
        let directory = archive
            .archive_path
            .join("journals")
            .join(&disk.disk_id)
            .join(&disk.slot_id)
            .join(disk.binding_generation.to_string());
        let member = members.get_mut(&disk.disk_id).unwrap();
        match damage {
            "different-seal" => {
                let bytes = fs::read(fixture.original.disks[2].root.join("disk.json")).unwrap();
                fs::write(directory.join("disk.json"), &bytes).unwrap();
                member["seal_bytes_hash"] = blake3::hash(&bytes).to_hex().to_string().into();
            }
            "wrong-tail" => member["journal"]["tail_frame_hash"] = "e".repeat(64).into(),
            "truncated-fence" => {
                let path = directory.join("journal.bin");
                let bytes = fs::read(&path).unwrap();
                let mut reader = JournalFrameReader::new(bytes.as_slice());
                let mut offset = 0;
                let mut previous = None;
                while let Some((record, frame)) = reader.next_record().unwrap() {
                    if record.record_type == RecordType::MembershipFence {
                        break;
                    }
                    offset = reader.valid_bytes;
                    previous = Some(frame);
                }
                assert!(offset > 0 && offset < bytes.len() as u64);
                let truncated = &bytes[..offset as usize];
                fs::write(path, truncated).unwrap();
                member["journal"]["bytes"] = offset.to_string().into();
                member["journal"]["content_hash"] =
                    blake3::hash(truncated).to_hex().to_string().into();
                member["journal"]["tail_frame_hash"] = previous.unwrap().into();
            }
            "empty-members" => members.clear(),
            "empty-seals" => replace_archive_map(
                &archive.archive_path,
                &mut root,
                "publication_seals",
                json_codec::Map::new(),
            ),
            _ => unreachable!(),
        }
        replace_archive_map(&archive.archive_path, &mut root, "members", members);
        republish_archive_root(&mut archive, &root);
        let damaged = file_bytes(&archive.archive_path);
        assert!(
            verify_membership_control_archive(
                &archive.archive_path,
                &archive.root_hash,
                &archive.transition_hash
            )
            .is_err(),
            "{damage}"
        );
        assert_eq!(file_bytes(&archive.archive_path), damaged, "{damage}");
        assert_eq!(fixture.retained_bytes(), retained_before, "{damage}");
        assert_eq!(file_bytes(&fixture.parked()), target_before, "{damage}");
    }
}

fn portable_map(
    joint: &json_codec::Value,
    reference: &json_codec::Value,
) -> json_codec::Map<String, json_codec::Value> {
    let reference: Reference = json_codec::from_value(reference.clone()).unwrap();
    crate::metadata_map::read_map(&reference, |kind, hash| {
        AuthenticatedMetadataObject::authenticate(
            kind,
            hash,
            joint["objects"][format!("{kind}/{hash}")].clone(),
        )
    })
    .unwrap()
}

fn replace_portable_map(
    joint: &mut json_codec::Value,
    values: json_codec::Map<String, json_codec::Value>,
) -> json_codec::Value {
    let mut encoder = Encoder::new(MAX_METADATA);
    let reference = encoder
        .map_root(values, ProofMeasure::Plain)
        .unwrap()
        .reference()
        .clone();
    for object in encoder.finish() {
        joint["objects"][format!("{}/{}", object.kind(), object.hash)] =
            decode_strict(object.bytes()).unwrap();
    }
    json_codec::to_value(reference).unwrap()
}

fn replace_joint_proposal(joint: &mut json_codec::Value, proposal: json_codec::Value) {
    let old_proposal = joint["proposal_hash"].as_str().unwrap().to_owned();
    let hash = object_hash("membership-joint-proposal", &proposal).unwrap();
    joint["objects"]
        .as_object_mut()
        .unwrap()
        .remove(&format!("membership-joint-proposal/{old_proposal}"));
    joint["objects"][format!("membership-joint-proposal/{hash}")] = proposal;
    joint["proposal_hash"] = hash.clone().into();
    let old_certificate = joint["certificate_hash"].as_str().unwrap().to_owned();
    let mut certificate = joint["objects"]
        .as_object_mut()
        .unwrap()
        .remove(&format!("membership-joint-certificate/{old_certificate}"))
        .unwrap();
    certificate["proposal_hash"] = hash.into();
    let hash = object_hash("membership-joint-certificate", &certificate).unwrap();
    joint["objects"][format!("membership-joint-certificate/{hash}")] = certificate;
    joint["certificate_hash"] = hash.into();
}

#[test]
fn retained_exit_joint_scope_and_member_substitution_fail_before_vote_reuse() {
    let mut fixture = RetainedFixture::missing();
    fixture.controls();
    fixture.engine.prepare_disk_exit_generation().unwrap();
    let core = fixture
        .engine
        .pending_disk_exit
        .as_ref()
        .unwrap()
        .generation
        .portable
        .exit_core()
        .unwrap();
    let joint = json_codec::to_value(&core.joint).unwrap();
    core.joint.verify().unwrap();
    for damage in [
        "full-format",
        "add-format",
        "omitted-original",
        "old-only-substitution",
    ] {
        let mut changed = joint.clone();
        let proposal_hash = changed["proposal_hash"].as_str().unwrap();
        let mut proposal =
            changed["objects"][format!("membership-joint-proposal/{proposal_hash}")].clone();
        match damage {
            "full-format" => proposal["format"] = "fenced-exit-control-slot-decision".into(),
            "add-format" => proposal["format"] = "fenced-joint-slot-decision".into(),
            "omitted-original" => {
                let mut originals = portable_map(&changed, &proposal["original_members"]);
                originals.remove(&fixture.original.disks[1].disk_id);
                proposal["original_members"] = replace_portable_map(&mut changed, originals);
            }
            "old-only-substitution" => {
                let mut originals = portable_map(&changed, &proposal["original_members"]);
                let witness = originals
                    .remove(&fixture.original.disks[1].disk_id)
                    .unwrap();
                originals.insert(fixture.original.disks[0].disk_id.clone(), witness);
                proposal["original_members"] = replace_portable_map(&mut changed, originals);
            }
            _ => unreachable!(),
        }
        // Rehash both envelopes so a stale proposal hash is not the rejection.
        // No real journal or durable vote is invented for the altered proposal.
        replace_joint_proposal(&mut changed, proposal);
        let changed: PortableJoint = json_codec::from_value(changed).unwrap();
        let error = changed.verify().err().unwrap();
        match damage {
            "full-format" | "add-format" => assert!(
                error
                    .message()
                    .contains("joint effect and certificate purpose differ")
            ),
            "omitted-original" => assert!(
                error
                    .message()
                    .contains("joint proposal member sets differ")
            ),
            _ => assert_eq!(error.code(), ErrorCode::DiskIdentityMismatch),
        }
    }
    assert!(
        !fixture
            .engine
            .state
            .operations
            .contains_key(&fixture.operation)
    );
}

#[test]
fn retained_exit_portable_joint_and_registry_require_every_referenced_object() {
    let mut fixture = RetainedFixture::missing();
    fixture.controls();
    fixture.engine.prepare_disk_exit_generation().unwrap();
    let generation = &fixture
        .engine
        .pending_disk_exit
        .as_ref()
        .unwrap()
        .generation;
    let core = generation.portable.exit_core().unwrap();
    core.registry_from_source(&generation.base).unwrap();
    let joint = json_codec::to_value(&core.joint).unwrap();
    for key in joint["objects"].as_object().unwrap().keys() {
        let mut changed = joint.clone();
        changed["objects"].as_object_mut().unwrap().remove(key);
        let changed: PortableJoint = json_codec::from_value(changed).unwrap();
        assert!(changed.verify().is_err(), "missing {key}");
    }
    for key in core.objects.keys() {
        let mut changed = core.clone();
        changed.objects.remove(key);
        assert!(
            changed.registry_from_source(&generation.base).is_err(),
            "missing registry object {key}"
        );
    }
    assert!(fixture.engine.config.generation().is_none());
    assert!(
        !fixture
            .engine
            .state
            .operations
            .contains_key(&fixture.operation)
    );
}
