use super::*;

#[test]
fn write_admission_combines_session_location_and_publication_state() {
    let mut state = State {
        authenticated: true,
        ..Default::default()
    };
    state.files.writable = true;
    assert!(!write_allowed(&state), "unknown health fails closed");
    state.management.storage = crate::management::Storage::Ready;
    assert!(write_allowed(&state));
    state.session_transition = crate::ui::SessionTransition::Restoring;
    assert!(!write_allowed(&state));
    state.session_transition = crate::ui::SessionTransition::Idle;
    state.files.loading = true;
    assert!(!write_allowed(&state));
    state.files.loading = false;
    state.files.mutation_pending = true;
    assert!(!write_allowed(&state));
    state.files.mutation_pending = false;
    assert!(write_allowed(&state));
    crate::ui::begin_logout(&mut state);
    assert!(state.authenticated, "revocation has not been confirmed");
    assert!(!write_allowed(&state));
    state.session_transition = crate::ui::SessionTransition::Idle;
    state.authenticated = false;
    state.session_transition = crate::ui::SessionTransition::LogoutUnconfirmed;
    assert!(
        !write_allowed(&state),
        "failed logout permits revocation retry only"
    );
}

#[test]
fn immutable_directory_snapshots_never_authorize_default_publication() {
    use crate::files::model::{DirectoryPreview, DirectoryPreviewSource};
    let mut state = State {
        authenticated: true,
        ..Default::default()
    };
    state.management.storage = crate::management::Storage::Ready;
    state.files.directory_preview = Some(DirectoryPreview {
        source: DirectoryPreviewSource::Snapshot {
            branch_id: "validated-history-branch".into(),
            tree_id: "validated-history-tree".into(),
        },
        path: "Files/folder".into(),
    });
    assert!(!default_write_allowed(&state));
    assert!(!write_allowed(&state));
    // Even a stale writable listing must not turn a historical read into a
    // publishable current-branch review.
    state.files.writable = true;
    assert!(!default_write_allowed(&state));
    assert!(!write_allowed(&state));
}
