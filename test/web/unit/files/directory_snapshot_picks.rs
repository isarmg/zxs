use super::super::{
    tests::{born, head, location, state, unborn},
    wire::{Command, Record},
};
use super::*;
use storage_contract::{OperationId, snapshot_worktree::draft_commit::CommitDirectoryDrafts};

fn page(folder: &str, candidates: Vec<Candidate>) -> Review {
    let mut s = state();
    s.files.repository_path = folder.into();
    s.files.slot = "P01".into();
    Review {
        target: Target::capture(&s).unwrap(),
        head: head(),
        selected: candidates.iter().map(|i| i.selected.target.entry).collect(),
        candidates,
    }
}
fn first() -> Candidate {
    let mut candidate = born();
    candidate.selected.target = location(11, "left/one.txt");
    candidate
}
fn second() -> Candidate {
    let mut candidate = unborn();
    candidate.selected.target = location(21, "right/one.txt");
    candidate
}

#[test]
fn distinct_folders_freeze_one_exact_mixed_original_without_recursive_or_implicit_rows() {
    let who = "alice".parse().unwrap();
    let left = page("left", vec![first()]);
    let kept = Kept::new(&who, &left, vec![first()]);
    let mut right = page("right", vec![second()]);
    let mut ignored = unborn();
    ignored.selected.target = location(93, "right/ignored.txt");
    right.candidates.push(ignored);
    assert!(has_current(Some(&kept), &who, &right));
    let all = collect(Some(&kept), &who, &right).unwrap();
    assert_eq!(all, vec![&first(), &second()]);
    let request = CommitDirectoryDrafts {
        source: wire::ready_source(&right.head).unwrap(),
        message: "both exact paths".into(),
        operation_id: OperationId::from_random([91; 16]),
        selected: all.into_iter().map(|it| it.selected.clone()).collect(),
    };
    let record = Record::new(
        who.clone(),
        Command::Commit {
            repository: right.target.repository,
            request,
        },
    )
    .unwrap();
    let frozen = record.encode().unwrap();
    assert_eq!(Record::parse(&frozen, &who).unwrap(), record);
    assert!(!frozen.contains("ignored.txt"));
    right.selected.clear();
    assert!(has_current(Some(&kept), &who, &right));
    assert_eq!(collect(Some(&kept), &who, &right).unwrap(), vec![&first()]);
    let again = page("left", vec![first()]);
    assert_eq!(collect(Some(&kept), &who, &again).unwrap(), vec![&first()]);
}

#[test]
fn changed_identity_revision_branch_account_and_budget_cannot_substitute_a_kept_original() {
    let who = "alice".parse().unwrap();
    let left = page("left", vec![first()]);
    let kept = Kept::new(&who, &left, vec![first()]);
    let mut wrong = page("right", vec![second()]);
    assert_eq!(
        collect(Some(&kept), &"bob".parse().unwrap(), &wrong),
        Err(Rejection::SourceChanged)
    );
    wrong.target.epoch += 1;
    assert_eq!(
        collect(Some(&kept), &who, &wrong),
        Err(Rejection::SourceChanged)
    );
    wrong.target.epoch = left.target.epoch;
    wrong.target.slot = "P02".into();
    assert!(!has_current(Some(&kept), &who, &wrong));
    wrong.target.slot = left.target.slot.clone();
    wrong.target.namespace_head = CommitId::from_random([89; 16]);
    assert_eq!(
        collect(Some(&kept), &who, &wrong),
        Err(Rejection::SourceChanged)
    );
    wrong.target.namespace_head = left.target.namespace_head;
    wrong.head.head =
        storage_contract::snapshot_worktree::discovery::SnapshotHeadDiscovery::NotInitialized;
    assert!(!has_current(Some(&kept), &who, &wrong));
    let mut alias = second();
    alias.selected.target.file = first().selected.target.file;
    assert_eq!(
        collect(Some(&kept), &who, &page("right", vec![alias])),
        Err(Rejection::ConflictingOccurrence)
    );
    let mut case_alias = second();
    case_alias.selected.target.path = name_policy::PortablePath::parse("LEFT/ONE.TXT").unwrap();
    assert_eq!(
        collect(Some(&kept), &who, &page("left", vec![case_alias])),
        Err(Rejection::ConflictingOccurrence)
    );
    let mut changed = first();
    changed.selected.expected = second().selected.expected;
    assert_eq!(
        collect(Some(&kept), &who, &page("left", vec![changed])),
        Err(Rejection::ConflictingOccurrence)
    );
    let mut missing = page("left", vec![first()]);
    missing.selected.insert(second().selected.target.entry);
    assert_eq!(collect(None, &who, &missing), Err(Rejection::InvalidPage));
    let mut options = Vec::new();
    for i in 0..wire::MAX_SELECTIONS {
        let mut item = unborn();
        item.selected.target = location(u8::try_from(i).unwrap(), &format!("left/{i}.txt"));
        options.push(item);
    }
    let mut overflow = unborn();
    overflow.selected.target = location(255, "right/over.txt");
    let full = Kept::new(&who, &left, options);
    assert_eq!(
        collect(Some(&full), &who, &page("right", vec![overflow])),
        Err(Rejection::TooMany)
    );
}
