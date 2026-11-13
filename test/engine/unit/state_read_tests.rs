//! Authenticated disk bytes are raw until complete reference admission succeeds.
use super::*;
use crate::journal::{load_snapshot, store_snapshot};

#[derive(Clone, Copy, Debug)]
enum Corruption {
    RepositoryKey,
    HistoryKey,
    MissingRepository,
    MissingHistory,
    MissingMain,
    MissingHead,
    MissingParent,
    Cycle,
    MultipleParents,
    OrphanCommit,
    OrphanVersion,
    OrphanContentState,
    MissingPlacement,
    OrphanFileSelection,
    OrphanFileNote,
    OrphanDirectorySelection,
    OrphanContext,
    OrphanVersionNote,
}

#[test]
fn stored_snapshots_reject_broken_reference_closure_before_engine_admission() {
    use Corruption::*;
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 3);
    save(&mut engine, &repo.main_branch_id, "file.txt", b"body");
    let original = engine.state.as_ref().clone();
    let branch = &original.branches[&repo.main_branch_id];
    let head = branch.head_commit_id.clone();
    let root = original.histories[&repo.history_tree_id]
        .root_commit_id
        .clone();
    let version = original.commits[&head]
        .entries
        .iter()
        .find(|entry| entry.kind == NamespaceEntryKind::File)
        .unwrap()
        .version_id
        .clone();
    for corruption in [
        RepositoryKey,
        HistoryKey,
        MissingRepository,
        MissingHistory,
        MissingMain,
        MissingHead,
        MissingParent,
        Cycle,
        MultipleParents,
        OrphanCommit,
        OrphanVersion,
        OrphanContentState,
        MissingPlacement,
        OrphanFileSelection,
        OrphanFileNote,
        OrphanDirectorySelection,
        OrphanContext,
        OrphanVersionNote,
    ] {
        let mut state = original.clone();
        match corruption {
            RepositoryKey => {
                state
                    .repositories
                    .get_mut(&repo.repository_id)
                    .unwrap()
                    .repository_id = new_id("repo").unwrap()
            }
            HistoryKey => {
                state
                    .histories
                    .get_mut(&repo.history_tree_id)
                    .unwrap()
                    .history_tree_id = new_id("history").unwrap()
            }
            MissingRepository => {
                state.repositories.remove(&repo.repository_id);
            }
            MissingHistory => {
                state.histories.remove(&repo.history_tree_id);
            }
            MissingMain => {
                state
                    .repositories
                    .get_mut(&repo.repository_id)
                    .unwrap()
                    .main_branch_id = new_id("br").unwrap()
            }
            MissingHead => {
                state
                    .branches
                    .get_mut(&repo.main_branch_id)
                    .unwrap()
                    .head_commit_id = new_id("commit").unwrap()
            }
            MissingParent => {
                state.commits.get_mut(&head).unwrap().parent_commit_ids =
                    vec![new_id("commit").unwrap()]
            }
            Cycle => state.commits.get_mut(&head).unwrap().parent_commit_ids = vec![head.clone()],
            MultipleParents => {
                state.commits.get_mut(&head).unwrap().parent_commit_ids =
                    vec![root.clone(), root.clone()]
            }
            OrphanCommit => {
                let mut commit = state.commits[&head].clone();
                commit.commit_id = new_id("commit").unwrap();
                commit.history_tree_id = new_id("history").unwrap();
                state.commits.insert(commit.commit_id.clone(), commit);
            }
            OrphanVersion => {
                let mut orphan = state.versions[&version].clone();
                orphan.version_id = new_id("ver").unwrap();
                orphan.file_id = new_id("file").unwrap();
                orphan.history_tree_id = new_id("history").unwrap();
                let mut placement = state.placements[&version].clone();
                placement.version_id = orphan.version_id.clone();
                state
                    .placements
                    .insert(orphan.version_id.clone(), placement);
                state
                    .content_states
                    .insert(orphan.version_id.clone(), ContentState::Available);
                state.versions.insert(orphan.version_id.clone(), orphan);
            }
            OrphanContentState => {
                state
                    .content_states
                    .insert(new_id("ver").unwrap(), ContentState::Available);
            }
            MissingPlacement => {
                state.placements.remove(&version);
            }
            OrphanFileSelection => {
                state
                    .file_selections
                    .insert(new_id("file").unwrap(), new_id("br").unwrap());
            }
            OrphanFileNote => {
                state
                    .file_notes
                    .insert(new_id("file").unwrap(), "note".into());
            }
            OrphanDirectorySelection => {
                state
                    .directory_selections
                    .insert(new_id("entry").unwrap(), new_id("dbr").unwrap());
            }
            OrphanContext => {
                let context = FileContext {
                    node_id: None,
                    selection_generation: 0,
                    branch_id: repo.main_branch_id.clone(),
                    inherited_from: None,
                };
                state
                    .file_contexts
                    .insert(format!("{}/file.txt", new_id("repo").unwrap()), context);
            }
            OrphanVersionNote => {
                state
                    .version_notes
                    .insert(new_id("ver").unwrap(), "note".into());
            }
        }
        // The malformed graph has valid object hashes: this exercises reference
        // admission rather than merely rejecting a changed page checksum.
        let hash = store_snapshot(&fixture.config.disks, &state).unwrap();
        let error = load_snapshot(&fixture.config.disks[0], &hash).unwrap_err();
        assert_eq!(
            error.code(),
            ErrorCode::StateInvalid,
            "{corruption:?}: {error}"
        );
        if matches!(corruption, OrphanVersion) {
            assert_eq!(
                error.message(),
                "snapshot versions contains unowned records"
            );
        }
        assert!(
            crate::publication_evidence::verified_snapshot(&fixture.config.disks, &hash).is_err(),
            "{corruption:?}"
        );
    }
    let mut sparse = original.clone();
    sparse.content_states.remove(&version);
    let hash = store_snapshot(&fixture.config.disks, &sparse).unwrap();
    let admitted = load_snapshot(&fixture.config.disks[0], &hash).unwrap();
    assert_eq!(
        admitted, sparse,
        "Available is an intentional sparse default, not a missing reference"
    );
    assert!(!admitted.content_states.contains_key(&version));
    assert_eq!(store_snapshot(&[], &admitted).unwrap(), hash);
    drop(engine);
    let reopened = Engine::open(&fixture.root).unwrap();
    assert_eq!(reopened.state.as_ref(), &original);
}

#[test]
fn merge_ancestry_still_rejects_user_supplied_foreign_or_missing_endpoints() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 3);
    let head = engine.state.branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let error = engine
        .add_merge_link(
            &repo.main_branch_id,
            &new_id("commit").unwrap(),
            &repo.main_branch_id,
            &head,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::CommitNotFound);
    let error = engine
        .add_merge_link(
            &new_id("br").unwrap(),
            &head,
            &repo.main_branch_id,
            &head,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::BranchNotFound);
}

#[test]
fn stored_snapshot_rejects_file_identity_reused_across_histories() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let first = repository(&mut engine, 3);
    save(&mut engine, &first.main_branch_id, "first.txt", b"first");
    let second = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("second").unwrap(),
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    save(&mut engine, &second.main_branch_id, "second.txt", b"second");
    let mut state = engine.state.as_ref().clone();
    let first_file = state
        .versions
        .values()
        .find(|version| version.history_tree_id == first.history_tree_id)
        .unwrap()
        .file_id
        .clone();
    let second_version = state
        .versions
        .values()
        .find(|version| version.history_tree_id == second.history_tree_id)
        .unwrap()
        .version_id
        .clone();
    state.versions.get_mut(&second_version).unwrap().file_id = first_file;
    let hash = store_snapshot(&fixture.config.disks, &state).unwrap();
    let error = load_snapshot(&fixture.config.disks[0], &hash).unwrap_err();
    assert_eq!(error.code(), ErrorCode::StateInvalid);
    assert_eq!(
        error.message(),
        "snapshot stable file identity crosses histories"
    );
}

#[derive(Clone, Copy, Debug)]
enum VersionIdentityCorruption {
    CreationTimeOffset,
    CreationTimeInvalidDate,
    CreationName,
}

impl VersionIdentityCorruption {
    fn apply(self, version: &mut FileVersion) {
        match self {
            Self::CreationTimeOffset => {
                version.created_at = "2026-10-27T00:00:00+00:00".into();
            }
            Self::CreationTimeInvalidDate => {
                version.created_at = "2026-02-30T00:00:00Z".into();
            }
            Self::CreationName => version.creation_name = "../outside.txt".into(),
        }
    }
}

fn assert_version_identity_admission_error(error: Error) {
    assert_eq!(error.code(), ErrorCode::StateInvalid);
    let Error::Context { source, .. } = error else {
        panic!("snapshot admission must retain its version-validation source");
    };
    assert_eq!(source.code(), ErrorCode::HistoryPackageInvalid);
    assert!(
        source
            .message()
            .starts_with("invalid version storage identity:"),
        "{source}"
    );
}

#[test]
fn stored_snapshots_admit_all_version_identities_before_per_file_sequence_use() {
    use VersionIdentityCorruption::*;
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 3);
    save(
        &mut engine,
        &repo.main_branch_id,
        "matching.txt",
        b"matching",
    );
    save(
        &mut engine,
        &repo.main_branch_id,
        "unrelated.txt",
        b"unrelated",
    );
    let entries = engine.list_tree(&repo.main_branch_id).unwrap();
    let matching = entries
        .iter()
        .find(|entry| entry.path() == "matching.txt")
        .unwrap();
    let unrelated = entries
        .iter()
        .find(|entry| entry.path() == "unrelated.txt")
        .unwrap();
    let original = engine.state.as_ref().clone();

    for target in [matching, unrelated] {
        for status in [
            ContentState::Available,
            ContentState::Purged,
            ContentState::Migrated,
        ] {
            let mut baseline = original.clone();
            baseline
                .content_states
                .insert(target.version_id.clone(), status);
            let hash = store_snapshot(&fixture.config.disks, &baseline).unwrap();
            let admitted =
                crate::publication_evidence::verified_snapshot(&fixture.config.disks, &hash)
                    .unwrap()
                    .into_state();
            assert_eq!(admitted, baseline, "{} {status:?}", target.path());
            let next = storage_contract::VersionId::generate().unwrap();
            let id = crate::container_storage::allocate_version_storage_id(
                &admitted, &matching.file_id, next,
            )
            .unwrap();
            assert!(admitted.versions.values().all(|version| version.storage_id != id));

            for corruption in [
                CreationTimeOffset,
                CreationTimeInvalidDate,
                CreationName,
            ] {
                let mut state = baseline.clone();
                corruption.apply(state.versions.get_mut(&target.version_id).unwrap());
                // Rehash the changed metadata, so checksum rejection cannot hide
                // a missing semantic boundary for either matching or unrelated rows.
                let hash = store_snapshot(&fixture.config.disks, &state).unwrap();
                let snapshot: crate::journal::Snapshot =
                    crate::journal::load_object(&fixture.config.disks[0], "snapshot", &hash)
                        .unwrap();
                let mut evidence = Vec::new();
                for disk in &fixture.config.disks {
                    for (kind, hash) in std::iter::once(("snapshot", hash.as_str())).chain(
                        crate::journal::map_addresses(disk, &snapshot.maps["versions"])
                            .iter()
                            .map(|(kind, hash)| (*kind, hash.as_str())),
                    ) {
                        let path = crate::journal::object_path(disk, kind, hash);
                        evidence.push((path.clone(), fs::read(path).unwrap()));
                    }
                }
                assert_version_identity_admission_error(
                    load_snapshot(&fixture.config.disks[0], &hash).unwrap_err(),
                );
                let error = match crate::publication_evidence::verified_snapshot(
                    &fixture.config.disks,
                    &hash,
                ) {
                    Ok(_) => panic!(
                        "{} {status:?} {corruption:?} entered engine state",
                        target.path()
                    ),
                    Err(error) => error,
                };
                assert_version_identity_admission_error(error);
                for (path, bytes) in evidence {
                    assert_eq!(fs::read(path).unwrap(), bytes, "rejected evidence changed");
                }
            }
        }
    }
    assert_eq!(engine.state.as_ref(), &original);
    drop(engine);
    let reopened = Engine::open(&fixture.root).unwrap();
    assert_eq!(reopened.state.as_ref(), &original);
    let id = crate::container_storage::allocate_version_storage_id(
        reopened.state(),
        &matching.file_id,
        storage_contract::VersionId::generate().unwrap(),
    )
    .unwrap();
    assert!(reopened.state().versions.values().all(|version| version.storage_id != id));
}

#[test]
fn stored_event_sequences_admit_duplicates_and_reject_rehashed_invalid_sequences() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 3);
    save(&mut engine, &repo.main_branch_id, "file.txt", b"body");
    let original = engine.state.as_ref().clone();
    assert!(original.events.len() >= 2);
    assert!(
        original
            .events
            .iter()
            .map(|event| event.sequence)
            .is_sorted()
    );
    let mut duplicate = original.clone();
    duplicate
        .events
        .push(original.events.iter().next_back().unwrap().clone());
    assert_eq!(
        duplicate.events.iter().next_back().unwrap().sequence,
        duplicate.sequence
    );
    let duplicate_hash = store_snapshot(&fixture.config.disks, &duplicate).unwrap();
    for disk in &fixture.config.disks {
        assert_eq!(load_snapshot(disk, &duplicate_hash).unwrap(), duplicate);
    }
    assert_eq!(
        crate::publication_evidence::verified_snapshot(&fixture.config.disks, &duplicate_hash)
            .unwrap()
            .into_state(),
        duplicate
    );
    let mut decreasing = original.clone();
    decreasing.events = original.events.iter().rev().cloned().collect();
    assert!(
        !decreasing
            .events
            .iter()
            .map(|event| event.sequence)
            .is_sorted()
    );
    let mut future = original.clone();
    let last = future.events.len() - 1;
    future.events[last].sequence = future.sequence + 1;
    assert!(future.events.iter().map(|event| event.sequence).is_sorted());
    for (invalid, message) in [
        (decreasing, "snapshot event sequences are not nondecreasing"),
        (future, "snapshot event sequence exceeds published sequence"),
    ] {
        // Recompute every object address so a raw digest mismatch cannot hide
        // semantic admission failures. No candidate/journal is changed.
        let hash = store_snapshot(&fixture.config.disks, &invalid).unwrap();
        let snapshot: crate::journal::Snapshot =
            crate::journal::load_object(&fixture.config.disks[0], "snapshot", &hash).unwrap();
        let mut evidence = Vec::new();
        for disk in &fixture.config.disks {
            for (kind, address) in std::iter::once(("snapshot", hash.as_str())).chain(
                crate::journal::map_addresses(disk, &snapshot.events)
                    .iter()
                    .map(|(kind, hash)| (*kind, hash.as_str())),
            ) {
                let path = crate::journal::object_path(disk, kind, address);
                evidence.push((path.clone(), fs::read(path).unwrap()));
            }
            let error = load_snapshot(disk, &hash).unwrap_err();
            assert_eq!(error.code(), ErrorCode::StateInvalid);
            assert_eq!(error.message(), message);
        }
        let error = crate::publication_evidence::verified_snapshot(&fixture.config.disks, &hash)
            .err()
            .unwrap();
        assert_eq!(error.code(), ErrorCode::StateInvalid);
        assert_eq!(error.message(), message);
        for (path, bytes) in evidence {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
        assert_eq!(engine.state.as_ref(), &original);
    }
    drop(engine);
    assert_eq!(
        Engine::open(&fixture.root).unwrap().state.as_ref(),
        &original
    );
}

// These regressions re-encode real Engine snapshots with valid object hashes.
// They check cold semantic admission, not forged publication acceptance.
struct DeletedAdmissionFixture {
    engine: Engine,
    file: String,
    fixture: Fixture,
}
impl DeletedAdmissionFixture {
    fn new() -> Self {
        let fixture = Fixture::new();
        let mut engine = fixture.initialize();
        let repository = engine
            .create_disk_root("P01", "admin", OperationId::generate().unwrap())
            .unwrap();
        save(
            &mut engine,
            &repository.main_branch_id,
            "deleted.txt",
            b"retained",
        );
        let file = engine
            .namespace_tree(&repository.main_branch_id)
            .unwrap()
            .get("deleted.txt")
            .unwrap()
            .file_id
            .clone();
        let repository_id =
            storage_contract::history_window::HistoryRepositoryId::parse(&repository.repository_id)
                .unwrap();
        let file_id = storage_contract::history_window::HistoryFileId::parse(&file).unwrap();
        let path = name_policy::LookupPath::parse("deleted.txt").unwrap();
        engine
            .soft_delete_file(crate::DeleteFileRequest {
                target: crate::FileDeletionTarget {
                    repository_id: &repository_id,
                    file_id: &file_id,
                    path: &path,
                },
                principal: "admin",
                operation_id: OperationId::generate().unwrap(),
            })
            .unwrap();
        Self {
            engine,
            file,
            fixture,
        }
    }
    fn admit_snapshot(&self, state: &State) -> Result<State> {
        let hash = store_snapshot(&self.fixture.config.disks, state).unwrap();
        load_snapshot(&self.fixture.config.disks[0], &hash)
    }
    fn reject_lifecycle(&self, state: &State) {
        let error = self.admit_snapshot(state).unwrap_err();
        assert_eq!(error.code(), ErrorCode::PublicationInvalid, "{error}");
    }
}

#[test]
fn soft_delete_cold_admission_rejects_missing_current_marker_without_an_undo_receipt() {
    let deleted = DeletedAdmissionFixture::new();
    let original = deleted.engine.state().clone();
    assert_eq!(deleted.admit_snapshot(&original).unwrap(), original);
    let mut missing = original.clone();
    missing.file_deletions.remove(&deleted.file).unwrap();
    assert_eq!(missing.operations, original.operations);
    assert_eq!(missing.operation_principals, original.operation_principals);
    assert_eq!(missing.events, original.events);
    deleted.reject_lifecycle(&missing);
}

#[test]
fn soft_delete_cold_admission_rejects_a_creation_receipts_different_internal_operation_id() {
    let deleted = DeletedAdmissionFixture::new();
    let original = deleted.engine.state().clone();
    assert_eq!(deleted.admit_snapshot(&original).unwrap(), original);
    let operation = original.file_deletions[&deleted.file].operation_id;
    let mut altered = original.clone();
    altered.operations.get_mut(&operation).unwrap().operation_id = OperationId::generate().unwrap();
    assert_eq!(altered.file_deletions, original.file_deletions);
    assert_eq!(altered.events, original.events);
    deleted.reject_lifecycle(&altered);
}

#[test]
fn soft_delete_cold_admission_replays_undo_then_redelete_without_resurrecting_old_markers() {
    let mut deleted = DeletedAdmissionFixture::new();
    let node = deleted.engine.state().file_deletions[&deleted.file].clone();
    let file = storage_contract::history_window::HistoryFileId::parse(&deleted.file).unwrap();
    deleted
        .engine
        .undo_file_deletion(crate::UndoFileDeletionRequest {
            file_id: &file,
            deletion_node_id: &node.node_id,
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
    let undone = deleted.engine.state().clone();
    assert_eq!(deleted.admit_snapshot(&undone).unwrap(), undone);
    let mut resurrected = undone.clone();
    resurrected
        .file_deletions
        .insert(deleted.file.clone(), node.clone());
    deleted.reject_lifecycle(&resurrected);
    let branch = deleted.engine.state().repositories[&node.repository_id]
        .main_branch_id
        .clone();
    // The old D and its undo remain valid after a genuine later content save.
    // Neither cold State admission nor the SQLite projection may compare that
    // historical D frontier with today's changed HEAD.
    save(
        &mut deleted.engine,
        &branch,
        node.path.as_str(),
        b"changed after undo",
    );
    deleted
        .engine
        .soft_delete_file(crate::DeleteFileRequest {
            target: crate::FileDeletionTarget {
                repository_id: &storage_contract::history_window::HistoryRepositoryId::parse(
                    &node.repository_id,
                )
                .unwrap(),
                file_id: &file,
                path: &name_policy::LookupPath::parse(node.path.as_str()).unwrap(),
            },
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
    let again = deleted.engine.state().clone();
    assert_ne!(again.file_deletions[&deleted.file].node_id, node.node_id);
    assert_ne!(
        again.file_deletions[&deleted.file].branch_heads,
        node.branch_heads
    );
    assert_eq!(deleted.admit_snapshot(&again).unwrap(), again);
    let mut stale = again.clone();
    stale.file_deletions.insert(deleted.file.clone(), node);
    deleted.reject_lifecycle(&stale);
}

#[test]
fn soft_delete_cold_admission_requires_one_matching_event_and_exact_undo_consumption() {
    let mut deleted = DeletedAdmissionFixture::new();
    let original = deleted.engine.state().clone();
    let node = original.file_deletions[&deleted.file].clone();
    let mut missing_event = original.clone();
    missing_event
        .events
        .retain(|event| event.operation_id != node.operation_id);
    deleted.reject_lifecycle(&missing_event);
    let mut duplicate_event = original.clone();
    let event = duplicate_event.events.last().unwrap().clone();
    duplicate_event.events.push(event);
    deleted.reject_lifecycle(&duplicate_event);
    let operation = OperationId::generate().unwrap();
    deleted
        .engine
        .undo_file_deletion(crate::UndoFileDeletionRequest {
            file_id: &storage_contract::history_window::HistoryFileId::parse(&deleted.file)
                .unwrap(),
            deletion_node_id: &node.node_id,
            principal: "admin",
            operation_id: operation,
        })
        .unwrap();
    let mut wrong_node = deleted.engine.state().clone();
    let unknown = new_id("fdel").unwrap();
    let receipt = wrong_node.operations.get_mut(&operation).unwrap();
    receipt.result["removed_deletion_node"]["node_id"] = unknown.clone().into();
    // Even a self-consistent request digest does not prove that this D existed.
    receipt.request_digest = Engine::native_request_digest(
        "file.delete.undo",
        "admin",
        &json_codec::json!({"file_id":deleted.file,"deletion_node_id":unknown}),
    )
    .unwrap();
    deleted.reject_lifecycle(&wrong_node);
    let mut internal_id = deleted.engine.state().clone();
    internal_id
        .operations
        .get_mut(&operation)
        .unwrap()
        .operation_id = OperationId::generate().unwrap();
    deleted.reject_lifecycle(&internal_id);
}
