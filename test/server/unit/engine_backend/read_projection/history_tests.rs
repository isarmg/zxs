use super::*;

#[test]
fn typed_file_history_preserves_core_query_wire_and_admitted_snapshot_lifetime() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"first");
    let file = engine.list_tree(&repo.main_branch_id).unwrap()[0]
        .file_id
        .clone();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"second");
    let versions = engine.file_history(&file).unwrap();
    let expected = json!({"file_id":file,"versions":versions});
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let snapshot = backend.file_history(&identity, &file).unwrap();
    assert_eq!(snapshot, versions);
    assert!(
        snapshot
            .windows(2)
            .all(|pair| pair[0].version_sequence <= pair[1].version_sequence)
    );
    save_projection_file(
        &mut backend.engine.lock().unwrap(),
        &repo.main_branch_id,
        "file.txt",
        b"later",
    );
    let missing = backend.file_history(&identity, "missing").unwrap_err();
    assert_eq!(missing.code, storage_contract::ErrorCode::NotFound);
    assert_eq!(missing.message, "file history not found");
    auth.logout(&token).unwrap();
    assert_eq!(
        backend.file_history(&identity, &file).unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
    );
    struct Probe<'a>(
        &'a EngineBackend,
        &'a crate::backend_read::FileHistoryResponse,
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
                    "file history serialization retained engine mutex",
                ));
            }
            self.1.serialize(serializer)
        }
    }
    let response = crate::backend_read::FileHistoryResponse::new(file, snapshot);
    assert_eq!(
        json_codec::to_value(Probe(&backend, &response)).unwrap(),
        expected
    );
    assert_eq!(
        storage_contract::canonical_json(&response).unwrap(),
        storage_contract::canonical_json(&expected).unwrap()
    );
    drop(auth);
    drop(backend);
    assert_eq!(json_codec::to_value(response).unwrap(), expected);
}
