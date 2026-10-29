use super::*;
use crate::files::history::HistoryBinding;
use crate::files::history::tests::{ROOT, binding, branch_page, chain};

fn error(code: storage_contract::ErrorCode) -> api::Error {
    api::Error {
        code,
        message: "injected".into(),
        retryable: false,
        status: 409,
        request_id: None,
        source: None,
    }
}

#[test]
fn installing_a_window_replaces_the_previous_page_and_every_derived_slot() {
    let mut history = History::default();
    history.install(chain(3));
    let first = history.loaded().unwrap().clone();
    history.branches = Some(branch_page(vec![], None));
    history.merges = Some(
        MergePage::parse(
            crate::files::history::tests::merge_page_wire(vec![]),
            &binding(),
        )
        .unwrap(),
    );
    history.panel = Some(AuxPanel::Parents {
        node_id: "n3".into(),
        page: ParentsPage {
            node_id: "n3".into(),
            parents: vec![],
            previous_cursor: None,
            next_cursor: None,
        },
    });
    assert!(history.branch_page().is_some() && history.merge_page().is_some());
    let weak = first.weak();
    history.install(chain(2));
    assert!(history.branches.is_none() && history.merges.is_none() && history.panel.is_none());
    assert_eq!(history.loaded().unwrap().nodes.len(), 2);
    drop(first);
    assert!(
        weak.upgrade().is_none(),
        "the replaced page is freed once its last reader drops it"
    );
}

#[test]
fn stale_root_keeps_the_window_readable_but_closes_root_bound_pages() {
    let mut history = History::default();
    history.install(chain(2));
    history.branches = Some(branch_page(vec![], None));
    let generation = history.generation;
    history.mark_stale(StaleReason::RootChanged);
    assert!(matches!(history.state, HistoryState::Stale { .. }));
    assert!(
        history.loaded().is_none(),
        "a stale page is not a write basis"
    );
    assert!(history.root().is_none());
    assert_eq!(history.window().unwrap().nodes.len(), 2);
    assert!(history.branch_page().is_none() && history.branches.is_none());
    assert_eq!(history.generation, generation + 1);
    assert!(matches!(
        history.state,
        HistoryState::Stale {
            reason: StaleReason::RootChanged,
            ..
        }
    ));
    // Marking stale without a resident page leaves nothing to display.
    let mut empty = History::default();
    empty.mark_stale(StaleReason::RootChanged);
    assert!(matches!(empty.state, HistoryState::Unloaded));
    let mut failed = History::default();
    failed.fail(error(storage_contract::ErrorCode::InjectedFailure));
    failed.mark_stale(StaleReason::RootChanged);
    assert!(matches!(failed.state, HistoryState::Unloaded));
}

#[test]
fn clear_and_invalidate_bump_the_generation_and_distinguish_content_from_path_changes() {
    let mut history = History::default();
    history.install(chain(1));
    let generation = history.generation;
    history.invalidate();
    assert_eq!(history.generation, generation + 1);
    assert!(history.loaded().is_some(), "a path change keeps the page");
    history.clear();
    assert_eq!(history.generation, generation + 2);
    assert!(history.window().is_none());
    assert!(matches!(history.state, HistoryState::Unloaded));
}

#[test]
fn a_branch_page_from_another_root_is_never_consulted() {
    let mut history = History::default();
    history.install(chain(1));
    let other = BranchPage::parse(
        {
            let mut wire = crate::files::history::tests::branch_page_wire(vec![], Some("x"));
            wire["history_root"] = crate::files::history::tests::OTHER_ROOT.into();
            wire
        },
        &HistoryBinding {
            root: HistoryRoot::parse(crate::files::history::tests::OTHER_ROOT).unwrap(),
            ..binding()
        },
    )
    .unwrap();
    history.branches = Some(other);
    assert!(history.branch_page().is_none());
    assert!(history.selected_branch().is_none());
    history.branches = Some(branch_page(vec![], Some("sel")));
    assert!(history.branch_page().is_some());
    assert_eq!(
        history.selected_branch(),
        None,
        "a candidate page does not select a location"
    );
    assert_eq!(history.root().unwrap().as_str(), ROOT);
}

#[test]
fn read_slots_hand_out_tickets_and_aborting_invalidates_the_previous_ticket() {
    let mut reads = HistoryReads::default();
    let before = reads.ticket(ReadSlot::Panel);
    assert!(reads.slots[ReadSlot::Panel.index()].cancellation.is_none());
    // No browser here: tickets are the observable part of the fence.
    reads.abort(ReadSlot::Panel);
    assert_eq!(reads.ticket(ReadSlot::Panel), before + 1);
    assert_eq!(reads.ticket(ReadSlot::Main), 0);
    reads.abort_all();
    assert_eq!(reads.ticket(ReadSlot::Main), 1);
    assert_eq!(reads.ticket(ReadSlot::Panel), before + 2);
    reads.finish(ReadSlot::Main);
    assert!(reads.slots[ReadSlot::Main.index()].cancellation.is_none());
}

#[test]
fn a_context_change_releases_a_cancelled_branch_load_instead_of_sticking_loading() {
    let mut history = History::default();
    history.install(chain(1));
    history.branch_read = BranchReadState::Loading;
    history.invalidate();
    assert!(history.needs_branches());
    history.branches = Some(branch_page(vec![], None));
    history.branch_read = BranchReadState::Loading;
    history.invalidate();
    assert!(matches!(history.branch_read, BranchReadState::Loaded));
    assert!(!history.needs_branches());
}
