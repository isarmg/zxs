use super::*;
use storage_contract::State;

// Frozen pre-change Tree admission, independent of the shared production
// repository/head helpers. Full entries/pending projection cannot add a
// decision after these metadata checks and is intentionally not reproduced.
fn old_tree_admission(state: &State, input: &Value) -> Result<(), ApiError> {
    let branch = field(input, "branch_id")?;
    let branch_info = state.branches.get(branch).ok_or_else(|| {
        ApiError::new(
            ::http::StatusCode::NOT_FOUND,
            storage_contract::ErrorCode::NotFound,
            "branch not found",
        )
    })?;
    if branch_info.state == storage_contract::BranchState::Migrated {
        return Err(ApiError::new(
            ::http::StatusCode::CONFLICT,
            storage_contract::ErrorCode::TreeMoved,
            "branch has moved to an independent history domain",
        ));
    }
    let history = state
        .histories
        .get(&branch_info.history_tree_id)
        .ok_or_else(|| {
            ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::NotFound,
                "history not found",
            )
        })?;
    state
        .repositories
        .get(&history.repository_id)
        .ok_or_else(|| {
            ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::NotFound,
                "repository not found",
            )
        })?;
    state
        .commits
        .get(&branch_info.head_commit_id)
        .filter(|commit| commit.history_tree_id == branch_info.history_tree_id)
        .ok_or_else(|| {
            ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::NotFound,
                "commit not found",
            )
        })?;
    Ok(())
}

fn decision<T>(
    result: Result<T, ApiError>,
) -> Result<
    (),
    (
        ::http::StatusCode,
        storage_contract::ErrorCode,
        String,
        bool,
    ),
> {
    result
        .map(|_| ())
        .map_err(|error| (error.status, error.code, error.message, error.retryable))
}

#[test]
fn narrow_authorization_matches_old_tree_metadata_decisions_and_error_precedence() {
    let (_root, engine, repo, _) = fixture();
    let base = engine.state();
    let input = json!({"branch_id":repo.main_branch_id});
    let branch = &repo.main_branch_id;
    let head = base.branches[branch].head_commit_id.clone();
    for case in [
        "valid",
        "archived",
        "missing branch",
        "migrated",
        "missing history",
        "missing repository",
        "missing head",
        "foreign head",
        "migrated and missing history",
        "missing history and head",
        "missing repository and head",
    ] {
        let mut state = base.clone();
        match case {
            "valid" => {}
            "archived" => {
                state.branches.get_mut(branch).unwrap().state =
                    storage_contract::BranchState::Archived
            }
            "missing branch" => {
                state.branches.remove(branch);
            }
            "migrated" | "migrated and missing history" => {
                state.branches.get_mut(branch).unwrap().state =
                    storage_contract::BranchState::Migrated;
                if case.ends_with("history") {
                    state.histories.clear();
                }
            }
            "missing history" | "missing history and head" => {
                state.histories.clear();
                if case.ends_with("head") {
                    state.commits.remove_discard(&head);
                }
            }
            "missing repository" | "missing repository and head" => {
                state.repositories.clear();
                if case.ends_with("head") {
                    state.commits.remove_discard(&head);
                }
            }
            "missing head" => {
                state.commits.remove_discard(&head);
            }
            "foreign head" => {
                state.commits.get_mut(&head).unwrap().history_tree_id = "foreign".into();
            }
            _ => unreachable!(),
        }
        assert_eq!(
            decision(authorize_current(&state, &repo.main_branch_id)),
            decision(old_tree_admission(&state, &input)),
            "{case}",
        );
    }
}

#[test]
fn narrow_authorization_borrows_metadata_without_tree_projection_and_keeps_payload_fixed() {
    let (_root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "visible.txt", b"content");
    let input = json!({"branch_id":repo.main_branch_id});
    // Positive control: the work counter observes the old full projection.
    TREE_PROJECTION_WORK.with(|work| work.set((0, 0)));
    crate::TreeSnapshot::capture(&engine, crate::tests::tree_request(&input)).unwrap();
    assert_eq!(TREE_PROJECTION_WORK.with(|work| work.get()), (1, 1));
    for count in [0, 1, 100, 1_000, 10_000] {
        let (mut snapshot, _) = synthetic_commit(&mut engine, &repo, count);
        let mut state = engine.state().clone();
        let head = state.branches[&repo.main_branch_id].head_commit_id.clone();
        snapshot.commit_id = head.clone();
        state.commits.insert(head.clone(), snapshot);
        let (_, borrowed) = branch_snapshot(&state, &repo.main_branch_id, None).unwrap();
        assert!(std::ptr::eq(borrowed, &state.commits[&head]));
        assert_eq!(borrowed.namespace.entries as usize, count);
        TREE_PROJECTION_WORK.with(|work| work.set((0, 0)));
        for _ in 0..32 {
            authorize_current(&state, &repo.main_branch_id).unwrap();
        }
        assert_eq!(TREE_PROJECTION_WORK.with(|work| work.get()), (0, 0));
        assert_eq!(state.commits[&head].namespace.entries as usize, count);
    }
}

#[test]
fn narrow_authorization_rechecks_identity_sessions_and_binding_before_branch_errors() {
    let (root, engine, repo, _) = fixture();
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let input = json!({"branch_id":repo.main_branch_id});
    let binding = crate::auth::SmbBinding {
        repository_id: repo.repository_id.clone(),
        branch_id: repo.main_branch_id.clone(),
        generation: repo.smb_binding_generation,
    };
    let bound = Identity {
        smb_binding: Some(binding.clone()),
        ..identity.clone()
    };
    let assert_equal = |identity: &Identity, input: Value| {
        let old = backend
            .tree_snapshot(identity, crate::tests::tree_request(&input))
            .map(|snapshot| json_codec::to_value(snapshot).unwrap());
        let narrow = backend.authorize_current(identity, field(&input, "branch_id").unwrap());
        assert_eq!(decision(old), decision(narrow));
    };
    assert_equal(&bound, input.clone());
    assert_eq!(
        backend
            .authorize_current(&bound, &repo.main_branch_id)
            .unwrap(),
        ()
    );
    for kind in ["principal", "session", "repository", "branch", "generation"] {
        let mut denied = bound.clone();
        match kind {
            "principal" => denied.principal = "other".into(),
            "session" => denied.admission_session = Some("missing".into()),
            "repository" => denied.smb_binding.as_mut().unwrap().repository_id = "missing".into(),
            "branch" => denied.smb_binding.as_mut().unwrap().branch_id = "missing".into(),
            "generation" => denied.smb_binding.as_mut().unwrap().generation += 1,
            _ => unreachable!(),
        }
        assert_equal(&denied, input.clone());
        assert_equal(&denied, json!({"branch_id":"missing"}));
    }
    let snapshot = auth.snapshot().unwrap();
    for kind in ["expired", "credential generation"] {
        let mut revoked = snapshot.clone();
        if kind == "expired" {
            revoked["sessions"][identity.admission_session.as_ref().unwrap()]["expires_at"] =
                json!("0");
        } else {
            revoked["administrator"]["generation"] = json!("2");
        }
        backend
            .engine
            .lock()
            .unwrap()
            .persist_auth_state(
                revoked,
                "system_auth",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        assert_equal(&bound, input.clone());
        assert_eq!(
            backend
                .authorize_current(&bound, &repo.main_branch_id)
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::Unauthorized,
            "{kind}",
        );
    }
    backend
        .engine
        .lock()
        .unwrap()
        .persist_auth_state(
            snapshot,
            "system_auth",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    auth.logout(&token).unwrap();
    assert_equal(&bound, input);
    assert_eq!(
        backend
            .authorize_current(&bound, "missing")
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
}

#[test]
fn narrow_authorization_after_held_lock_binding_change_rechecks_current_generation() {
    let (_root, engine, repo, identity) = fixture();
    let backend = Arc::new(EngineBackend::new(engine));
    let bound = Identity {
        smb_binding: Some(crate::auth::SmbBinding {
            repository_id: repo.repository_id.clone(),
            branch_id: repo.main_branch_id.clone(),
            generation: repo.smb_binding_generation,
        }),
        ..identity
    };
    let mut held = backend.engine.lock().unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let worker_backend = backend.clone();
    let branch = repo.main_branch_id.clone();
    let worker = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        worker_backend.authorize_current(&bound, &branch)
    });
    started_rx.recv().unwrap();
    let generation = held.state().histories[&repo.history_tree_id].generation;
    held.set_smb_binding(
        &repo.repository_id,
        &repo.main_branch_id,
        true,
        generation,
        "owner",
        storage_contract::OperationId::generate(),
    )
    .unwrap();
    drop(held);
    assert_eq!(
        worker.join().unwrap().unwrap_err().code,
        storage_contract::ErrorCode::SmbRebindRequired
    );
}
