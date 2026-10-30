use super::*;
use crate::tests::directory_scopes_value;
use crate::{DirectoryBranchFilter, DirectoryBranchProjection, ReadDirectoryScopes};

fn legacy_full(engine: &Engine, repo: &str) -> Value {
    let mut branches = engine
        .state()
        .directory_branches
        .values()
        .filter(|branch| branch.repository_id == repo)
        .collect::<Vec<_>>();
    branches.sort_by_key(|branch| {
        (
            std::cmp::Reverse(branch.sequence),
            branch.directory_branch_id.as_str(),
        )
    });
    let rows = branches
        .into_iter()
        .map(|branch| {
            let mut row = json_codec::to_value(branch).unwrap();
            row["current"] = json!(
                engine
                    .state()
                    .directory_selections
                    .get(&branch.directory_id)
                    == Some(&branch.directory_branch_id)
            );
            row["file_count"] = json!(
                branch
                    .entries
                    .iter()
                    .filter(|entry| entry.kind == storage_contract::NamespaceEntryKind::File)
                    .count()
                    .to_string()
            );
            row
        })
        .collect::<Vec<_>>();
    json!({"repository_id":repo,"directory_branches":rows})
}
fn compact(full: &Value) -> Value {
    let rows = full["directory_branches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let keys = [
                "directory_branch_id",
                "repository_id",
                "directory_id",
                "name",
                "parent_branch_id",
                "head_tree_id",
                "commit_id",
                "path",
                "materialized",
                "current",
                "sequence",
                "file_count",
            ];
            Value::Object(
                keys.into_iter()
                    .map(|key| (key.into(), row[key].clone()))
                    .collect(),
            )
        })
        .collect::<Vec<_>>();
    json!({"repository_id":full["repository_id"],"directory_branches":rows})
}
#[test]
fn directory_scopes_capture_requested_shape_once_and_outlive_engine() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(
        &mut engine,
        &repo.main_branch_id,
        "Café/file.txt",
        b"content",
    );
    for path in ["", "Café"] {
        engine
            .initialize_directory_scope(
                &repo.repository_id,
                (if path.is_empty() {
                    None
                } else {
                    Some(name_policy::LookupPath::parse(path).unwrap())
                })
                .as_ref(),
                "owner",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        engine
            .create_directory_branches(
                &repo.repository_id,
                (if (path).is_empty() {
                    None
                } else {
                    Some(name_policy::LookupPath::parse(path).unwrap())
                })
                .as_ref(),
                &storage_contract::BranchName::parse("frozen").unwrap(),
                "owner",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
    }
    let expected = legacy_full(&engine, &repo.repository_id);
    let expected_summary = compact(&expected);
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let request = |projection| ReadDirectoryScopes {
        repository_id: &repo.repository_id,
        filter: &DirectoryBranchFilter::All,
        projection,
    };
    let full = backend
        .directory_scopes(
            &identity,
            request(DirectoryBranchProjection::Full),
            &crate::tests::history_read_control(),
        )
        .unwrap();
    let summary = backend
        .directory_scopes(
            &identity,
            request(DirectoryBranchProjection::Summary),
            &crate::tests::history_read_control(),
        )
        .unwrap();
    assert_eq!(
        directory_scopes_value(&full)["directory_branches"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        directory_scopes_value(&summary)["directory_branches"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    let root_only = backend
        .directory_scopes(
            &identity,
            ReadDirectoryScopes {
                repository_id: &repo.repository_id,
                filter: &DirectoryBranchFilter::Root,
                projection: DirectoryBranchProjection::Summary,
            },
            &crate::tests::history_read_control(),
        )
        .unwrap();
    assert!({
        let value = directory_scopes_value(&root_only);
        let rows = value["directory_branches"].as_array().unwrap();
        rows.len() == 2
            && rows
                .iter()
                .all(|row| row["directory_id"] == repo.repository_id)
    });
    let nfd =
        DirectoryBranchFilter::Directory(name_policy::LookupPath::parse("CAFE\u{301}").unwrap());
    let nested = backend
        .directory_scopes(
            &identity,
            ReadDirectoryScopes {
                repository_id: &repo.repository_id,
                filter: &nfd,
                projection: DirectoryBranchProjection::Summary,
            },
            &crate::tests::history_read_control(),
        )
        .unwrap();
    assert!({
        let value = directory_scopes_value(&nested);
        let rows = value["directory_branches"].as_array().unwrap();
        rows.len() == 2 && rows.iter().all(|row| row["path"] == "Café")
    });
    backend
        .engine
        .lock()
        .unwrap()
        .create_directory_branches(
            &repo.repository_id,
            None,
            &storage_contract::BranchName::parse("later").unwrap(),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert!(backend.engine.try_lock().is_ok());
    assert_eq!(directory_scopes_value(&full), expected);
    assert_eq!(directory_scopes_value(&summary), expected_summary);
    assert_eq!(
        storage_contract::canonical_json(&directory_scopes_value(&full)).unwrap(),
        storage_contract::canonical_json(&expected).unwrap()
    );
    let missing = || ReadDirectoryScopes {
        repository_id: "missing",
        filter: &DirectoryBranchFilter::All,
        projection: DirectoryBranchProjection::Full,
    };
    let error = backend
        .directory_scopes(&identity, missing(), &crate::tests::history_read_control())
        .err()
        .unwrap();
    assert_eq!(error.code, storage_contract::ErrorCode::NotFound);
    assert_eq!(error.message, "repository not found");
    auth.logout(&token).unwrap();
    assert_eq!(
        backend
            .directory_scopes(&identity, missing(), &crate::tests::history_read_control())
            .err()
            .unwrap()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    drop(auth);
    drop(backend);
    assert_eq!(directory_scopes_value(&full), expected);
    assert_eq!(directory_scopes_value(&summary), expected_summary);
}

#[test]
fn detached_scopes_encode_once_and_release_root_while_publication_advances() {
    let (_root, mut engine, repo, identity) = fixture();
    save_projection_file(
        &mut engine,
        &repo.main_branch_id,
        "Café/file.txt",
        b"before",
    );
    engine
        .initialize_directory_scope(
            &repo.repository_id,
            None,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let id = engine.state().directory_selections[&repo.repository_id].clone();
    let head = &engine.state().branches[&repo.main_branch_id].head_commit_id;
    let commit = engine.state().commits.get_shared(head).unwrap();
    let weak = Arc::downgrade(&commit);
    drop(commit);
    let expected = legacy_full(&engine, &repo.repository_id);
    let backend = EngineBackend::new(engine);
    let view = backend.engine.lock().unwrap().published_read_view();
    {
        let original = &view.state().directory_branches[&id];
        let row = crate::DirectoryScopeBranch {
            branch: original,
            current: true,
            file_count: 1,
        };
        assert!(std::ptr::eq(
            row.branch,
            &view.state().directory_branches[&id]
        ));
        assert_eq!(row.branch.entries.as_ptr(), original.entries.as_ptr());
        let encoded = crate::DirectoryScopesSnapshot::encode(
            &repo.repository_id,
            DirectoryBranchProjection::Full,
            &[row],
        )
        .unwrap();
        assert_eq!(directory_scopes_value(&encoded), expected);
    }
    let mut writer = backend.engine.lock().unwrap();
    writer
        .rename_directory_branch(
            &repo.repository_id,
            &id,
            &storage_contract::BranchName::parse("renamed").unwrap(),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    save_projection_file(&mut writer, &repo.main_branch_id, "later.txt", b"later");
    let repository_id = repo.repository_id.clone();
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        send.send(crate::engine_backend::directory_scopes::read(
            view,
            ReadDirectoryScopes {
                repository_id: &repository_id,
                filter: &DirectoryBranchFilter::All,
                projection: DirectoryBranchProjection::Full,
            },
            &crate::tests::history_read_control(),
        ))
        .unwrap();
    });
    let result = receive.recv_timeout(std::time::Duration::from_secs(30));
    drop(writer);
    worker.join().unwrap();
    let snapshot = backend
        .finish_directory_scopes(
            &identity,
            result.expect("scope scan waited for writer mutex"),
            &crate::tests::history_read_control(),
        )
        .unwrap();
    assert_eq!(directory_scopes_value(&snapshot), expected);
    assert_eq!(
        directory_scopes_value(&snapshot)["directory_branches"][0]["file_count"],
        "1"
    );
    drop(backend);
    assert!(
        weak.upgrade().is_none(),
        "encoded snapshot must not retain the old publication"
    );
    let allocation = snapshot.body().as_ptr();
    let encoded = snapshot.into_encoded();
    assert_eq!(
        encoded.body().as_ptr(),
        allocation,
        "HTTP drain must move the already encoded allocation"
    );
    assert_eq!(
        json_codec::from_slice::<Value>(encoded.body()).unwrap(),
        expected
    );
}

#[test]
fn scope_completion_reauthorizes_success_errors_and_cancellation() {
    for managed in [false, true] {
        let (root, engine, repo, _) = fixture();
        let backend = Arc::new(EngineBackend::new(engine));
        let auth =
            AuthStore::initialize(root.path(), "owner", "integration-test-password").unwrap();
        let mut auth = if managed {
            backend.set_management_auth(auth.reader()).unwrap();
            auth
        } else {
            backend.protect_auth(auth).unwrap()
        };
        let token = auth
            .login("owner", "integration-test-password")
            .unwrap()
            .token;
        let identity = auth.authenticate(&token).unwrap();
        let request = || ReadDirectoryScopes {
            repository_id: &repo.repository_id,
            filter: &DirectoryBranchFilter::All,
            projection: DirectoryBranchProjection::Full,
        };
        let control = crate::tests::history_read_control();
        let snapshot = crate::engine_backend::directory_scopes::read(
            backend.engine.lock().unwrap().published_read_view(),
            request(),
            &control,
        );
        let failure = || {
            ApiError::new(
                ::http::StatusCode::INTERNAL_SERVER_ERROR,
                storage_contract::ErrorCode::ObjectCorrupt,
                "scope namespace is corrupt",
            )
        };
        assert_eq!(
            backend
                .finish_directory_scopes(&identity, Err(failure()), &control)
                .err()
                .unwrap()
                .code,
            storage_contract::ErrorCode::ObjectCorrupt
        );
        let cancelled = crate::tests::history_read_control();
        drop(cancelled.wait_owner());
        assert_eq!(
            backend
                .directory_scopes(&identity, request(), &cancelled)
                .err()
                .unwrap()
                .code,
            storage_contract::ErrorCode::RequestTimeout
        );
        assert_eq!(
            crate::engine_backend::directory_scopes::read(
                backend.engine.lock().unwrap().published_read_view(),
                request(),
                &cancelled
            )
            .err()
            .unwrap()
            .code,
            storage_contract::ErrorCode::RequestTimeout
        );
        auth.logout(&token).unwrap();
        for result in [snapshot, Err(failure())] {
            assert_eq!(
                backend
                    .finish_directory_scopes(&identity, result, &control)
                    .err()
                    .unwrap()
                    .code,
                storage_contract::ErrorCode::Unauthorized
            );
        }
        assert_eq!(
            backend
                .finish_directory_scopes(&identity, Err(failure()), &cancelled)
                .err()
                .unwrap()
                .code,
            storage_contract::ErrorCode::Unauthorized
        );
    }
}

#[tokio::test]
async fn borrowed_scope_wire_preserves_empty_fields_bytes_and_response_lease() {
    use futures::StreamExt;
    let (_root, mut engine, repo, _) = fixture();
    engine
        .initialize_directory_scope(
            &repo.repository_id,
            None,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let mut branch = engine
        .state()
        .directory_branches
        .values()
        .next()
        .unwrap()
        .clone();
    branch.entries.clear();
    branch.file_branches.clear();
    branch.file_contexts.clear();
    branch.inherited_files.clear();
    branch.directory_refs.clear();
    branch.sequence = u64::MAX;
    let current = true;
    let mut branch_bytes = json_codec::to_vec(&branch).unwrap();
    assert_eq!(branch_bytes.pop(), Some(b'}'));
    branch_bytes
        .extend_from_slice(format!(",\"current\":{},\"file_count\":\"0\"}}", current).as_bytes());
    let mut expected = format!(
        "{{\"repository_id\":{},\"directory_branches\":[",
        json_codec::to_string(&repo.repository_id).unwrap()
    )
    .into_bytes();
    expected.extend_from_slice(&branch_bytes);
    expected.extend_from_slice(b"]}");
    let snapshot = crate::DirectoryScopesSnapshot::encode(
        &repo.repository_id,
        DirectoryBranchProjection::Full,
        &[crate::DirectoryScopeBranch {
            branch: &branch,
            current,
            file_count: 0,
        }],
    )
    .unwrap();
    let allocation = snapshot.body().as_ptr();
    let encoded = snapshot.into_encoded();
    assert_eq!(encoded.body().as_ptr(), allocation);
    assert_eq!(encoded.body(), expected);
    let value: Value = json_codec::from_slice(encoded.body()).unwrap();
    let wire = &value["directory_branches"][0];
    assert_eq!(wire["entries"], json!([]));
    assert_eq!(wire["file_branches"], json!({}));
    for key in ["file_contexts", "inherited_files", "directory_refs"] {
        assert!(wire.get(key).is_none());
    }
    assert_eq!(wire["sequence"], u64::MAX.to_string());
    let slots = Arc::new(tokio::sync::Semaphore::new(1));
    let limits = crate::TransferLimits::default();
    let response = encoded.into_leased_response(
        slots.clone().try_acquire_owned().unwrap(),
        &limits,
        tokio::time::Instant::now() + limits.total_timeout,
        None,
    );
    let mut stream = response.into_body().into_data_stream();
    let chunk = stream.next().await.unwrap().unwrap();
    assert!(stream.next().await.is_none());
    drop(stream);
    assert_eq!(slots.available_permits(), 0);
    drop(chunk);
    assert_eq!(slots.available_permits(), 1);
    branch.name = "x".repeat(storage_contract::client_protocol::MAX_RESPONSE_BYTES);
    assert_eq!(
        crate::DirectoryScopesSnapshot::encode(
            &repo.repository_id,
            DirectoryBranchProjection::Full,
            &[crate::DirectoryScopeBranch {
                branch: &branch,
                current,
                file_count: 0
            }],
        )
        .err()
        .unwrap()
        .code,
        storage_contract::ErrorCode::ResourceLimitExceeded
    );
}
