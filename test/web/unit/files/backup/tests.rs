use super::*;

fn configuration_response() -> Value {
    json!({
        "scope":{"id":"scope","kind":"directory","path":"docs"},
        "policy_id":null,"effective_policy_id":"inherited",
        "generation":"0","effective_generation":"7","inherited":true,
        "target_copies":2,"backup_prefixes":{"B01":"preserved-prefix"},
        "route_slot_ids":["P01","B01"],"available_route_slot_ids":["P01","B01"],
        "backup_routes":[["P01","B01"]],"backup_slot_ids":["B01"],
        "max_copies":2,"file_count":"1","backup_paths":{"B01":"preserved-prefix/file"}
    })
}

#[test]
fn backup_configuration_rejects_lossy_prefix_projection_before_save() {
    Owner::new().with(|| {
        let mut admitted = Vec::new();
        for (case, prefixes) in [("missing", None), ("non-string", Some(json!({"B01":42})))] {
            let state = RwSignal::new(State {
                authenticated: true,
                session_epoch: 7,
                ..Default::default()
            });
            state.update(|s| {
                s.files.backup = Some(Draft {
                    id: "draft".into(),
                    repository: "repo".into(),
                    path: "docs".into(),
                    configuration: None,
                    loading: true,
                });
            });
            let mut response = configuration_response();
            match prefixes {
                Some(prefixes) => response["backup_prefixes"] = prefixes,
                None => {
                    response.as_object_mut().unwrap().remove("backup_prefixes");
                }
            }
            let result = read_configuration(response);
            assert_eq!(
                result.as_ref().err().unwrap().code,
                storage_contract::ErrorCode::InvalidResponse
            );
            complete_configuration_load(state, 7, "draft", result);
            state.with_untracked(|s| {
                let draft = s.files.backup.as_ref().unwrap();
                assert!(!draft.loading);
                if let Ok(body) = request(draft) {
                    admitted.push((case, body));
                }
                assert!(draft.configuration.is_none());
                assert_eq!(s.errors.len(), 1);
            });
        }
        assert!(
            admitted.is_empty(),
            "invalid reads became writable: {admitted:?}"
        );
    });
}

#[test]
fn backup_configuration_required_projection_is_not_defaulted_or_filtered() {
    for field in [
        "scope",
        "generation",
        "inherited",
        "target_copies",
        "backup_prefixes",
        "backup_routes",
        "max_copies",
        "backup_paths",
    ] {
        let mut value = configuration_response();
        value.as_object_mut().unwrap().remove(field);
        let error = read_configuration(value).err().expect(field);
        assert!(matches!(
            error.source.as_deref(),
            Some(api::ErrorSource::BackupConfiguration(_))
        ));
        use std::error::Error as _;
        assert!(error.source().unwrap().source().is_some());
    }
    for (field, invalid) in [
        ("backup_prefixes", json!(null)),
        ("backup_prefixes", json!({"B01":42})),
        (
            "backup_paths",
            json!([{"slot_id":"B01","logical_path":"old"}]),
        ),
        ("backup_paths", json!({"B01":42})),
        ("backup_routes", json!([["P01", 42]])),
        ("backup_routes", json!([[]])),
        ("backup_routes", json!([["P01", ""]])),
        ("max_copies", json!("2")),
        ("target_copies", json!("2")),
        ("generation", json!(0)),
    ] {
        let mut value = configuration_response();
        value[field] = invalid;
        assert!(
            read_configuration(value).is_err(),
            "accepted invalid {field}"
        );
    }
}

#[test]
fn backup_configuration_preserves_valid_prefixes_and_allows_explicit_capacity_reduction() {
    for policy_id in [Value::Null, json!("policy")] {
        let mut value = configuration_response();
        value["policy_id"] = policy_id.clone();
        value["effective_policy_id"] = policy_id;
        value["generation"] = json!(u64::MAX.to_string());
        let configuration = read_configuration(value).unwrap();
        assert!(configuration.inherited);
        assert_eq!(configuration.backup_paths["B01"], "preserved-prefix/file");
        let mut draft = Draft {
            id: "draft".into(),
            repository: "repo".into(),
            path: "docs".into(),
            configuration: Some(configuration),
            loading: false,
        };
        let body = request(&draft).unwrap();
        assert_eq!(body["backup_prefixes"], json!({"B01":"preserved-prefix"}));
        assert_eq!(body["expected_generation"], u64::MAX.to_string());
        // An absent slot within a valid map means the legitimate empty prefix.
        draft
            .configuration
            .as_mut()
            .unwrap()
            .backup_prefixes
            .clear();
        assert_eq!(
            request(&draft).unwrap()["backup_prefixes"],
            json!({"B01":""})
        );
        // Route changes may leave the original count above the current maximum.
        let configuration = draft.configuration.as_mut().unwrap();
        configuration.backup_routes = vec![vec!["P01".into()]];
        configuration.max_copies = 1;
        assert!(request(&draft).is_err());
        draft.configuration.as_mut().unwrap().target_copies = 1;
        assert!(request(&draft).is_ok());
    }
}

#[test]
fn navigation_discards_an_open_backup_review() {
    use crate::files::model::{Model, Navigation};
    for destination in [
        Navigation::Root,
        Navigation::Disk("B01".into()),
        Navigation::Directory("other".into()),
    ] {
        let mut model = Model {
            backup: Some(Draft {
                id: "draft".into(),
                repository: "repo".into(),
                path: "old-path".into(),
                configuration: None,
                loading: true,
            }),
            ..Default::default()
        };
        assert!(model.backup.is_some());
        model.navigate(destination);
        assert!(model.backup.is_none());
    }
}

fn unauthorized() -> api::Error {
    api::Error {
        code: storage_contract::ErrorCode::Unauthorized,
        message: "Session expired".into(),
        status: 401,
        retryable: false,
        request_id: Some("expired-backup".into()),
        source: None,
    }
}

#[test]
fn closed_backup_read_still_revokes_its_expired_session() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 7,
            ..Default::default()
        });
        // The user closed the panel before this request completed.
        assert!(state.with_untracked(|s| s.files.backup.is_none()));
        complete_configuration_load(state, 7, "closed-draft", Err(unauthorized()));
        state.with_untracked(|s| {
            assert!(!s.session_active());
            assert!(s.files.backup.is_none());
            assert_eq!(s.errors.len(), 1);
            assert_eq!(
                s.errors[0].feedback.request_id.as_deref(),
                Some("expired-backup")
            );
        });
    });
}

#[test]
fn stale_backup_rejection_cannot_revoke_a_replacement_session() {
    Owner::new().with(|| {
        for (epoch, transition) in [
            (8, crate::ui::SessionTransition::Idle),
            (7, crate::ui::SessionTransition::LoggingOut),
            (7, crate::ui::SessionTransition::LogoutUnconfirmed),
        ] {
            let state = RwSignal::new(State {
                authenticated: true,
                session_epoch: epoch,
                session_transition: transition,
                ..Default::default()
            });
            complete_configuration_load(state, 7, "closed-draft", Err(unauthorized()));
            state.with_untracked(|s| {
                assert!(s.authenticated);
                assert_eq!(s.session_transition, transition);
                assert!(s.errors.is_empty());
            });
        }
    });
}

#[test]
fn replaced_backup_panel_ignores_old_data_but_not_a_current_session_revocation() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 7,
            ..Default::default()
        });
        state.update(|s| {
            s.files.backup = Some(Draft {
                id: "new-draft".into(),
                repository: "repo".into(),
                path: "new-path".into(),
                configuration: None,
                loading: true,
            });
        });
        complete_configuration_load(
            state,
            7,
            "old-draft",
            read_configuration(configuration_response()),
        );
        let mut ordinary_error = unauthorized();
        ordinary_error.status = 503;
        complete_configuration_load(state, 7, "old-draft", Err(ordinary_error));
        state.with_untracked(|s| {
            assert!(s.session_active());
            assert!(s.errors.is_empty());
            let draft = s.files.backup.as_ref().unwrap();
            assert!(draft.loading);
            assert!(draft.configuration.is_none());
        });
        complete_configuration_load(state, 7, "old-draft", Err(unauthorized()));
        // A concurrent rejection must not report the same expired session twice.
        complete_configuration_load(state, 7, "new-draft", Err(unauthorized()));
        state.with_untracked(|s| {
            assert!(!s.session_active());
            assert_eq!(s.errors.len(), 1);
            let draft = s.files.backup.as_ref().unwrap();
            assert_eq!(draft.path, "new-path");
            assert!(draft.loading);
            assert!(draft.configuration.is_none());
        });
    });
}

#[test]
fn multi_primary_routes_use_each_selected_prefix_and_the_shortest_capacity() {
    let mut configuration = read_configuration(configuration_response()).unwrap();
    configuration.backup_routes = vec![
        vec!["P01".into(), "B01".into(), "C01".into()],
        vec!["P02".into(), "B02".into()],
    ];
    configuration.max_copies = 3;
    assert_eq!(max_copies(&configuration), 2);
    assert_eq!(target_slots(&configuration), vec!["B01", "B02"]);
    configuration.target_copies = 1;
    assert_eq!(target_slots(&configuration), Vec::<String>::new());
    configuration.target_copies = 2;
    configuration.backup_prefixes = BTreeMap::from([
        ("B01".into(), "first-chain".into()),
        ("B02".into(), "other-chain".into()),
    ]);
    let draft = Draft {
        id: "draft".into(),
        repository: "repo".into(),
        path: "docs".into(),
        configuration: Some(configuration),
        loading: false,
    };
    assert_eq!(
        request(&draft).unwrap()["backup_prefixes"],
        json!({"B01":"first-chain","B02":"other-chain"})
    );
    let mut invalid = draft;
    invalid.configuration.as_mut().unwrap().target_copies = 3;
    assert!(request(&invalid).is_err());
}
#[test]
fn independent_prefixes_are_relative_to_a_stable_scope_and_route() {
    let mut configuration = read_configuration(configuration_response()).unwrap();
    configuration.scope.id = "stable-directory".into();
    configuration.backup_routes = vec![vec!["P01".into(), "B01".into(), "C01".into()]];
    configuration.max_copies = 3;
    configuration.target_copies = 3;
    configuration.backup_prefixes = BTreeMap::from([
        ("B01".into(), "我的备份".into()),
        ("C01".into(), "长期保存".into()),
    ]);
    let draft = Draft {
        id: "draft".into(),
        repository: "repo".into(),
        path: "docs".into(),
        configuration: Some(configuration),
        loading: false,
    };
    let body = request(&draft).unwrap();
    assert_eq!(body["backup_prefixes"]["B01"], "我的备份");
    assert_eq!(body["backup_prefixes"]["C01"], "长期保存");
    assert_eq!(body["expected_scope_id"], "stable-directory");
    assert_eq!(body["expected_generation"], "0");
    assert!(valid_prefix(&"a".repeat(200)));
    assert!(!valid_prefix("../escape"));
    assert!(!valid_prefix("/absolute"));
    let mut invalid = draft;
    invalid.configuration.as_mut().unwrap().target_copies = 4;
    assert!(request(&invalid).is_err());
}

#[test]
fn backup_read_does_not_allocate_a_draft_for_an_inactive_session() {
    Owner::new().with(|| {
        for (authenticated, session_transition) in [
            (false, crate::ui::SessionTransition::Idle),
            (true, crate::ui::SessionTransition::Restoring),
            (true, crate::ui::SessionTransition::LoggingOut),
            (true, crate::ui::SessionTransition::LogoutUnconfirmed),
        ] {
            let state = RwSignal::new(State {
                authenticated,
                session_transition,
                ..Default::default()
            });
            start(state, "repo".into(), "docs".into());
            assert!(state.with_untracked(|s| s.files.backup.is_none()));
        }
    });
}

#[test]
fn backup_submission_keeps_session_and_storage_guards_without_role_fields() {
    Owner::new().with(|| {
        for (authenticated, transition, storage, writable) in [
            (
                false,
                crate::ui::SessionTransition::Idle,
                crate::management::Storage::Ready,
                true,
            ),
            (
                true,
                crate::ui::SessionTransition::Restoring,
                crate::management::Storage::Ready,
                true,
            ),
            (
                true,
                crate::ui::SessionTransition::LoggingOut,
                crate::management::Storage::Ready,
                true,
            ),
            (
                true,
                crate::ui::SessionTransition::LogoutUnconfirmed,
                crate::management::Storage::Ready,
                true,
            ),
            (
                true,
                crate::ui::SessionTransition::Idle,
                crate::management::Storage::ReadOnly,
                true,
            ),
            (
                true,
                crate::ui::SessionTransition::Idle,
                crate::management::Storage::Unavailable,
                true,
            ),
            (
                true,
                crate::ui::SessionTransition::Idle,
                crate::management::Storage::Ready,
                false,
            ),
        ] {
            let state = RwSignal::new(State {
                authenticated,
                session_transition: transition,
                ..Default::default()
            });
            state.update(|s| {
                s.management.storage = storage;
                s.files.writable = writable;
                s.files.backup = Some(Draft {
                    id: "draft".into(),
                    repository: "repo".into(),
                    path: "docs".into(),
                    configuration: Some(read_configuration(configuration_response()).unwrap()),
                    loading: false,
                });
            });
            submit(state);
            state.with_untracked(|s| {
                assert!(s.files.backup.is_some());
                assert!(!s.files.mutation_pending);
                assert!(s.files.publications.is_empty());
            });
        }
    });
}
