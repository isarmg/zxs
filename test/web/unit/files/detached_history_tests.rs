use super::*;
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId, HistoryRoot, HistoryTreeId, HistoryVersionId};
fn file() -> File {
    File {
        file_id: HistoryFileId::parse(&format!("file_{}", "a".repeat(32))).unwrap(),
        repository_id: HistoryRepositoryId::parse(&format!("repo_{}", "b".repeat(32))).unwrap(),
        history_tree_id: HistoryTreeId::parse(&format!("history_{}", "c".repeat(32))).unwrap(),
        history_root: HistoryRoot::parse(&"d".repeat(64)).unwrap(),
        representative_version_id: HistoryVersionId::parse(&format!("ver_{}", "e".repeat(32))).unwrap(),
        recorded_name: "old-name.txt".into(), representative_size: 3, retained_versions: 1,
    }
}
#[test]
fn retained_entry_carries_file_identity_without_forging_a_namespace_location() {
    let file = file();
    let entry = Entry::retained(&file);
    assert!(entry.retained_history() && entry.read_only());
    assert!(!entry.backup() && !entry.preview());
    assert_eq!(entry.file_id(), file.file_id.as_str());
    assert_eq!(entry.entry_id(), file.file_id.as_str());
    assert!(entry.context().is_none() && entry.repository_id().is_none() && entry.selected_branch_id().is_none());
    assert_eq!(entry.version_id(), Some(file.representative_version_id.as_str()));
    assert_eq!(entry.name(), "old-name.txt");
}
#[test]
fn stale_discovery_controls_do_not_open_history_in_another_session() {
    Owner::new().with(|| {
        let state = RwSignal::new(State { authenticated: true, session_epoch: 9, ..Default::default() });
        open(state, 8, &file());
        state.with_untracked(|s| {
            assert!(s.files.selected.is_none());
            assert_eq!(s.page, crate::ui::Page::Files);
        });
    });
}
#[test]
fn discovery_cursor_never_sends_a_claimed_current_file_path() {
    let file = file();
    let request = Request { snapshot: Some(file.history_root), after_file: Some(file.file_id.clone()) };
    let route = endpoint(&request);
    assert!(route.contains(file.file_id.as_str()));
    assert!(!route.contains("old-name") && !route.contains("repository_path"));
}
