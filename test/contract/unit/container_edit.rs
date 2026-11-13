use crate::container_edit::{CommitMode, ContainerEdit, DraftBase, DraftRevision, EditConflict};
use crate::{ContentHash, Generation, OperationId, Principal};

fn revision() -> DraftRevision {
    DraftRevision {
        operation: "op_00000000000000000000000000000001".parse().unwrap(),
        expected_generation: Generation::new(1),
        principal: Principal::new("alice".into()).unwrap(),
        base: DraftBase {
            entry: crate::EntryId::generate().unwrap(),
            repository: "repo_00000000000000000000000000000001".parse().unwrap(),
            branch: "br_00000000000000000000000000000001".parse().unwrap(),
            node: "ver_00000000000000000000000000000001".parse().unwrap(),
            version: "ver_00000000000000000000000000000001".parse().unwrap(),
            selection_generation: Generation::new(0),
        },
        content_hash: ContentHash::from_digest(blake3::hash(b"draft")),
        size: 5,
    }
}

#[test]
fn automatic_is_initial_and_dirty_requires_explicit_discard() {
    let automatic = ContainerEdit::initial();
    assert_eq!(automatic.mode(), CommitMode::Automatic);
    let manual = automatic.set_mode(Generation::new(0), CommitMode::Manual).unwrap();
    let draft = revision();
    let dirty = manual.save(Generation::new(1), draft.clone()).unwrap();
    assert_eq!(dirty.generation(), Generation::new(2));
    assert_eq!(dirty.set_mode(Generation::new(2), CommitMode::Automatic),
        Err(EditConflict::UncommittedDraft));
    assert_eq!(dirty.require_clean(), Err(EditConflict::UncommittedDraft));
    let clean = dirty.discard(Generation::new(2), draft.operation).unwrap();
    assert_eq!(clean.mode(), CommitMode::Manual);
    assert!(clean.draft().is_none());
    assert_eq!(dirty.draft(), Some(&draft));
}

#[test]
fn stale_generation_and_changed_source_do_not_replace_original_draft() {
    let draft = revision();
    let manual = ContainerEdit::initial()
        .set_mode(Generation::new(0), CommitMode::Manual).unwrap();
    let dirty = manual.save(Generation::new(1), draft.clone()).unwrap();
    assert_eq!(dirty.save(Generation::new(1), draft.clone()), Err(EditConflict::GenerationChanged));
    let mut changed = draft.clone();
    changed.expected_generation = Generation::new(2);
    changed.operation = OperationId::generate().unwrap();
    changed.base.selection_generation = Generation::new(7);
    assert_eq!(dirty.save(Generation::new(2), changed), Err(EditConflict::SourceChanged));
    assert_eq!(dirty.draft(), Some(&draft));
}
