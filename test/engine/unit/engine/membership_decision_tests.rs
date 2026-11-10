use super::*;
use crate::engine::membership_transition::decision::DecisionKind;
use std::{
    fs,
    sync::{Arc, Barrier},
};

struct Fixture {
    _temp: tempfile::TempDir,
    root: DiskRoot,
    identity: MemberIdentity,
    transition: String,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("member");
        private_dir(&path).unwrap();
        let root = DiskRoot::open(&path).unwrap();
        let physical = root.directory_identity(root.path()).unwrap();
        let identity = json_codec::from_value(json_codec::json!({
            "pool_id": "pool_00000000000000000000000000000001",
            "disk_id": "disk_00000000000000000000000000000001",
            "slot_id": "P01", "binding_generation": "1",
            "device": physical.device.to_string(), "inode": physical.inode.to_string(),
            "simulation": true
        }))
        .unwrap();
        Self {
            _temp: temp,
            root,
            identity,
            transition: "1".repeat(64),
        }
    }
    fn open(&self) -> DecisionStore {
        DecisionStore::open(&self.root, &self.identity, &self.transition, true).unwrap()
    }
    fn path(&self) -> PathBuf {
        decision_directory(&self.root, &self.transition).join(DECISION_FILE)
    }
    fn record(&self, choice: usize) -> DecisionRecord {
        let choice = match choice {
            0 => Choice::Joint {
                proposal_hash: "2".repeat(64),
                acceptance_hash: "3".repeat(64),
            },
            1 => Choice::Abort {
                core_hash: "4".repeat(64),
                acceptance_hash: "5".repeat(64),
            },
            2 => Choice::Abort {
                core_hash: "6".repeat(64),
                acceptance_hash: "7".repeat(64),
            },
            3 => Choice::Joint {
                proposal_hash: "8".repeat(64),
                acceptance_hash: "9".repeat(64),
            },
            _ => unreachable!(),
        };
        DecisionRecord {
            format: DecisionFormat::Decision,
            transition_hash: self.transition.clone(),
            identity: self.identity.clone(),
            choice,
        }
    }
}
fn select(store: &DecisionStore, record: &DecisionRecord) -> Result<DecisionReceipt> {
    store.select(
        record,
        true,
        #[cfg(feature = "fault-injection")]
        None,
    )
}
fn changed_identity(identity: &MemberIdentity, field: &str, value: &str) -> MemberIdentity {
    let mut encoded = json_codec::to_value(identity).unwrap();
    encoded[field] = json_codec::to_value(value).unwrap();
    json_codec::from_value(encoded).unwrap()
}

const DECISION_KINDS: [(DecisionKind, &str, &str); 8] = [
    (
        DecisionKind::BackupRegistryPlacement,
        "BackupRegistryPlacement",
        "core_hash",
    ),
    (DecisionKind::Readmission, "Readmission", "proposal_hash"),
    (DecisionKind::Joint, "Joint", "proposal_hash"),
    (DecisionKind::Abort, "Abort", "core_hash"),
    (DecisionKind::TerminalResume, "TerminalResume", "core_hash"),
    (
        DecisionKind::TerminalChainReadmission,
        "TerminalChainReadmission",
        "core_hash",
    ),
    (
        DecisionKind::TerminalChainIoAdmission,
        "TerminalChainIoAdmission",
        "core_hash",
    ),
    (
        DecisionKind::TerminalResumeCancel,
        "TerminalResumeCancel",
        "core_hash",
    ),
];

fn choose_kind(
    store: &DecisionStore,
    kind: DecisionKind,
    install: bool,
) -> Result<DecisionReceipt> {
    store.receipt(
        kind,
        &"2".repeat(64),
        &"3".repeat(64),
        install,
        #[cfg(feature = "fault-injection")]
        None,
    )
}

#[test]
fn membership_decision_kinds_keep_exact_native_fields_and_receipt_binding() {
    let fixture = Fixture::new();
    let value = "2".repeat(64);
    let acceptance = "3".repeat(64);
    let changed = "4".repeat(64);
    let other_identity = changed_identity(&fixture.identity, "binding_generation", "2");
    for (kind, tag, field) in DECISION_KINDS {
        let receipt = DecisionReceipt::plan(
            kind,
            &fixture.transition,
            &fixture.identity,
            &value,
            &acceptance,
        )
        .unwrap();
        // Independent wire fixture: a shared constructor must not rename the
        // two proposal fields, collapse kinds or change the native hash input.
        let mut choice = json_codec::json!({
            "kind": tag,
            "acceptance_hash": acceptance,
        });
        choice[field] = json_codec::to_value(&value).unwrap();
        let record = json_codec::json!({
            "format": "fenced-membership-decision",
            "transition_hash": fixture.transition,
            "identity": fixture.identity,
            "choice": choice,
        });
        assert_eq!(
            canonical_json(&receipt.record).unwrap(),
            canonical_json(&record).unwrap(),
            "{kind:?}",
        );
        assert_eq!(
            receipt.record_hash,
            object_hash(DECISION_KIND, &record).unwrap(),
            "{kind:?}",
        );
        let decoded: DecisionReceipt = decode_strict(&canonical_json(&receipt).unwrap()).unwrap();
        decoded
            .verify(
                kind,
                &fixture.transition,
                &fixture.identity,
                &value,
                &acceptance,
            )
            .unwrap();
        for (transition, identity, value, acceptance) in [
            (&changed, &fixture.identity, &value, &acceptance),
            (&fixture.transition, &other_identity, &value, &acceptance),
            (
                &fixture.transition,
                &fixture.identity,
                &changed,
                &acceptance,
            ),
            (&fixture.transition, &fixture.identity, &value, &changed),
        ] {
            assert_eq!(
                decoded
                    .verify(kind, transition, identity, value, acceptance)
                    .unwrap_err()
                    .code(),
                ErrorCode::LogFork,
                "{kind:?}",
            );
        }
        let mut forged = decoded;
        forged.record_hash = changed.clone();
        assert_eq!(
            forged
                .verify(
                    kind,
                    &fixture.transition,
                    &fixture.identity,
                    &value,
                    &acceptance,
                )
                .unwrap_err()
                .code(),
            ErrorCode::LogFork,
        );
    }
}

#[test]
fn membership_decision_kinds_reject_malformed_hashes_before_local_selection() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let value = "2".repeat(64);
    let acceptance = "3".repeat(64);
    for (kind, _, _) in DECISION_KINDS {
        for malformed in [
            String::new(),
            "1".repeat(63),
            "A".repeat(64),
            "g".repeat(64),
        ] {
            for (transition, value, acceptance) in [
                (&malformed, &value, &acceptance),
                (&fixture.transition, &malformed, &acceptance),
                (&fixture.transition, &value, &malformed),
            ] {
                assert_eq!(
                    DecisionReceipt::plan(kind, transition, &fixture.identity, value, acceptance,)
                        .unwrap_err()
                        .code(),
                    ErrorCode::ObjectCorrupt,
                );
            }
            for (value, acceptance) in [(&malformed, &acceptance), (&value, &malformed)] {
                assert_eq!(
                    store
                        .receipt(
                            kind,
                            value,
                            acceptance,
                            true,
                            #[cfg(feature = "fault-injection")]
                            None,
                        )
                        .unwrap_err()
                        .code(),
                    ErrorCode::ObjectCorrupt,
                );
                assert!(!fixture.path().exists());
            }
        }
    }
}

#[test]
fn membership_decision_every_kind_remains_disjoint_and_requires_actual_readback() {
    for (chosen, _, _) in DECISION_KINDS {
        let fixture = Fixture::new();
        let store = fixture.open();
        assert_eq!(
            choose_kind(&store, chosen, false).unwrap_err().code(),
            ErrorCode::RecoveryRequired,
        );
        assert!(!fixture.path().exists());
        let planned = DecisionReceipt::plan(
            chosen,
            &fixture.transition,
            &fixture.identity,
            &"2".repeat(64),
            &"3".repeat(64),
        )
        .unwrap();
        assert!(store.verify_receipt(&planned, false).is_err());
        assert!(!fixture.path().exists());
        let selected = choose_kind(&store, chosen, true).unwrap();
        assert_eq!(selected, planned);
        let original = fs::read(fixture.path()).unwrap();
        let reopened = fixture.open();
        for (requested, _, _) in DECISION_KINDS {
            for install in [false, true] {
                let result = choose_kind(&reopened, requested, install);
                if requested == chosen {
                    assert_eq!(result.unwrap(), selected);
                } else {
                    assert_eq!(result.unwrap_err().code(), ErrorCode::LogFork);
                }
                assert_eq!(fs::read(fixture.path()).unwrap(), original);
            }
        }
        reopened.verify_receipt(&selected, false).unwrap();
        fs::write(fixture.path(), b"{broken").unwrap();
        for synchronize in [false, true] {
            assert!(reopened.verify_receipt(&selected, synchronize).is_err());
            assert_eq!(fs::read(fixture.path()).unwrap(), b"{broken");
        }
        fs::remove_file(fixture.path()).unwrap();
        assert!(reopened.verify_receipt(&selected, false).is_err());
        assert!(!fixture.path().exists());
    }
}

#[test]
fn membership_decision_all_joint_abort_orders_share_one_immutable_slot() {
    for first in 0..4 {
        for second in 0..4 {
            let fixture = Fixture::new();
            let store = fixture.open();
            let receipt = select(&store, &fixture.record(first)).unwrap();
            let bytes = fs::read(fixture.path()).unwrap();
            let result = select(&store, &fixture.record(second));
            if first == second {
                assert_eq!(result.unwrap(), receipt);
            } else {
                assert_eq!(result.unwrap_err().code(), ErrorCode::LogFork);
            }
            assert_eq!(fs::read(fixture.path()).unwrap(), bytes);
            // Reopening retains the same physical intent slot, not P/G/outcome.
            let reopened = fixture.open();
            assert_eq!(select(&reopened, &fixture.record(first)).unwrap(), receipt);
            assert_eq!(reopened.root.path(), store.root.path());
        }
    }
}

#[test]
fn membership_decision_real_concurrent_choices_have_exactly_one_winner() {
    for (left, right) in [(0, 1), (1, 0), (1, 2), (0, 3), (0, 0), (1, 1)] {
        let fixture = Fixture::new();
        let first = fixture.open();
        let second = fixture.open();
        let barrier = Arc::new(Barrier::new(2));
        let a = fixture.record(left);
        let b = fixture.record(right);
        let (one, two) = std::thread::scope(|scope| {
            let barrier_a = Arc::clone(&barrier);
            let one = scope.spawn(move || {
                barrier_a.wait();
                select(&first, &a)
            });
            let two = scope.spawn(move || {
                barrier.wait();
                select(&second, &b)
            });
            (one.join().unwrap(), two.join().unwrap())
        });
        assert_eq!(
            usize::from(one.is_ok()) + usize::from(two.is_ok()),
            if left == right { 2 } else { 1 }
        );
        let receipt = one.or(two).unwrap();
        fixture.open().verify_receipt(&receipt, true).unwrap();
    }
}

#[test]
fn membership_decision_corrupt_unknown_truncated_or_unbound_is_never_empty() {
    for malformed in [
        b"".to_vec(),
        b"{".to_vec(),
        b"null".to_vec(),
        br#"{"format":"unknown"}"#.to_vec(),
    ] {
        let fixture = Fixture::new();
        let store = fixture.open();
        fs::write(fixture.path(), &malformed).unwrap();
        for choice in 0..2 {
            assert!(select(&store, &fixture.record(choice)).is_err());
        }
        assert!(
            DecisionStore::open(&fixture.root, &fixture.identity, &fixture.transition, true)
                .is_err()
        );
        assert_eq!(fs::read(fixture.path()).unwrap(), malformed);
    }
    let fixture = Fixture::new();
    let store = fixture.open();
    let record = fixture.record(0);
    for malformed in [
        [canonical_json(&record).unwrap(), b"suffix".to_vec()].concat(),
        [
            canonical_json(&record).unwrap(),
            canonical_json(&record).unwrap(),
        ]
        .concat(),
    ] {
        fs::write(fixture.path(), &malformed).unwrap();
        assert!(select(&store, &record).is_err());
        assert_eq!(fs::read(fixture.path()).unwrap(), malformed);
    }
    fs::remove_file(store.root.path().join(BINDING_FILE)).unwrap();
    assert!(
        DecisionStore::inspect_preparation(&fixture.root, &fixture.identity, &fixture.transition)
            .is_err()
    );
    assert!(
        DecisionStore::open(&fixture.root, &fixture.identity, &fixture.transition, true).is_err()
    );
}

#[test]
fn membership_decision_intent_identity_binding_and_physical_root_must_match() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let original = fixture.record(0);
    let mut foreign = original.clone();
    foreign.transition_hash = "a".repeat(64);
    assert!(select(&store, &foreign).is_err());
    for (field, value) in [
        ("disk_id", "disk_other"),
        ("slot_id", "P02"),
        ("binding_generation", "2"),
        ("inode", "0"),
        ("pool_id", "pool_other"),
    ] {
        foreign = original.clone();
        foreign.identity = changed_identity(&original.identity, field, value);
        assert!(select(&store, &foreign).is_err());
        assert!(
            DecisionStore::open(&fixture.root, &foreign.identity, &fixture.transition, true)
                .is_err()
        );
        assert!(!fixture.path().exists());
    }
    let bytes = canonical_json(&foreign).unwrap();
    fs::write(fixture.path(), &bytes).unwrap();
    assert!(select(&store, &original).is_err());
    assert_eq!(fs::read(fixture.path()).unwrap(), bytes);
    assert!(DecisionStore::open(&fixture.root, &fixture.identity, "../other", true).is_err());
    assert!(DecisionStore::open(&fixture.root, &fixture.identity, &"A".repeat(64), true).is_err());
}

#[test]
fn membership_decision_neutral_preparation_resumes_but_temporary_records_never_vote() {
    let fixture = Fixture::new();
    let path = decision_directory(&fixture.root, &fixture.transition);
    fixture.root.private_dir(&path).unwrap();
    DecisionStore::inspect_preparation(&fixture.root, &fixture.identity, &fixture.transition)
        .unwrap();
    let store = fixture.open();
    let record = fixture.record(1);
    let staged = store
        .root
        .path()
        .join(format!(".state-atomic-{}.tmp", "a".repeat(32)));
    fs::write(&staged, canonical_json(&record).unwrap()).unwrap();
    let fabricated = DecisionReceipt {
        record_hash: object_hash(DECISION_KIND, &record).unwrap(),
        record,
    };
    assert!(store.verify_receipt(&fabricated, false).is_err());
    store.require_joint_allowed().unwrap();
    select(&store, &fixture.record(0)).unwrap();
    assert_eq!(
        fs::read(staged).unwrap(),
        canonical_json(&fabricated.record).unwrap()
    );
}

#[test]
fn membership_decision_legacy_singleton_cannot_become_a_second_voting_route() {
    for before in [true, false] {
        let fixture = Fixture::new();
        let store = fixture.open();
        let receipt = if before {
            Some(select(&store, &fixture.record(1)).unwrap())
        } else {
            None
        };
        let legacy = fixture
            .root
            .path()
            .join("metadata/membership-pending")
            .join(&fixture.transition)
            .join("joint");
        fixture.root.private_dir(&legacy).unwrap();
        let root = fixture.root.private_child(&legacy).unwrap();
        let mut journal =
            PendingJournal::open_joint_at(&root, &legacy, 1, &"2".repeat(64), &"3".repeat(64))
                .unwrap();
        let disk = Config::development([fixture.root.path().to_path_buf()])
            .unwrap()
            .disks
            .remove(0);
        journal.synchronize(&disk).unwrap();
        let bytes = fs::read(legacy.join("journal.bin")).unwrap();
        assert!(
            DecisionStore::open(&fixture.root, &fixture.identity, &fixture.transition, true)
                .is_err()
        );
        assert!(select(&store, &fixture.record(0)).is_err());
        if let Some(receipt) = receipt {
            assert!(store.verify_receipt(&receipt, true).is_err());
        } else {
            assert!(!fixture.path().exists());
        }
        assert_eq!(fs::read(legacy.join("journal.bin")).unwrap(), bytes);
    }
}

#[test]
fn membership_decision_portable_receipt_is_not_actual_local_selection() {
    let fixture = Fixture::new();
    let store = fixture.open();
    let record = fixture.record(0);
    let fake = DecisionReceipt {
        record_hash: object_hash(DECISION_KIND, &record).unwrap(),
        record: record.clone(),
    };
    fake.verify(
        DecisionKind::Joint,
        &fixture.transition,
        &fixture.identity,
        &"2".repeat(64),
        &"3".repeat(64),
    )
    .unwrap();
    for synchronize in [false, true] {
        assert!(store.verify_receipt(&fake, synchronize).is_err());
    }
    let real = select(&store, &record).unwrap();
    assert_eq!(real, fake);
    let mut wrong = real.clone();
    wrong.record_hash = "f".repeat(64);
    assert!(store.verify_receipt(&wrong, true).is_err());
    assert!(
        wrong
            .verify(
                DecisionKind::Joint,
                &fixture.transition,
                &fixture.identity,
                &"2".repeat(64),
                &"3".repeat(64)
            )
            .is_err()
    );
    fs::remove_file(fixture.path()).unwrap();
    assert!(store.verify_receipt(&real, false).is_err());
    assert!(!fixture.path().exists());
}

#[cfg(feature = "fault-injection")]
#[test]
fn membership_decision_sync_noreplace_and_lost_reply_failures_never_issue_receipts() {
    for choice in 0..2 {
        for fault in [
            DecisionFault::StagingWrite,
            DecisionFault::PreparedFileSync,
            DecisionFault::NoReplace,
            DecisionFault::PublishedReply,
            DecisionFault::FileSync,
            DecisionFault::DirectorySync,
        ] {
            let fixture = Fixture::new();
            let store = fixture.open();
            let record = fixture.record(choice);
            assert!(store.select(&record, true, Some(fault)).is_err());
            let published = matches!(
                fault,
                DecisionFault::PublishedReply
                    | DecisionFault::FileSync
                    | DecisionFault::DirectorySync
            );
            assert_eq!(fixture.path().exists(), published, "{fault:?}");
            if published {
                assert!(select(&store, &fixture.record(1 - choice)).is_err());
            }
            let recovered = fixture.open();
            let receipt = select(&recovered, &record).unwrap();
            for retry_fault in [DecisionFault::FileSync, DecisionFault::DirectorySync] {
                assert!(recovered.select(&record, true, Some(retry_fault)).is_err());
                assert_eq!(select(&recovered, &record).unwrap(), receipt);
            }
            assert!(select(&recovered, &fixture.record(1 - choice)).is_err());
        }
    }
}

#[test]
fn membership_decision_abort_visibility_vetoes_joint_without_claiming_abort_authority() {
    let fixture = Fixture::new();
    let store = fixture.open();
    store.require_joint_allowed().unwrap();
    select(&store, &fixture.record(1)).unwrap();
    assert_eq!(
        store.require_joint_allowed().unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
}

#[test]
fn membership_decision_prepared_noreplace_collision_preserves_both_choice_orders() {
    for first in 0..2 {
        let fixture = Fixture::new();
        let store = fixture.open();
        let directory = store.root.private_directory(store.root.path()).unwrap();
        let name = EntryName::new(DECISION_FILE).unwrap();
        let prepared = AtomicFile::stage(&directory, &name)
            .unwrap()
            .prepare(&canonical_json(&fixture.record(first)).unwrap())
            .unwrap();
        let winner = select(&store, &fixture.record(1 - first)).unwrap();
        let bytes = fs::read(fixture.path()).unwrap();
        assert!(prepared.create().is_err());
        assert_eq!(fs::read(fixture.path()).unwrap(), bytes);
        assert!(select(&store, &fixture.record(first)).is_err());
        assert_eq!(select(&store, &fixture.record(1 - first)).unwrap(), winner);
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn membership_decision_unknown_and_published_outcomes_reopen_before_confirming() {
    for choice in 0..2 {
        for fault in [
            DecisionFault::UnknownPublication,
            DecisionFault::PublishedDurability,
        ] {
            let fixture = Fixture::new();
            let store = fixture.open();
            let record = fixture.record(choice);
            let receipt = store.select(&record, true, Some(fault)).unwrap();
            assert_eq!(
                fs::read(fixture.path()).unwrap(),
                canonical_json(&record).unwrap()
            );
            assert_eq!(receipt.record, record);
            assert_eq!(
                receipt.record_hash,
                object_hash(DECISION_KIND, &record).unwrap()
            );
            fixture.open().verify_receipt(&receipt, true).unwrap();
            assert!(select(&store, &fixture.record(1 - choice)).is_err());
        }
    }
}

#[test]
fn membership_decision_partial_neutral_binding_resumes_without_promoting_staging() {
    for valid in [true, false] {
        let fixture = Fixture::new();
        let directory = decision_directory(&fixture.root, &fixture.transition);
        fixture.root.private_dir(&directory).unwrap();
        let binding = Binding {
            format: SlotFormat::Slot,
            transition_hash: fixture.transition.clone(),
            identity: fixture.identity.clone(),
        };
        let bytes = canonical_json(&binding).unwrap();
        let prefix = if valid {
            bytes[..bytes.len() / 2].to_vec()
        } else {
            b"unrelated".to_vec()
        };
        let temporary = directory.join(format!(".state-atomic-{}.tmp", "b".repeat(32)));
        fs::write(&temporary, &prefix).unwrap();
        let admission = DecisionStore::inspect_preparation(
            &fixture.root,
            &fixture.identity,
            &fixture.transition,
        );
        let opened =
            DecisionStore::open(&fixture.root, &fixture.identity, &fixture.transition, true);
        if valid {
            admission.unwrap();
            let store = opened.unwrap();
            assert!(!fixture.path().exists());
            select(&store, &fixture.record(0)).unwrap();
        } else {
            assert!(admission.is_err());
            assert!(opened.is_err());
            assert!(!directory.join(BINDING_FILE).exists());
        }
        assert_eq!(fs::read(temporary).unwrap(), prefix);
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn membership_decision_uncertain_publication_still_requires_both_confirmation_barriers() {
    for choice in 0..2 {
        for fault in [
            DecisionFault::UnknownPublicationFileSync,
            DecisionFault::PublishedDurabilityDirectorySync,
        ] {
            let fixture = Fixture::new();
            let store = fixture.open();
            let record = fixture.record(choice);
            assert!(store.select(&record, true, Some(fault)).is_err());
            assert_eq!(
                fs::read(fixture.path()).unwrap(),
                canonical_json(&record).unwrap()
            );
            assert!(select(&store, &fixture.record(1 - choice)).is_err());
            let receipt = select(&fixture.open(), &record).unwrap();
            assert_eq!(receipt.record, record);
        }
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn membership_decision_unpublished_failure_allows_either_choice_but_never_replaces_winner() {
    for first in 0..2 {
        for fault in [
            DecisionFault::StagingWrite,
            DecisionFault::PreparedFileSync,
            DecisionFault::NoReplace,
        ] {
            let fixture = Fixture::new();
            let store = fixture.open();
            assert!(
                store
                    .select(&fixture.record(first), true, Some(fault))
                    .is_err()
            );
            assert!(!fixture.path().exists());
            let winner = select(&fixture.open(), &fixture.record(1 - first)).unwrap();
            let bytes = fs::read(fixture.path()).unwrap();
            assert!(select(&store, &fixture.record(first)).is_err());
            assert_eq!(select(&store, &fixture.record(1 - first)).unwrap(), winner);
            assert_eq!(fs::read(fixture.path()).unwrap(), bytes);
        }
    }
}
