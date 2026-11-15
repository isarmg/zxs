use super::*;
use storage_contract::file_draft::DraftPayloadId;
use storage_contract::{BranchId, ContentHash, FileNodeId, Generation, VersionId};

fn revision(value: u64) -> WorkingRevision {
    WorkingRevision::new(Generation::new(value))
}

fn base(selection: u64) -> DraftBase {
    let version = VersionId::from_random([3; 16]);
    DraftBase::new(
        BranchId::from_random([2; 16]),
        FileNodeId::Version(version),
        version,
        Generation::new(selection),
    )
}

fn payload(identity: u8, bytes: &[u8]) -> DraftPayload {
    DraftPayload::new(
        DraftPayloadId::from_random([identity; 16]),
        ContentHash::from_digest(blake3::hash(bytes)),
        bytes.len() as u64,
    )
}

#[test]
fn manual_writes_retain_first_base_and_commit_consumes_the_exact_latest_draft() {
    let automatic = FileWorkingState::Automatic {
        revision: revision(0),
    };
    let mode = change_mode(&automatic, revision(0), CommitMode::Manual).unwrap();
    assert!(mode.changed());
    assert_eq!(mode.previous(), &automatic);
    let clean = mode.into_next();
    assert_eq!(clean.revision(), revision(1));
    let first = write(&clean, revision(1), base(8), payload(4, b"first draft"))
        .unwrap()
        .into_next();
    assert!(clean.draft().is_none());
    let second = write(&first, revision(2), base(8), payload(5, b"second draft"))
        .unwrap()
        .into_next();
    let (original_base, original_payload) = first.draft().unwrap();
    let (latest_base, latest_payload) = second.draft().unwrap();
    assert_eq!(original_base, latest_base);
    assert_ne!(original_payload.payload_id(), latest_payload.payload_id());
    assert_eq!(original_payload.content_hash(), payload(4, b"first draft").content_hash());
    let prepared = commit(&second, revision(3)).unwrap();
    assert_eq!(prepared.base(), latest_base);
    assert_eq!(prepared.payload(), latest_payload);
    assert!(second.draft().is_some());
    let completed = prepared.into_change();
    assert_eq!(completed.previous(), &second);
    assert_eq!(completed.next().revision(), revision(4));
    assert_eq!(completed.next().mode(), CommitMode::Manual);
    assert!(completed.next().draft().is_none());
}

#[test]
fn stale_writes_mode_changes_and_selection_rebinding_do_not_replace_a_draft() {
    let clean = FileWorkingState::ManualClean {
        revision: revision(2),
    };
    let dirty = write(&clean, revision(2), base(1), payload(4, b"draft"))
        .unwrap()
        .into_next();
    let before = dirty.clone();
    assert!(matches!(
        write(&dirty, revision(2), base(2), payload(5, b"later")),
        Err(DraftError::RevisionChanged { expected, actual })
            if expected == revision(2) && actual == revision(3)
    ));
    let original = base(1);
    let other_version = VersionId::from_random([9; 16]);
    for replacement_base in [
        base(2),
        DraftBase::new(
            BranchId::from_random([9; 16]),
            original.node_id(),
            original.version_id(),
            original.selection_generation(),
        ),
        DraftBase::new(
            original.branch_id(),
            FileNodeId::Version(other_version),
            original.version_id(),
            original.selection_generation(),
        ),
        DraftBase::new(
            original.branch_id(),
            original.node_id(),
            other_version,
            original.selection_generation(),
        ),
    ] {
        assert!(matches!(
            write(&dirty, revision(3), replacement_base, payload(5, b"later")),
            Err(DraftError::BaseChanged)
        ));
    }
    assert!(matches!(
        change_mode(&dirty, revision(3), CommitMode::Automatic),
        Err(DraftError::Dirty)
    ));
    assert!(matches!(require_clean(&dirty), Err(DraftError::Dirty)));
    assert!(!change_mode(&dirty, revision(3), CommitMode::Manual).unwrap().changed());
    assert_eq!(dirty, before);
    let automatic = FileWorkingState::Automatic {
        revision: revision(0),
    };
    assert!(matches!(
        write(&automatic, revision(0), base(1), payload(4, b"draft")),
        Err(DraftError::ManualRequired)
    ));
}

#[test]
fn discard_preserves_mode_and_same_bytes_still_advance_the_working_fence() {
    let clean = FileWorkingState::ManualClean {
        revision: revision(1),
    };
    let first = write(&clean, revision(1), base(1), payload(4, b"same"))
        .unwrap()
        .into_next();
    let same_bytes = write(&first, revision(2), base(1), payload(5, b"same"))
        .unwrap()
        .into_next();
    assert_eq!(same_bytes.revision(), revision(3));
    assert_eq!(
        same_bytes.draft().unwrap().1.content_hash(),
        first.draft().unwrap().1.content_hash(),
    );
    let discarded = discard(&same_bytes, revision(3)).unwrap().into_next();
    assert_eq!(discarded.revision(), revision(4));
    assert_eq!(discarded.mode(), CommitMode::Manual);
    assert!(discarded.draft().is_none());
    assert!(!discard(&discarded, revision(4)).unwrap().changed());
    assert!(matches!(
        discard(&discarded, revision(3)),
        Err(DraftError::RevisionChanged { .. })
    ));
    assert!(matches!(
        commit(&discarded, revision(4)),
        Err(DraftError::DraftRequired)
    ));
}

#[test]
fn exhausted_working_revision_never_wraps_or_clears_a_dirty_body() {
    let maximum = revision(u64::MAX);
    let automatic = FileWorkingState::Automatic { revision: maximum };
    assert!(!change_mode(&automatic, maximum, CommitMode::Automatic).unwrap().changed());
    assert!(matches!(
        change_mode(&automatic, maximum, CommitMode::Manual),
        Err(DraftError::RevisionExhausted(GenerationExhausted))
    ));
    let dirty = FileWorkingState::ManualDirty {
        revision: maximum,
        base: base(1),
        payload: payload(4, b"retain"),
    };
    let before = dirty.clone();
    assert!(matches!(
        write(&dirty, maximum, base(1), payload(5, b"replacement")),
        Err(DraftError::RevisionExhausted(GenerationExhausted))
    ));
    assert!(matches!(
        commit(&dirty, maximum),
        Err(DraftError::RevisionExhausted(GenerationExhausted))
    ));
    assert!(matches!(
        discard(&dirty, maximum),
        Err(DraftError::RevisionExhausted(GenerationExhausted))
    ));
    assert_eq!(dirty, before);
}
