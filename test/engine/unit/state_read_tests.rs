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
    let version = engine
        .namespace_entries(&original.commits[&head].namespace)
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == NamespaceEntryKind::File)
        .unwrap()
        .version_id;
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
                    .repository_id = new_id("repo")
            }
            HistoryKey => {
                state
                    .histories
                    .get_mut(&repo.history_tree_id)
                    .unwrap()
                    .history_tree_id = new_id("history")
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
                    .main_branch_id = new_id("br")
            }
            MissingHead => {
                state
                    .branches
                    .get_mut(&repo.main_branch_id)
                    .unwrap()
                    .head_commit_id = new_id("commit")
            }
            MissingParent => {
                state.commits.get_mut(&head).unwrap().parent_commit_ids = vec![new_id("commit")]
            }
            Cycle => state.commits.get_mut(&head).unwrap().parent_commit_ids = vec![head.clone()],
            MultipleParents => {
                state.commits.get_mut(&head).unwrap().parent_commit_ids =
                    vec![root.clone(), root.clone()]
            }
            OrphanCommit => {
                let mut commit = state.commits[&head].clone();
                commit.commit_id = new_id("commit");
                commit.history_tree_id = new_id("history");
                state.commits.insert(commit.commit_id.clone(), commit);
            }
            OrphanVersion => {
                let mut orphan = state.versions[&version].clone();
                orphan.version_id = new_id("ver");
                orphan.file_id = new_id("file");
                orphan.history_tree_id = new_id("history");
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
                    .insert(new_id("ver"), ContentState::Available);
            }
            MissingPlacement => {
                state.placements.remove(&version);
            }
            OrphanFileSelection => {
                state.file_selections.insert(new_id("file"), new_id("br"));
            }
            OrphanFileNote => {
                state.file_notes.insert(new_id("file"), "note".into());
            }
            OrphanDirectorySelection => {
                state
                    .directory_selections
                    .insert(new_id("entry"), new_id("dbr"));
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
                    .insert(format!("{}/file.txt", new_id("repo")), context);
            }
            OrphanVersionNote => {
                state.version_notes.insert(new_id("ver"), "note".into());
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
            &new_id("commit"),
            &repo.main_branch_id,
            &head,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::CommitNotFound);
    let error = engine
        .add_merge_link(
            &new_id("br"),
            &head,
            &repo.main_branch_id,
            &head,
            "admin",
            storage_contract::OperationId::generate(),
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
            storage_contract::OperationId::generate(),
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
    StorageName,
    ZeroSequence,
    OverflowSequence,
    DigestPrefix,
    CreationTimeOffset,
    CreationTimeInvalidDate,
    CreationName,
}

impl VersionIdentityCorruption {
    fn apply(self, version: &mut FileVersion) {
        match self {
            Self::StorageName => version.storage_name = "invalid.txt".into(),
            Self::ZeroSequence => {
                let suffix = version.storage_name.split_once('_').unwrap().1;
                version.storage_name = format!("v000000_{suffix}");
            }
            Self::OverflowSequence => {
                let suffix = version.storage_name.split_once('_').unwrap().1;
                version.storage_name = format!("v18446744073709551616_{suffix}");
            }
            Self::DigestPrefix => {
                let (prefix, digest) = version.storage_name.rsplit_once('_').unwrap();
                let first = if digest.starts_with('0') { '1' } else { '0' };
                version.storage_name = format!("{prefix}_{first}{}", &digest[1..]);
            }
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
            let (name, _) = crate::container_storage::version_identity(
                &admitted,
                &matching.file_id,
                matching.path(),
                &blake3::hash(b"next"),
            )
            .unwrap();
            assert!(name.starts_with("v000002_"), "{name}");

            for corruption in [
                StorageName,
                ZeroSequence,
                OverflowSequence,
                DigestPrefix,
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
    let (name, _) = crate::container_storage::version_identity(
        reopened.state(),
        &matching.file_id,
        matching.path(),
        &blake3::hash(b"after reopen"),
    )
    .unwrap();
    assert!(name.starts_with("v000002_"), "{name}");
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
            .tail()
            .iter()
            .map(|event| event.sequence)
            .is_sorted()
    );
    let mut duplicate = original.clone();
    duplicate
        .events
        .push(original.events.tail().last().unwrap().clone())
        .unwrap();
    assert_eq!(
        duplicate.events.tail().last().unwrap().sequence,
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
    decreasing.events = storage_contract::SegmentedEvents::from_events(
        original.events.tail().iter().rev().cloned().collect(),
    )
    .unwrap();
    assert!(
        !decreasing
            .events
            .tail()
            .iter()
            .map(|event| event.sequence)
            .is_sorted()
    );
    let mut future = original.clone();
    let mut reordered = original.events.tail().to_vec();
    reordered.last_mut().unwrap().sequence = future.sequence + 1;
    future.events = storage_contract::SegmentedEvents::from_events(reordered).unwrap();
    assert!(
        future
            .events
            .tail()
            .iter()
            .map(|event| event.sequence)
            .is_sorted()
    );
    // Out-of-order bytes now fail at the segment decode boundary, before the
    // resident-state admission that still owns the published-sequence bound.
    for (invalid, code, message) in [
        (
            decreasing,
            ErrorCode::ObjectCorrupt,
            "event segment sequences are out of order",
        ),
        (
            future,
            ErrorCode::StateInvalid,
            "snapshot event sequence exceeds published sequence",
        ),
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
            assert_eq!(error.code(), code);
            assert_eq!(error.message(), message);
        }
        let error = crate::publication_evidence::verified_snapshot(&fixture.config.disks, &hash)
            .err()
            .unwrap();
        assert_eq!(error.code(), code);
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
