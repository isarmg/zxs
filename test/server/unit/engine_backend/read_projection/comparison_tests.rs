use super::*;

#[test]
fn typed_comparison_shares_captured_entries_and_serializes_after_unlock_and_revocation() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"first");
    let left_id = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let left = engine.state().commits.get_shared(&left_id).unwrap();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"second");
    let right_id = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let right = engine.state().commits.get_shared(&right_id).unwrap();
    let left_entries = engine.list_tree_at(&repo.main_branch_id, &left_id).unwrap();
    let right_entries = engine
        .list_tree_at(&repo.main_branch_id, &right_id)
        .unwrap();
    let expected = json!({"left":left_id,"right":right_id,"changes":[{"path":"file.txt","change":"modified","before":left_entries[0],"after":right_entries[0]}]});
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let before_left = Arc::strong_count(&left);
    let before_right = Arc::strong_count(&right);
    let snapshot = backend
        .compare_commits(
            &identity,
            crate::CompareCommits {
                left: &left_id,
                right: &right_id,
            },
        )
        .unwrap();
    assert_eq!(Arc::strong_count(&left), before_left + 1);
    assert_eq!(Arc::strong_count(&right), before_right + 1);
    let storage_contract::CommitChange::Modified { before, after } =
        snapshot.changes().next().unwrap()
    else {
        panic!("expected modified")
    };
    // The comparison owns its materialized namespaces; the compared records
    // must still be the exact published entries of each side.
    assert_eq!(before, &left_entries[0]);
    assert_eq!(after, &right_entries[0]);
    save_projection_file(
        &mut backend.engine.lock().unwrap(),
        &repo.main_branch_id,
        "file.txt",
        b"later",
    );
    let missing = backend
        .compare_commits(
            &identity,
            crate::CompareCommits {
                left: &left_id,
                right: "missing",
            },
        )
        .unwrap_err();
    assert_eq!(missing.code, storage_contract::ErrorCode::NotFound);
    assert_eq!(missing.message, "comparison commit not found");
    auth.logout(&token).unwrap();
    assert_eq!(
        backend
            .compare_commits(
                &identity,
                crate::CompareCommits {
                    left: "missing",
                    right: "missing"
                }
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    struct Probe<'a>(
        &'a EngineBackend,
        &'a crate::backend_comparison::ComparisonResponse,
    );
    impl Serialize for Probe<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let unlocked = std::thread::scope(|scope| {
                scope
                    .spawn(|| self.0.engine.try_lock().is_ok())
                    .join()
                    .unwrap()
            });
            if !unlocked {
                return Err(S::Error::custom(
                    "comparison serialization retained engine mutex",
                ));
            }
            self.1.serialize(serializer)
        }
    }
    let response = crate::backend_comparison::ComparisonResponse::from(snapshot);
    assert_eq!(
        json_codec::to_value(Probe(&backend, &response)).unwrap(),
        expected
    );
    drop(left);
    drop(right);
    drop(auth);
    drop(backend);
    assert_eq!(json_codec::to_value(response).unwrap(), expected);
}
