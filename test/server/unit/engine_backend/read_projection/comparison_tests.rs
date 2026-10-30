use super::*;

fn comparison_value(snapshot: &crate::ComparisonSnapshot) -> Value {
    json_codec::from_slice(snapshot.body()).unwrap()
}

#[test]
fn comparison_reads_and_encodes_detached_then_releases_publication_before_finish() {
    let (_root, mut engine, repo, identity) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"first");
    let left = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"second");
    let right = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let before = engine.list_tree_at(&repo.main_branch_id, &left).unwrap();
    let after = engine.list_tree_at(&repo.main_branch_id, &right).unwrap();
    let expected = json!({"left":left,"right":right,"changes":[{"path":"file.txt","change":"modified","before":before[0],"after":after[0]}]});
    let weak = Arc::downgrade(&engine.state().commits.get_shared(&left).unwrap());
    let backend = EngineBackend::new(engine);
    let view = backend.engine.lock().unwrap().published_read_view();
    let mut writer = backend.engine.lock().unwrap();
    save_projection_file(&mut writer, &repo.main_branch_id, "later.txt", b"later");
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        send.send(capture_comparison(
            view,
            crate::CompareCommits {
                left: &left,
                right: &right,
            },
            &tree_control(),
        ))
        .unwrap();
    });
    let result = receive.recv_timeout(std::time::Duration::from_secs(30));
    drop(writer);
    worker.join().unwrap();
    let snapshot = backend
        .finish_comparison(
            &identity,
            result.expect("comparison waited for writer mutex"),
            &tree_control(),
        )
        .unwrap();
    assert_eq!(comparison_value(&snapshot), expected);
    drop(backend);
    assert!(
        weak.upgrade().is_none(),
        "encoded comparison must not retain commits or its publication"
    );
    let allocation = snapshot.body().as_ptr();
    let encoded = snapshot.into_encoded();
    assert_eq!(encoded.body().as_ptr(), allocation);
    assert_eq!(
        json_codec::from_slice::<Value>(encoded.body()).unwrap(),
        expected
    );
}

#[test]
fn comparison_admission_preserves_left_history_repository_order_and_cross_repository_reads() {
    let (_root, mut engine, repo, identity) = fixture();
    let other = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Other").unwrap(),
            "P01",
            3,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let left = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let right = engine.state().branches[&other.main_branch_id]
        .head_commit_id
        .clone();
    let backend = EngineBackend::new(engine);
    let request = || crate::CompareCommits {
        left: &left,
        right: &right,
    };
    assert_eq!(
        comparison_value(
            &backend
                .compare_commits(&identity, request(), &tree_control())
                .unwrap()
        )["changes"],
        json!([])
    );
    for (missing, expected) in [
        ("history", "history not found"),
        ("repository", "repository not found"),
    ] {
        let mut engine = backend.engine.lock().unwrap();
        let original = engine.state().clone();
        let mut broken = original.clone();
        if missing == "history" {
            broken.histories.remove(&repo.history_tree_id);
        } else {
            broken.repositories.remove(&repo.repository_id);
        }
        storage_engine::test_support::replace_state(&mut engine, broken);
        drop(engine);
        let failure = backend
            .compare_commits(
                &identity,
                crate::CompareCommits {
                    left: &left,
                    right: "missing",
                },
                &tree_control(),
            )
            .unwrap_err();
        assert_eq!(failure.message, expected);
        storage_engine::test_support::replace_state(&mut backend.engine.lock().unwrap(), original);
    }
    assert_eq!(
        backend
            .compare_commits(
                &identity,
                crate::CompareCommits {
                    left: "missing",
                    right: &right
                },
                &tree_control()
            )
            .unwrap_err()
            .message,
        "comparison commit not found"
    );
}

#[test]
fn comparison_completion_reauthorizes_success_errors_budget_and_cancellation() {
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
        let view = backend.engine.lock().unwrap().published_read_view();
        let commit = view.state().branches[&repo.main_branch_id]
            .head_commit_id
            .clone();
        let request = || crate::CompareCommits {
            left: &commit,
            right: &commit,
        };
        let snapshot = capture_comparison(view, request(), &tree_control());
        let failure = || {
            ApiError::new(
                ::http::StatusCode::INTERNAL_SERVER_ERROR,
                storage_contract::ErrorCode::ObjectCorrupt,
                "comparison namespace is corrupt",
            )
        };
        assert_eq!(
            backend
                .finish_comparison(&identity, Err(failure()), &tree_control())
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::ObjectCorrupt
        );
        let cancelled = tree_control();
        drop(cancelled.wait_owner());
        assert_eq!(
            backend
                .compare_commits(&identity, request(), &cancelled)
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::RequestTimeout
        );
        auth.logout(&token).unwrap();
        for result in [
            snapshot,
            Err(failure()),
            Err(ApiError::limit("comparison byte budget exhausted")),
        ] {
            assert_eq!(
                backend
                    .finish_comparison(&identity, result, &tree_control())
                    .unwrap_err()
                    .code,
                storage_contract::ErrorCode::Unauthorized
            );
        }
        assert_eq!(
            backend
                .finish_comparison(&identity, Err(failure()), &cancelled)
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::Unauthorized
        );
        assert_eq!(
            backend
                .compare_commits(
                    &identity,
                    crate::CompareCommits {
                        left: "missing",
                        right: "missing"
                    },
                    &tree_control()
                )
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::Unauthorized
        );
    }
}
