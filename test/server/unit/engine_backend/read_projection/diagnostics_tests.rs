use super::*;
use crate::backend_diagnostics::{CheckResponse, DisksResponse};

#[test]
fn typed_diagnostics_keep_observation_reports_and_current_admission() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"content");
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let expected_check = backend.engine.lock().unwrap().check().unwrap();
    let report = backend.integrity_check(&identity).unwrap();
    assert_eq!(
        json_codec::to_value(&report).unwrap(),
        json_codec::to_value(&expected_check).unwrap()
    );
    assert!(report.healthy_replicas > 0);
    let disks = backend.disk_status(&identity).unwrap();
    assert_eq!(disks.len(), 3);
    assert!(disks.iter().all(|disk| disk.online));
    let disk_wire = json!({"disks":disks});
    let check_wire = json_codec::to_value(&report).unwrap();
    let disks = DisksResponse::from(disks);
    let report = CheckResponse::from(report);
    std::fs::rename(root.path().join("d1"), root.path().join("removed-primary")).unwrap();
    let changed = backend.disk_status(&identity).unwrap();
    assert!(
        !changed
            .iter()
            .find(|disk| disk.slot_id == "P01")
            .unwrap()
            .online
    );
    assert!(
        !backend
            .integrity_check(&identity)
            .unwrap()
            .issues
            .is_empty()
    );
    // Authority publication still requires its real disk paths; restore the
    // temporary fixture before persisting revocation, rather than bypassing it.
    std::fs::rename(root.path().join("removed-primary"), root.path().join("d1")).unwrap();
    auth.logout(&token).unwrap();
    assert_eq!(
        backend.disk_status(&identity).unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
    );
    assert_eq!(
        backend.integrity_check(&identity).unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
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
                    "diagnostics serialization retained engine mutex",
                ));
            }
            self.1.serialize(serializer)
        }
    }
    assert_eq!(
        json_codec::to_value(Probe(&backend, &disks)).unwrap(),
        disk_wire
    );
    assert_eq!(
        json_codec::to_value(Probe(&backend, &report)).unwrap(),
        check_wire
    );
    drop(auth);
    drop(backend);
    assert_eq!(json_codec::to_value(disks).unwrap(), disk_wire);
    assert_eq!(json_codec::to_value(report).unwrap(), check_wire);
}
