use super::*;
use storage_contract::snapshot_worktree::outcome::{WorktreeCommitOutcome, WorktreeCommitReceipt};
use crate::files::directory_snapshot::worktree::{requests::commit_command, tests::state};
use storage_contract::working_tree::{SnapshotNodeId, SnapshotRepositoryId};

fn id(n: u8) -> OperationId { OperationId::from_random([n; 16]) }

fn original(s: &State) -> (wire::Record, WorktreeCommitReceipt, SnapshotHandle) {
    let command = commit_command(s, id(70)).unwrap();
    let wire::Command::CommitWorktree { repository, source, request } = command.clone() else {
        panic!("a commit to the exact open instance")
    };
    let committed = SnapshotHandle { publication: id(71), node_id: SnapshotNodeId::from_random([72; 16]), ..source };
    let result = WorktreeCommitReceipt {
        repository_id: repository, request, result: WorktreeCommitOutcome::Complete { snapshot: committed },
    };
    (wire::Record::new(s.principal.clone().unwrap(), command).unwrap(), result, committed)
}

fn hidden(s: &mut State) -> MountChangeReceipt {
    let (original, _, committed) = original(s);
    let published = original.result(json_codec::to_value(committed).unwrap()).unwrap();
    accepted(s, &original, &published);
    let wire::Command::MountChange { request, .. } = hide(s, id(73)).unwrap() else { panic!("hide") };
    MountChangeReceipt { request, outcome: MountOutcome::Hidden {} }
}

#[test]
fn hide_requires_confirmed_commit_exact_open_revision_and_the_current_visible_occurrence() {
    let mut s = state();
    assert!(hide(&s, id(73)).is_none(), "a healthy status is not a committed content batch");
    let before = hidden(&mut s).request;
    let obs = s.files.directory_snapshot.worktree.observed.as_mut().unwrap();
    assert!(matches!(before.action, MountAction::Hide { visible, .. } if visible == obs.status.visible));
    let expected = obs.status.revision;
    obs.status.revision.generation = storage_contract::Generation::new(99);
    assert!(hide(&s, id(74)).is_none(), "a later save cannot reuse this confirmed revision");
    let obs = s.files.directory_snapshot.worktree.observed.as_mut().unwrap();
    obs.status.revision = expected;
    obs.status.visible = id(79);
    let wire::Command::MountChange { request, .. } = hide(&s, id(74)).unwrap() else { panic!("hide") };
    assert!(matches!(request.action, MountAction::Hide { visible, .. } if visible == id(79)));
    assert_ne!(before.action, request.action, "equal name and revision do not revive the old occurrence");
    s.files.mutation_pending = true;
    assert!(hide(&s, id(74)).is_none());
    s.files.mutation_pending = false;
    s.session_epoch += 1;
    assert!(hide(&s, id(74)).is_none());
}

#[test]
fn original_commit_lookup_accepts_only_the_opened_instance_revision_and_a_real_completed_new_node() {
    let mut s = state();
    let (_, original, committed) = original(&s);
    let input = original.request.operation_id;
    edit(&mut s, Field::Operation, input.to_string());
    let q = Reader::capture(&s).unwrap();
    let v = json_codec::to_value(&original).unwrap();
    let p = q.admit(v.clone()).unwrap();
    assert_eq!((p.expected, p.committed, p.operation), (q.status.revision, committed, input));
    let mut pending = original.clone();
    pending.result = WorktreeCommitOutcome::Pending {};
    assert!(q.admit(json_codec::to_value(pending).unwrap()).is_err());
    for (key, wrong) in [
        ("operation_id", json_codec::json!(id(1))),
        ("target", json_codec::json!({"publication": id(2), "worktree_id": q.status.target.worktree_id})),
        ("expected", json_codec::json!({"generation":"987", "manifest":q.status.revision.manifest})),
    ] {
        let mut changed = v.clone();
        changed["request"][key] = wrong;
        assert!(q.admit(changed).is_err(), "{key}");
    }
    let mut positional = v;
    positional["request"]["expected"] = json_codec::json!(["2", q.status.revision.manifest]);
    assert!(q.admit(positional).is_err(), "named request objects are required");
    let mut same = original;
    same.result = WorktreeCommitOutcome::Complete { snapshot: q.status.original };
    assert!(q.admit(json_codec::to_value(same).unwrap()).is_err());
    s.files.directory_snapshot.worktree.observed.as_mut().unwrap().status.visible = id(81);
    assert_ne!(Reader::capture(&s).as_ref(), Some(&q), "late proof cannot install after remount");
}

#[test]
fn restore_uses_only_the_supplied_hidden_origin_and_the_current_account_not_a_global_listing() {
    let mut s = state();
    let hidden = hidden(&mut s);
    let server = json_codec::to_string(&hidden).unwrap();
    assert!(restore(&s, id(75)).is_none());
    edit(&mut s, Field::Hidden, server.clone());
    let wire::Command::MountChange { request, .. } = restore(&s, id(75)).unwrap() else { panic!("restore") };
    assert_eq!(request.target, hidden.request.target);
    assert_eq!(request.expected, hidden.request.expected);
    assert_eq!(request.action, MountAction::Restore { deletion: hidden.request.operation_id, naming: RestoreNaming::OriginalOrAutomatic {} });
    edit(&mut s, Field::Name, "safe-new-name".into());
    let wire::Command::MountChange { request, .. } = restore(&s, id(76)).unwrap() else { panic!("restore") };
    assert!(matches!(request.action, MountAction::Restore { deletion, naming: RestoreNaming::Explicit { name } }
        if deletion == hidden.request.operation_id && name.as_str() == "safe-new-name"));
    let mut wrong = hidden.clone();
    if let MountAction::Hide { committed, .. } = &mut wrong.request.action {
        committed.repository_id = SnapshotRepositoryId::from_random([99; 16]);
    }
    edit(&mut s, Field::Hidden, json_codec::to_string(&wrong).unwrap());
    assert!(restore(&s, id(77)).is_none(), "a receipt for another opened root is not an input");
    edit(&mut s, Field::Hidden, server.clone());
    edit(&mut s, Field::Name, "x".repeat(MAX_RECEIPT_BYTES + 1));
    assert!(restore(&s, id(77)).is_none(), "oversize explicit name cannot become automatic");
    edit(&mut s, Field::Name, String::new());
    let mut pending = hidden;
    pending.outcome = MountOutcome::Pending {};
    edit(&mut s, Field::Hidden, json_codec::to_string(&pending).unwrap());
    assert!(restore(&s, id(77)).is_none());
    edit(&mut s, Field::Hidden, server.clone());
    s.principal = Some("another-manager".parse().unwrap());
    s.session_epoch += 1;
    assert!(restore(&s, id(77)).is_none(), "the previous account's pasted receipt is not inherited");
    s.files.directory_snapshot.owner = Some((s.session_epoch, s.principal.clone().unwrap()));
    s.files.directory_snapshot.review.as_mut().unwrap().target.epoch = s.session_epoch;
    edit(&mut s, Field::Hidden, server);
    let command = restore(&s, id(78)).unwrap();
    let record = wire::Record::new(s.principal.clone().unwrap(), command).unwrap();
    assert_eq!(record.principal.as_str(), "another-manager");
    assert!(record.command.body().unwrap().get("principal").is_none(), "this is a new action, not impersonation");
}

#[test]
fn original_mount_retry_requires_positive_exact_pending_not_absence_or_a_different_visible_token() {
    let mut s = state();
    let receipt = hidden(&mut s);
    let repo = Scope::capture(&s).unwrap().repository;
    let original = &receipt.request;
    let record = wire::Record::new(s.principal.clone().unwrap(), wire::Command::MountChange {
        repository: repo, request: original.clone(),
    }).unwrap();
    assert_eq!(record, wire::Record::parse(&record.encode().unwrap(), &record.principal).unwrap());
    assert_eq!(record.command.endpoint(), format!("/api/directory-worktrees/{}/mount-changes", original.target.worktree_id));
    assert_eq!(record.command.outcome_endpoint().unwrap(), format!("/api/repositories/{repo}/directory-worktrees/mount-changes/{}", original.operation_id));
    let decide = |r| wire::mount::settled(repo, original, r);
    let pending = MountChangeReceipt { outcome: MountOutcome::Pending {}, ..receipt.clone() };
    assert!(matches!(decide(Ok(json_codec::to_value(&pending).unwrap())), Ok(None)));
    assert!(matches!(decide(Ok(json_codec::to_value(&receipt).unwrap())), Ok(Some(wire::Publication::MountChanged { receipt: r, .. })) if r == receipt));
    for status in [403, 404, 503] {
        let mut error = wire::invalid("unknown or unauthorized original");
        error.status = status;
        assert_eq!(decide(Err(error)).unwrap_err().status, status);
    }
    let mut wrong = pending;
    if let MountAction::Hide { visible, .. } = &mut wrong.request.action { *visible = id(88); }
    assert!(decide(Ok(json_codec::to_value(wrong).unwrap())).is_err());
    let mut contradictory = receipt;
    let name = name_policy::Name::parse("same-name").unwrap();
    contradictory.outcome = MountOutcome::Restored { name: name.clone() };
    assert!(wire::mount::receipt(json_codec::to_value(&contradictory).unwrap()).is_err());
    contradictory.request.action = MountAction::Restore {
        deletion: id(71), naming: RestoreNaming::Explicit { name: name_policy::Name::parse("elsewhere").unwrap() },
    };
    assert!(wire::mount::receipt(json_codec::to_value(&contradictory).unwrap()).is_err());
    contradictory.request.action = MountAction::Restore {
        deletion: id(71), naming: RestoreNaming::Explicit { name },
    };
    assert!(wire::mount::receipt(json_codec::to_value(contradictory).unwrap()).is_ok());
}

#[test]
fn known_original_query_is_read_only_account_bound_and_never_confers_restore_or_the_current_name() {
    let mut s = state();
    let receipt = hidden(&mut s);
    edit(&mut s, Field::Lookup, receipt.request.operation_id.to_string());
    let q = Query::capture(&s).unwrap();
    let admitted = q.admit(json_codec::to_value(&receipt).unwrap()).unwrap();
    s.files.directory_snapshot.worktree.mount.seen = Some((q.clone(), admitted));
    assert_eq!(super::original::observed(&s), Some(&receipt));
    assert!(restore(&s, id(82)).is_none(), "seeing an original does not select or resubmit a change");
    let mut other = receipt;
    other.request.operation_id = id(83);
    assert!(q.admit(json_codec::to_value(other).unwrap()).is_err());
    s.management.storage = crate::management::Storage::ReadOnly;
    assert!(super::original::observed(&s).is_some());
    assert!(hide(&s, id(84)).is_none() && restore(&s, id(84)).is_none());
    edit(&mut s, Field::Lookup, id(85).to_string());
    assert!(super::original::observed(&s).is_none());
    assert_ne!(Query::capture(&s), Some(q));
    s.session_epoch += 1;
    assert!(Query::capture(&s).is_none());
}
