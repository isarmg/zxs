use super::*;
use crate::files::FileOperation;

#[test]
fn hidden_documents_do_not_start_and_resume_once_without_replaying_timer_debt() {
    let mut visibility = Visibility::default();
    assert_eq!(visibility.change(false), None);
    assert_eq!(visibility.change(true), Some(true));
    assert_eq!(visibility.change(true), None);
    assert_eq!(visibility.change(false), Some(false));
    assert_eq!(visibility.change(false), None);
    assert_eq!(visibility.change(true), Some(true));
}

#[test]
fn pausing_has_no_access_to_operation_receipts_or_admitted_reads() {
    let mut state = State {
        authenticated: true,
        ..Default::default()
    };
    state
        .files
        .operations
        .push(FileOperation::unknown("pending"));
    state.files.polling_operations.push("pending".into());
    state.management.loading = true;
    assert_eq!(demand(&state, false), Demand::default());
    assert_eq!(
        demand(&state, true),
        Demand {
            operations: true,
            management: true,
            telemetry: false,
        }
    );
    assert_eq!(state.files.operations[0].id(), "pending");
    assert_eq!(state.files.polling_operations, ["pending"]);
    assert!(state.management.loading);
    assert!(crate::files::pending(&state));
}

#[test]
fn every_page_retains_management_admission_but_only_status_polls_telemetry() {
    for page in [
        crate::ui::Page::Files,
        crate::ui::Page::Details,
        crate::ui::Page::Health,
        crate::ui::Page::Admin,
        crate::ui::Page::Status,
        crate::ui::Page::History,
        crate::ui::Page::Account,
    ] {
        let state = State {
            authenticated: true,
            page,
            ..Default::default()
        };
        assert_eq!(
            demand(&state, true),
            Demand {
                operations: true,
                management: true,
                telemetry: page == crate::ui::Page::Status,
            }
        );
        assert_eq!(demand(&state, false), Demand::default());
    }
}

#[test]
fn resume_cannot_bypass_session_restoration_or_uncertain_logout_fences() {
    use crate::ui::SessionTransition;
    for session_transition in [
        SessionTransition::Restoring,
        SessionTransition::LoggingOut,
        SessionTransition::LogoutUnconfirmed,
    ] {
        let state = State {
            authenticated: true,
            page: crate::ui::Page::Status,
            session_transition,
            ..Default::default()
        };
        assert_eq!(demand(&state, true), Demand::default());
    }
    let anonymous = State::default();
    assert_eq!(
        demand(&anonymous, true),
        Demand {
            operations: false,
            management: true,
            telemetry: false,
        }
    );
}
