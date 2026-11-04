use super::*;
use crate::engine::membership_transition::abort::tests::authority_reads::{
    ReadFixture, cancelled_candidate, read_fixture,
};
use std::fs;

fn copy_tree(from: &Path, to: &Path) {
    private_dir(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &to.join(entry.file_name()));
        } else {
            fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
    }
}
fn object<T: Serialize>(store: &ArchiveStore, kind: &'static str, value: &T) -> String {
    let prepared = PreparedMetadataObject::new(kind, value, MAX_METADATA).unwrap();
    store
        .object_bytes(kind, &prepared.hash, prepared.bytes())
        .unwrap();
    prepared.hash
}

struct HistoricalArchive {
    fixture: ReadFixture,
    store: ArchiveStore,
    root: ArchiveRoot,
    intent: TransitionIntent,
    candidate: Candidate,
}
impl HistoricalArchive {
    /// Construct already-recorded history with explicit test-only journal
    /// writers. Never enters Add activation or makes a production Abort choice.
    fn new() -> Self {
        let fixture = read_fixture();
        let destination = fixture.temp.path().join("successor-archive");
        copy_tree(&fixture.source, &destination);
        let store = ArchiveStore {
            root: DiskRoot::open(&destination).unwrap(),
        };
        let (original_archive, _) = fixture.authority.archives().unwrap().pop().unwrap();
        object(&store, ROOT_KIND, &original_archive);
        for core in fixture.authority.node_objects().unwrap() {
            store
                .object_bytes(core.kind(), &core.hash, core.bytes())
                .unwrap();
        }
        for (hash, evidence) in fixture.authority.evidence_entries() {
            store.install_evidence(hash, evidence).unwrap();
        }
        let (candidate, state, result) = cancelled_candidate(&fixture.authority, &fixture.base);
        let candidate_hash = object(&store, "candidate", &candidate);
        let writers = fixture
            .original
            .disks
            .iter()
            .enumerate()
            .map(|(index, disk)| {
                let mut writer = disk.clone();
                writer.root = fixture.temp.path().join(format!("history-writer-{index}"));
                private_dir(&writer.root).unwrap();
                writer
            })
            .collect::<Vec<_>>();
        let snapshot = prepare_snapshot(&state, MAX_METADATA).unwrap();
        snapshot.store_new_objects(&writers[..1]).unwrap();
        let (_, history) =
            crate::history_package::prepare_history_manifests(&state, MAX_METADATA).unwrap();
        for prepared in history {
            prepared.store(&writers[..1]).unwrap();
        }
        copy_tree(
            &writers[0].root.join("metadata/objects"),
            &store.path("objects"),
        );
        let accepts = writers
            .iter()
            .map(|disk| {
                crate::journal::append(
                    disk,
                    RecordType::Accept,
                    candidate.sequence,
                    &candidate_hash,
                    &candidate_hash,
                )
                .unwrap()
            })
            .collect();
        let finalize = Finalize {
            candidate_hash: candidate_hash.clone(),
            accepts,
            recovery_id: None,
        };
        let finalize_hash = object(&store, "finalize", &finalize);
        let finalize_receipts = writers
            .iter()
            .map(|disk| {
                crate::journal::append(
                    disk,
                    RecordType::Finalize,
                    candidate.sequence,
                    &candidate_hash,
                    &finalize_hash,
                )
                .unwrap()
            })
            .collect();
        let publication = Publication {
            candidate_hash: candidate_hash.clone(),
            finalize,
            finalize_receipts,
            payload_receipts: Vec::new(),
            result,
            recovery: None,
        };
        let publication_hash = object(&store, "publication", &publication);
        let receipts = writers
            .iter()
            .map(|disk| {
                crate::journal::append(
                    disk,
                    RecordType::Publication,
                    candidate.sequence,
                    &candidate_hash,
                    &publication_hash,
                )
                .unwrap()
            })
            .collect();
        let seal = PublicationSeal {
            candidate_hash: candidate_hash.clone(),
            publication_hash,
            receipts,
        };
        verify_publication_seal(&candidate, &publication, &seal).unwrap();
        let seal_hash = object(&store, SEAL_KIND, &seal);
        let mut intent = fixture.authority.intent().unwrap();
        intent.operation_id = OperationId::generate().unwrap();
        intent.origin = Origin::Successor {
            config: intent.origin.config().clone(),
            predecessor_authority_hash: fixture.authority.hash().unwrap(),
        };
        intent.boundary = Some(PublishedBoundary {
            sequence: candidate.sequence,
            candidate_hash: candidate_hash.clone(),
            snapshot_hash: candidate.snapshot_hash.clone(),
        });
        intent.fence_sequence = candidate.sequence + 1;
        let transition = object(&store, INTENT_KIND, &intent);
        let mut members = json_codec::Map::new();
        for (disk, writer) in fixture.original.disks.iter().zip(&writers) {
            let fence = crate::journal::append(
                writer,
                RecordType::MembershipFence,
                intent.fence_sequence,
                &transition,
                &transition,
            )
            .unwrap();
            let bytes = fs::read(crate::journal::log_path(writer)).unwrap();
            let original = store.member_directory(disk, None);
            let config_bytes = fs::read(original.join("config.json")).unwrap();
            let mut seal = decode_seal(&fs::read(original.join("disk.json")).unwrap()).unwrap();
            seal.format = DiskSealFormat::MembershipGeneration {
                authority_hash: fixture.authority.hash().unwrap(),
            };
            let seal_bytes = canonical_json(&seal).unwrap();
            let directory = store.member_directory(disk, Some(&transition));
            private_dir(&directory).unwrap();
            fs::write(directory.join("config.json"), &config_bytes).unwrap();
            fs::write(directory.join("disk.json"), &seal_bytes).unwrap();
            fs::write(directory.join("journal.bin"), &bytes).unwrap();
            let member = ArchivedMember {
                journal: MembershipJournalInspection {
                    disk_id: disk.disk_id.clone(),
                    slot_id: disk.slot_id.clone(),
                    binding_generation: disk.binding_generation,
                    bytes: bytes.len() as u64,
                    tail_frame_hash: fence.frame_hash.clone(),
                    content_hash: content_hash(&bytes),
                },
                config_bytes_hash: content_hash(&config_bytes),
                seal_bytes_hash: content_hash(&seal_bytes),
                fence,
            };
            members.insert(disk.disk_id.clone(), json_codec::to_value(member).unwrap());
        }
        let root = ArchiveRoot {
            format: Format::FrozenControlMetadata,
            transition_hash: transition,
            members: store.map(members).unwrap(),
            publication_seals: store
                .map([(candidate_hash, seal_hash.into())].into_iter().collect())
                .unwrap(),
        };
        fs::write(store.path(ROOT_FILE), canonical_json(&root).unwrap()).unwrap();
        fs::write(
            store.path(BINDING_FILE),
            canonical_json(&ArchiveBinding {
                format: Format::FrozenControlMetadata,
                transition_hash: root.transition_hash.clone(),
            })
            .unwrap(),
        )
        .unwrap();
        Self {
            fixture,
            store,
            root,
            intent,
            candidate,
        }
    }
    fn verify(&self) -> Result<MembershipArchiveInspection> {
        verify_membership_control_archive(
            self.store.root.path(),
            &object_hash(ROOT_KIND, &self.root).unwrap(),
            &self.root.transition_hash,
        )
    }
}

#[test]
fn membership_abort_held_successor_archive_verifies_complete_history_without_live_roots() {
    let history = HistoricalArchive::new();
    for disk in &history.fixture.original.disks {
        fs::rename(&disk.root, disk.root.with_extension("unavailable")).unwrap();
    }
    let rejected = history
        .fixture
        .authority
        .intent()
        .unwrap()
        .target
        .disks
        .last()
        .unwrap()
        .root
        .clone();
    fs::rename(&rejected, rejected.with_extension("unavailable")).unwrap();
    let inspected = history.verify().unwrap();
    assert_eq!(inspected.original_members, 3);
    assert_eq!(inspected.published_candidates, 1);
    let (base, publications) = read_generation_base(
        history
            .store
            .root
            .private_child(history.store.root.path())
            .unwrap(),
        &object_hash(ROOT_KIND, &history.root).unwrap(),
        &history.root.transition_hash,
        &history.intent,
    )
    .unwrap();
    assert_eq!(base.sequence, history.candidate.sequence);
    assert_eq!(
        publications[&history.candidate.sequence],
        object_hash("candidate", &history.candidate).unwrap()
    );
    assert_eq!(
        base.operations[&history.candidate.operation_id].kind,
        "membership-add-cancelled"
    );
}

#[test]
fn membership_abort_held_archive_copy_preserves_core_and_accepts_alternate_witness() {
    let history = HistoricalArchive::new();
    history.verify().unwrap();
    let hash = history.fixture.authority.hash().unwrap();
    let original_bytes = fs::read(history.store.evidence_path(&hash).unwrap()).unwrap();
    history
        .store
        .install_evidence(&hash, &history.fixture.alternate.evidence_entries()[&hash])
        .unwrap();
    assert_eq!(
        fs::read(history.store.evidence_path(&hash).unwrap()).unwrap(),
        original_bytes
    );
    let copy = history.fixture.temp.path().join("independent-copy");
    private_dir(&copy).unwrap();
    let (destination, _lock) = ArchiveStore::admit(
        DiskRoot::open(&copy).unwrap(),
        &history.root.transition_hash,
    )
    .unwrap();
    verify_root_copying(
        &history.store,
        &history.root,
        &history.root.transition_hash,
        Some(&destination),
        &mut |_| Ok(()),
        None,
    )
    .unwrap();
    verify_root(&destination, &history.root, &history.root.transition_hash).unwrap();
    assert_eq!(
        fs::read(destination.evidence_path(&hash).unwrap()).unwrap(),
        original_bytes
    );
    // An explicitly chosen second valid witness still proves exactly G and F.
    fs::write(
        destination.evidence_path(&hash).unwrap(),
        canonical_json(&history.fixture.alternate.evidence_entries()[&hash]).unwrap(),
    )
    .unwrap();
    verify_root(&destination, &history.root, &history.root.transition_hash).unwrap();
}

#[test]
fn membership_abort_held_archive_missing_corrupt_extra_and_rebound_evidence_fail_closed() {
    let history = HistoricalArchive::new();
    history.verify().unwrap();
    let hash = history.fixture.authority.hash().unwrap();
    let path = history.store.evidence_path(&hash).unwrap();
    let original = fs::read(&path).unwrap();
    fs::rename(&path, history.fixture.temp.path().join("saved-evidence")).unwrap();
    assert!(history.verify().is_err());
    fs::write(&path, b"{}").unwrap();
    assert!(history.verify().is_err());
    let foreign = read_fixture();
    fs::write(
        &path,
        canonical_json(
            foreign
                .authority
                .evidence_entries()
                .values()
                .next()
                .unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(history.verify().is_err());
    fs::write(&path, &original).unwrap();
    let extra = history.store.evidence_path(&"f".repeat(64)).unwrap();
    fs::write(&extra, &original).unwrap();
    assert!(history.verify().is_err());
    fs::remove_file(extra).unwrap();
    history.verify().unwrap();
    let mut bytes = 0;
    verify_generation_archive_observed(
        history
            .store
            .root
            .private_child(history.store.root.path())
            .unwrap(),
        &object_hash(ROOT_KIND, &history.root).unwrap(),
        &history.root.transition_hash,
        &mut |amount| {
            bytes += amount;
            Ok(())
        },
        usize::MAX,
    )
    .unwrap();
    assert!(bytes >= original.len());
    assert!(
        verify_generation_archive_observed(
            history
                .store
                .root
                .private_child(history.store.root.path())
                .unwrap(),
            &object_hash(ROOT_KIND, &history.root).unwrap(),
            &history.root.transition_hash,
            &mut |_| Ok(()),
            0
        )
        .is_err()
    );
}

#[test]
fn membership_abort_held_archive_requires_every_original_journal_and_publication_object() {
    let history = HistoricalArchive::new();
    history.verify().unwrap();
    // The complete live F remains available, but may not mask damaged copied F.
    verify_membership_control_archive(
        &history.fixture.source,
        &history.fixture.reference.root_hash,
        &history.fixture.reference.transition_hash,
    )
    .unwrap();
    let original_journal = history
        .store
        .member_directory(&history.fixture.original.disks[0], None)
        .join("journal.bin");
    let saved = fs::read(&original_journal).unwrap();
    fs::write(&original_journal, &saved[..saved.len() - 1]).unwrap();
    assert!(history.verify().is_err());
    fs::write(&original_journal, saved).unwrap();
    let old_root = history.fixture.authority.archives().unwrap().remove(0).0;
    let seals = archive_seals(&history.store, &old_root).unwrap();
    let seal_hash = seals.values().next().unwrap();
    let path = history.store.object_path(SEAL_KIND, seal_hash).unwrap();
    let saved = fs::read(&path).unwrap();
    fs::remove_file(&path).unwrap();
    assert!(history.verify().is_err());
    fs::write(path, saved).unwrap();
    history.verify().unwrap();
}
