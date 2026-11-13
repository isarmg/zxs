use super::*;

#[test]
fn status_work_and_text_budgets_fail_instead_of_truncating() {
    let cancellation = HistoryWindowCancellation::new();
    let mut budget = Budget::new(&cancellation);
    budget.work = 1;
    budget.step().unwrap();
    assert_eq!(budget.step().unwrap_err().code(), ErrorCode::ResourceLimit);
    let mut budget = Budget::new(&cancellation);
    budget.text_bytes = 3;
    budget.text("abc").unwrap();
    assert_eq!(
        budget.text("d").unwrap_err().code(),
        ErrorCode::ResourceLimit
    );
}

#[test]
fn cancellation_is_checked_during_each_work_and_text_charge() {
    let cancellation = HistoryWindowCancellation::new();
    let mut budget = Budget::new(&cancellation);
    budget.step().unwrap();
    cancellation.cancel();
    assert_eq!(
        budget.step().unwrap_err().code(),
        ErrorCode::OperationAborted
    );
    assert_eq!(
        budget.text("next").unwrap_err().code(),
        ErrorCode::OperationAborted
    );
}

#[test]
fn status_ignores_current_filename_projection_but_keeps_exact_source_contexts() {
    let mut state = State::default();
    let old = Entry {
        entry_id: "entry_00000000000000000000000000000001".into(),
        file_id: "file_00000000000000000000000000000001".into(),
        version_id: "ver_00000000000000000000000000000001".into(),
        kind: NamespaceEntryKind::File,
        path: name_policy::PortablePath::parse("dir/old").unwrap(), size: 7,
    };
    let mut latest = old.clone();
    latest.relocate(name_policy::PortablePath::parse("dir/latest").unwrap());
    state.file_locations.insert(old.file_id.clone(), "dir/latest".into());
    state.file_names.insert(old.file_id.clone(), "latest".into());
    let context = FileContext {
        branch_id: "br_00000000000000000000000000000001".into(),
        node_id: None, inherited_from: None, selection_generation: 0,
    };
    let old_contexts = BTreeMap::from([("dir/old".into(), context.clone())]);
    let new_contexts = BTreeMap::from([("dir/latest".into(), context)]);
    let old_entries = [old];
    let new_entries = [latest];
    let cancellation = HistoryWindowCancellation::new();
    let mut budget = Budget::new(&cancellation);
    let before = owned(&state, Contents {
        entries: &old_entries, contexts: &old_contexts, path: "dir",
    }, &BTreeSet::new(), &mut budget).unwrap();
    let after = owned(&state, Contents {
        entries: &new_entries, contexts: &new_contexts, path: "dir",
    }, &BTreeSet::new(), &mut budget).unwrap();
    assert!(before == after);
    assert_eq!(old_entries[0].path(), "dir/old");
}
