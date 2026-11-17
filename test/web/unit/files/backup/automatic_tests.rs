use super::*;
use json_codec::json;

fn draft() -> backup::Draft {
    backup::Draft {
        id: "loaded-editor".into(),
        repository: "repo_11111111111111111111111111111111".into(),
        path: "docs".into(),
        automatic_file: None,
        configuration: Some(
            backup::read_configuration(json!({
                "scope": {
                    "id": "entry_22222222222222222222222222222222",
                    "kind": "directory",
                    "path": "docs",
                },
                "generation": "7",
                "inherited": false,
                "target_copies": 3,
                "backup_prefixes": {"B01": "custom", "C01": "other"},
                "backup_routes": [["P01", "B01", "C01"]],
                "max_copies": 3,
                "backup_paths": {},
            }))
            .unwrap(),
        ),
        loading: false,
    }
}

#[test]
fn native_directory_request_uses_exact_identity_and_never_sends_prefixes_or_capture() {
    let draft = draft();
    let request = request(&draft).unwrap();
    assert_eq!(
        request.endpoint(),
        concat!(
            "/api/repositories/repo_11111111111111111111111111111111/",
            "automatic-directory-backup-configuration",
        )
    );
    assert_eq!(
        json_codec::to_value(request.body).unwrap(),
        json!({
            "path": "docs",
            "directory_id": "entry_22222222222222222222222222222222",
            "target_copies": 3,
            "expected_generation": "7",
        })
    );
    // The existing prefix action retains its independent request shape.
    let custom = backup::request(&draft).unwrap();
    assert_eq!(
        custom["backup_prefixes"],
        json!({"B01": "custom", "C01": "other"})
    );
    assert_eq!(
        custom["expected_scope_id"],
        "entry_22222222222222222222222222222222"
    );
    assert!(custom.get("directory_id").is_none());
}

#[test]
fn automatic_directory_action_keeps_scope_copy_and_current_session_gates() {
    let mut draft = draft();
    draft.path.clear();
    assert!(request(&draft).is_err());
    draft.path = "docs".into();
    draft.loading = true;
    assert!(request(&draft).is_err());
    draft.loading = false;
    let configuration = draft.configuration.as_mut().unwrap();
    configuration.scope.kind = NamespaceEntryKind::File;
    assert!(request(&draft).is_err());
    draft.configuration.as_mut().unwrap().scope.kind = NamespaceEntryKind::Directory;
    draft.configuration.as_mut().unwrap().target_copies = 4;
    assert!(request(&draft).is_err());
    draft.configuration.as_mut().unwrap().target_copies = 3;
    assert!(request(&draft).is_ok());

    Owner::new().with(|| {
        let state = RwSignal::new(State::default());
        state.update(|s| s.files.backup = Some(draft));
        submit(state);
        state.with_untracked(|s| {
            assert!(!s.files.mutation_pending);
            assert!(s.files.publications.is_empty());
        });
    });
}

#[test]
fn native_file_request_keeps_the_captured_occurrence_and_rejects_a_different_scope() {
    let mut draft = draft();
    let target = FileTarget {
        repository: draft.repository.parse().unwrap(),
        path: PortablePath::parse("docs/report.txt").unwrap(),
        entry: "entry_33333333333333333333333333333333".parse().unwrap(),
        file: "file_44444444444444444444444444444444".parse().unwrap(),
    };
    draft.path = target.path.to_string();
    let configuration = draft.configuration.as_mut().unwrap();
    configuration.scope.kind = NamespaceEntryKind::File;
    configuration.scope.id = target.file.to_string();
    draft.automatic_file = Some(target.clone());
    let frozen = request(&draft).unwrap();
    assert!(
        frozen
            .endpoint()
            .ends_with("/automatic-file-backup-configuration")
    );
    let body = json_codec::to_value(frozen.body).unwrap();
    assert_eq!(
        body,
        json!({
            "path": "docs/report.txt",
            "entry_id": "entry_33333333333333333333333333333333",
            "file_id": "file_44444444444444444444444444444444",
            "target_copies": 3,
            "expected_generation": "7",
        })
    );
    draft.configuration.as_mut().unwrap().scope.id = "file_55555555555555555555555555555555".into();
    assert!(request(&draft).is_err());
    draft.configuration.as_mut().unwrap().scope.id = target.file.to_string();
    draft.path = "docs/other.txt".into();
    assert!(request(&draft).is_err());
    draft.path = target.path.to_string();
    draft.repository = "repo_66666666666666666666666666666666".into();
    assert!(request(&draft).is_err());
    draft.repository = target.repository.to_string();
    draft.automatic_file = None;
    assert!(request(&draft).is_err());
}
