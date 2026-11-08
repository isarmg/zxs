use super::*;
use crate::ui::Page;

#[test]
fn administration_sections_only_offer_retained_actions() {
    assert_eq!(
        Section::Repositories
            .actions()
            .iter()
            .map(|(kind, _)| *kind)
            .collect::<Vec<_>>(),
        vec![Kind::Repository]
    );
    assert_eq!(
        Section::Rules
            .actions()
            .iter()
            .map(|(kind, _)| *kind)
            .collect::<Vec<_>>(),
        vec![Kind::Policy, Kind::Rule]
    );
    assert_eq!(
        Section::Diagnostics
            .actions()
            .iter()
            .map(|(kind, _)| *kind)
            .collect::<Vec<_>>(),
        vec![Kind::Inspect]
    );
    assert!(!Section::Rules.contains(Kind::Repository));
    assert!(!Section::Repositories.contains(Kind::Inspect));
}

#[test]
fn menu_changes_keep_drafts_and_original_unknown_operation() {
    Owner::new().with(|| {
        let mut original = State {
            authenticated: true,
            ..Default::default()
        };
        original.admin_console.kind = Kind::Rule;
        original
            .admin_console
            .fields
            .insert("slot".into(), "P02".into());
        original.admin_console.result = "original receipt".into();
        original.admin_console.uncertain_operation =
            Some(UncertainOperation::Known("original".into()));
        let state = RwSignal::new(original);
        for page in [
            Page::Admin,
            Page::Repositories,
            Page::Rules,
            Page::Diagnostics,
            Page::Files,
        ] {
            state.update(|s| s.change_page(page));
            select(state, Kind::Repository);
            state.with_untracked(|s| {
                assert_eq!(s.page, page);
                assert_eq!(s.admin_console.kind, Kind::Rule);
                assert_eq!(s.admin_console.fields["slot"], "P02");
                assert_eq!(s.admin_console.result, "original receipt");
                assert_eq!(
                    s.admin_console.uncertain_operation,
                    Some(UncertainOperation::Known("original".into()))
                );
                assert!(s.admin_console.reload_blocked());
            });
        }
    });
}

#[test]
fn selecting_current_action_keeps_draft_and_pending_action_cannot_be_switched() {
    Owner::new().with(|| {
        let mut original = State {
            authenticated: true,
            ..Default::default()
        };
        original.admin_console.kind = Kind::Policy;
        original
            .admin_console
            .fields
            .insert("copies".into(), "3".into());
        original.admin_console.result = "reviewed output".into();
        let state = RwSignal::new(original);
        select(state, Kind::Policy);
        state.with_untracked(|s| {
            assert_eq!(s.admin_console.fields["copies"], "3");
            assert_eq!(s.admin_console.result, "reviewed output");
        });
        state.update(|s| s.admin_console.busy = true);
        select(state, Kind::Inspect);
        assert_eq!(state.with_untracked(|s| s.admin_console.kind), Kind::Policy);
        state.update(|s| s.admin_console.busy = false);
        select(state, Kind::Inspect);
        state.with_untracked(|s| {
            assert_eq!(s.admin_console.kind, Kind::Inspect);
            assert!(s.admin_console.result.is_empty());
        });
    });
}
