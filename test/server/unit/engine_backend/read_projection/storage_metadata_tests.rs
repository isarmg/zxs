use super::*;

#[test]
fn typed_storage_metadata_is_owned_and_preserves_original_public_fields() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"first");
    let version = engine.list_tree(&repo.main_branch_id).unwrap()[0]
        .version_id
        .clone();
    let expected_placement = json_codec::to_value(&engine.state().placements[&version]).unwrap();
    let expected_topology = json!({"topology_id":"default","generation":engine.state().topology_generation.to_string(),"slots":engine.config().disks.iter().map(|disk|json!({"slot_id":disk.slot_id,"disk_id":disk.disk_id,"level":disk.level,"upstream_slot_id":disk.upstream_slot_id,"metadata_member":disk.metadata_member,"binding_generation":disk.binding_generation.to_string()})).collect::<Vec<_>>()});
    let (backend, auth, _token, identity) = authenticated_backend(engine, root.path());
    let placement = backend.placement_snapshot(&identity, &version).unwrap();
    let topology = backend.topology_snapshot(&identity).unwrap();
    save_projection_file(
        &mut backend.engine.lock().unwrap(),
        &repo.main_branch_id,
        "file.txt",
        b"second",
    );
    struct Probe<'a, T>(&'a EngineBackend, &'a T);
    impl<T: Serialize> Serialize for Probe<'_, T> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let unlocked = std::thread::scope(|scope| {
                scope
                    .spawn(|| self.0.engine.try_lock().is_ok())
                    .join()
                    .unwrap()
            });
            if !unlocked {
                return Err(S::Error::custom(
                    "storage metadata serialization retained engine mutex",
                ));
            }
            self.1.serialize(serializer)
        }
    }
    let placement = crate::backend_storage_metadata::PlacementResponse::from(placement);
    let topology = crate::backend_storage_metadata::TopologyResponse::from(topology);
    assert_eq!(
        json_codec::to_value(Probe(&backend, &placement)).unwrap(),
        expected_placement
    );
    assert_eq!(
        json_codec::to_value(Probe(&backend, &topology)).unwrap(),
        expected_topology
    );
    drop(auth);
    drop(backend);
    assert_eq!(json_codec::to_value(placement).unwrap(), expected_placement);
    assert_eq!(json_codec::to_value(topology).unwrap(), expected_topology);
}
#[test]
fn typed_storage_metadata_rechecks_current_authority_before_lookup() {
    let (root, engine, repo, _) = fixture();
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let missing = backend
        .placement_snapshot(&identity, "missing")
        .unwrap_err();
    assert_eq!(missing.code, storage_contract::ErrorCode::NotFound);
    assert_eq!(missing.message, "version not found");
    assert_eq!(backend.topology_snapshot(&identity).unwrap().slots.len(), 3);
    auth.logout(&token).unwrap();
    assert_eq!(
        backend.topology_snapshot(&identity).unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
    );
    assert_eq!(
        backend
            .placement_snapshot(&identity, &repo.repository_id)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
}
