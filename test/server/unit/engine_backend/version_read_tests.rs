use super::*;
use storage_contract::{CommitRequest, Entry, ErrorCode, OperationId, RepositoryName};

fn fixture() -> (tempfile::TempDir, Engine, Repository, Identity) {
    let root = crate::tests::private_tempdir();
    let config = storage_engine::Config::development([root.path().join("disk")]).unwrap();
    let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    let repo = engine
        .create_repository(
            &RepositoryName::parse("Version reads").unwrap(),
            "P01",
            1,
            "owner",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let identity = install_test_administrator(&mut engine, "owner");
    (root, engine, repo, identity)
}

fn save(engine: &mut Engine, branch: &str, body: &[u8]) -> Entry {
    let workspace = engine.create_workspace(branch, "owner").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", body)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "version read fixture".into(),
            request_digest: None,
        })
        .unwrap();
    engine
        .namespace_tree(branch)
        .unwrap()
        .get("file.txt")
        .unwrap()
        .clone()
}

// Independent pre-change lookup decision, including retired-before-history order.
fn previous_lookup(engine: &Engine, id: &str) -> Result<(), ApiError> {
    let version = engine
        .state()
        .versions
        .get(id)
        .filter(|version| {
            engine.state().content_states.get(&version.version_id) != Some(&ContentState::Retired)
        })
        .ok_or_else(|| {
            ApiError::new(
                http::StatusCode::NOT_FOUND,
                ErrorCode::NotFound,
                "version not found",
            )
        })?;
    repo_for_history(engine, &version.history_tree_id)?;
    Ok(())
}

fn decision<T>(result: Result<T, ApiError>) -> Result<(), (http::StatusCode, ErrorCode, String)> {
    result
        .map(|_| ())
        .map_err(|error| (error.status, error.code, error.message))
}

fn assert_rejected(backend: &EngineBackend, identity: &Identity, id: &str, code: ErrorCode) {
    for result in [
        backend.version_snapshot(identity, id).map(|_| ()),
        backend.authorize_version(identity, id),
        backend.placement_snapshot(identity, id).map(|_| ()),
    ] {
        let error = result.unwrap_err();
        assert_eq!(error.code, code);
        if code == ErrorCode::NotFound {
            assert_eq!(error.status, http::StatusCode::NOT_FOUND);
            assert_eq!(error.message, "version not found");
        }
    }
}

#[test]
fn version_reads_preserve_wire_borrows_and_missing_identifier_domains() {
    let (_root, mut engine, repo, identity) = fixture();
    let entry = save(&mut engine, &repo.main_branch_id, b"published content");
    let expected_record = engine.state().versions[&entry.version_id].clone();
    let expected_state = wire::stored_version_content_state(
        engine.state().content_states.get(&entry.version_id),
    );
    let (borrowed, stored) = authorized_version(&engine, &entry.version_id).unwrap();
    assert!(std::ptr::eq(borrowed, &engine.state().versions[&entry.version_id]));
    assert_eq!(wire::stored_version_content_state(stored.as_ref()), expected_state);
    let absent = VersionId::generate().unwrap().to_string();
    let wrong_prefix = entry.version_id.replacen("ver_", "file_", 1);
    let uppercase = entry.version_id.to_uppercase();
    for id in [
        entry.version_id.as_str(),
        absent.as_str(),
        "",
        "missing",
        "ver_0",
        "ver_0000000000000000000000000000000g",
        wrong_prefix.as_str(),
        uppercase.as_str(),
        " ver_00000000000000000000000000000000",
    ] {
        assert_eq!(
            decision(authorized_version(&engine, id)),
            decision(previous_lookup(&engine, id)),
        );
    }
    let backend = EngineBackend::new(engine);
    let captured = backend
        .version_snapshot(&identity, &entry.version_id)
        .unwrap();
    assert_eq!(captured.version, expected_record);
    assert_eq!(captured.content_state, expected_state);
    backend
        .authorize_version(&identity, &entry.version_id)
        .unwrap();
    let placement = backend
        .placement_snapshot(&identity, &entry.version_id)
        .unwrap();
    assert_eq!(placement.version_id, entry.version_id);
    for id in [absent.as_str(), "missing", wrong_prefix.as_str(), uppercase.as_str()] {
        assert_rejected(&backend, &identity, id, ErrorCode::NotFound);
    }
    let mut expected_wire = json_codec::to_value(&expected_record).unwrap();
    expected_wire["content_state"] = json_codec::to_value(expected_state).unwrap();
    {
        let mut engine = backend.engine.lock().unwrap();
        save(&mut engine, &repo.main_branch_id, b"later publication");
    }
    // The owned response is still encoded after releasing the admission lock.
    assert!(backend.engine.try_lock().is_ok());
    assert_eq!(
        json_codec::to_value(crate::backend_read::VersionResponse::from(captured)).unwrap(),
        expected_wire,
    );
}

#[test]
fn version_reads_hide_real_retirement_before_and_after_cold_open() {
    let (root, mut engine, repo, identity) = fixture();
    let retired = save(&mut engine, &repo.main_branch_id, b"old");
    let live = save(&mut engine, &repo.main_branch_id, b"current");
    let read = engine
        .capture_shared_history(
            storage_contract::shared_history::SharedHistoryRequest::parse(
                &retired.file_id,
                engine
                    .metadata_view()
                    .history_root(&repo.history_tree_id)
                    .unwrap(),
            )
            .unwrap(),
            storage_engine::HistoryWindowCancellation::new(),
        )
        .unwrap();
    let selection = storage_engine::PermanentNodeRemovalSelection::new(vec![
        storage_contract::history_window::HistoryVersionId::parse(&retired.version_id).unwrap(),
    ])
    .unwrap();
    engine
        .retire_file_nodes(&read, &selection, "owner", OperationId::generate().unwrap())
        .unwrap();
    drop(read);
    let verify = |engine: Engine| {
        let metadata = engine
            .version_metadata(retired.version_id.parse().unwrap())
            .unwrap();
        assert_eq!(metadata.1, Some(ContentState::Retired));
        assert_eq!(
            decision(authorized_version(&engine, &retired.version_id)),
            decision(previous_lookup(&engine, &retired.version_id)),
        );
        let backend = EngineBackend::new(engine);
        assert_rejected(&backend, &identity, &retired.version_id, ErrorCode::NotFound);
        assert_rejected(&backend, &identity, "malformed", ErrorCode::NotFound);
        assert!(backend.version_snapshot(&identity, &live.version_id).is_ok());
        assert!(backend.authorize_version(&identity, &live.version_id).is_ok());
        assert!(backend.placement_snapshot(&identity, &live.version_id).is_ok());
    };
    verify(engine);
    verify(Engine::open(&root.path().join("system")).unwrap());
}

#[test]
fn version_reads_keep_current_credentials_and_binding_before_lookup() {
    let (root, mut engine, repo, _) = fixture();
    let entry = save(&mut engine, &repo.main_branch_id, b"current");
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = crate::auth::AuthStore::initialize(
        &root.path().join("auth"),
        "owner",
        "integration-test-password",
    )
    .unwrap();
    let mut auth = backend.protect_auth(auth).unwrap();
    let token = auth
        .login("owner", "integration-test-password")
        .unwrap()
        .token;
    let identity = auth.authenticate(&token).unwrap();
    let stranger = Identity {
        principal: "stranger".into(),
        ..identity.clone()
    };
    let stale_binding = Identity {
        smb_binding: Some(crate::auth::SmbBinding {
            repository_id: repo.repository_id.clone(),
            branch_id: repo.main_branch_id.clone(),
            generation: repo.smb_binding_generation + 1,
        }),
        ..identity.clone()
    };
    let ids = [
        entry.version_id.as_str(),
        "malformed",
        "ver_00000000000000000000000000000000",
    ];
    for id in ids {
        assert_rejected(&backend, &stranger, id, ErrorCode::Unauthorized);
        assert_rejected(&backend, &stale_binding, id, ErrorCode::SmbRebindRequired);
    }
    auth.logout(&token).unwrap();
    for id in ids {
        assert_rejected(&backend, &identity, id, ErrorCode::Unauthorized);
    }
}
