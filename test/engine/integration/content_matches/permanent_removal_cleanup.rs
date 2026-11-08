//! Real purge receipts and on-disk proof/body mutations, not permanent-node deletion.
use super::*;
use storage_engine::{PermanentRemovalCleanupObservation, RemovalReplicaCleanupState};

fn containers(directory: &std::path::Path, file: &str, found: &mut Vec<std::path::PathBuf>) {
    let marker = directory.join(".zxs-container-owner.json");
    if marker.is_file() {
        let owner: String = json_codec::from_slice(&std::fs::read(marker).unwrap()).unwrap();
        if owner == file {
            found.push(directory.to_owned());
        }
        return;
    }
    for entry in std::fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            containers(&entry.path(), file, found);
        }
    }
}

fn fixture() -> (Pool, Entry, std::path::PathBuf, std::path::PathBuf) {
    let mut pool = Pool::new();
    pool.save(&[("a", b"shared"), ("b", b"shared")]);
    let a = pool.entry("a");
    pool.save(&[("a", b"new-a"), ("b", b"new-b")]);
    let version = &pool.engine.state().versions[&a.version_id];
    let history = pool.engine.config().disks[0]
        .root
        .join("histories")
        .join(&pool.history);
    let metadata = history.join("versions").join(&a.version_id);
    let mut found = Vec::new();
    containers(&history, &a.file_id, &mut found);
    assert_eq!(found.len(), 1);
    let body = found[0].join("versions").join(&version.storage_name);
    assert!(body.is_file());
    assert!(metadata.join("version.json").is_file());
    (pool, a, metadata, body)
}

fn inspect(pool: &Pool, entry: &Entry) -> PermanentRemovalCleanupObservation {
    let before = storage_contract::canonical_json(pool.engine.state()).unwrap();
    let captured = super::permanent_removal::read(pool, &entry.file_id);
    let result = pool
        .engine
        .inspect_permanent_removal_cleanup(
            &captured,
            &super::permanent_removal::selection(&[&entry.version_id]),
        )
        .unwrap();
    assert_eq!(result.replicas().len(), 1);
    assert_eq!(result.replicas()[0].version_id().as_str(), entry.version_id);
    assert_eq!(result.replicas()[0].slot_id(), "P01");
    assert_eq!(
        storage_contract::canonical_json(pool.engine.state()).unwrap(),
        before
    );
    result
}

fn purge(pool: &mut Pool, entry: &Entry) {
    let plan = pool
        .engine
        .create_purge_plan(&pool.history, vec![entry.version_id.clone()], "admin")
        .unwrap();
    pool.engine
        .commit_plan(&plan.plan_id, "admin", OperationId::generate().unwrap())
        .unwrap();
}

#[test]
fn removal_cleanup_observation_proves_current_copy_absence_after_purge_and_cold_reopen() {
    let (mut pool, a, metadata, body) = fixture();
    assert!(matches!(
        inspect(&pool, &a).replicas()[0].state(),
        RemovalReplicaCleanupState::NotRequested
    ));
    purge(&mut pool, &a);
    assert!(!body.exists());
    let proof = std::fs::read(metadata.join("purged.json")).unwrap();
    assert!(inspect(&pool, &a).all_current_routes_absent());
    let root = pool.root.clone();
    drop(pool.engine);
    pool.engine = Engine::open(&root).unwrap();
    assert!(inspect(&pool, &a).all_current_routes_absent());
    assert_eq!(std::fs::read(metadata.join("purged.json")).unwrap(), proof);
    // Body cleanup never retires records or splits equal-content file membership.
    assert!(pool.engine.state().versions.contains_key(&a.version_id));
    assert_eq!(
        super::permanent_removal::read(&pool, &a.file_id)
            .graph()
            .unwrap()
            .files
            .len(),
        2
    );
}

#[test]
fn removal_cleanup_observation_does_not_confuse_published_proof_with_missing_body() {
    let (mut pool, a, metadata, body) = fixture();
    purge(&mut pool, &a);
    std::fs::write(&body, b"shared").unwrap();
    let before = std::fs::read(&body).unwrap();
    let observed = inspect(&pool, &a);
    assert!(!observed.all_current_routes_absent());
    assert!(matches!(
        observed.replicas()[0].state(),
        RemovalReplicaCleanupState::PayloadPresent
    ));
    assert_eq!(std::fs::read(&body).unwrap(), before);

    // Both current supported body locations must be absent, not just the container.
    std::fs::remove_file(&body).unwrap();
    let prepared = metadata.join("content").join("a");
    std::fs::create_dir_all(prepared.parent().unwrap()).unwrap();
    std::fs::write(&prepared, b"shared").unwrap();
    let observed = inspect(&pool, &a);
    assert!(!observed.all_current_routes_absent());
    assert!(matches!(
        observed.replicas()[0].state(),
        RemovalReplicaCleanupState::PayloadPresent
    ));
    assert_eq!(std::fs::read(prepared).unwrap(), b"shared");
}

#[test]
fn removal_cleanup_observation_requires_authenticated_retained_evidence_not_just_absence() {
    let (mut pool, a, metadata, body) = fixture();
    purge(&mut pool, &a);
    let tombstone = metadata.join("purged.json");
    let saved = std::fs::read(&tombstone).unwrap();
    std::fs::remove_file(&tombstone).unwrap();
    assert!(!body.exists());
    let observed = inspect(&pool, &a);
    assert!(!observed.all_current_routes_absent());
    assert!(matches!(
        observed.replicas()[0].state(),
        RemovalReplicaCleanupState::MissingTombstone
    ));
    std::fs::write(&tombstone, b"{}").unwrap();
    let observed = inspect(&pool, &a);
    assert!(!observed.all_current_routes_absent());
    assert!(matches!(
        observed.replicas()[0].state(),
        RemovalReplicaCleanupState::Unverified(_)
    ));
    std::fs::write(&tombstone, saved).unwrap();
    assert!(inspect(&pool, &a).all_current_routes_absent());
    std::fs::remove_file(metadata.join("version.json")).unwrap();
    let observed = inspect(&pool, &a);
    assert!(!observed.all_current_routes_absent());
    assert!(matches!(
        observed.replicas()[0].state(),
        RemovalReplicaCleanupState::Unverified(_)
    ));
}

#[test]
fn removal_cleanup_observation_rejects_other_version_proof_and_symlink_body() {
    let (mut pool, a, metadata, body) = fixture();
    purge(&mut pool, &a);
    let tombstone = metadata.join("purged.json");
    let saved = std::fs::read(&tombstone).unwrap();
    let mut value: json_codec::Value = json_codec::from_slice(&saved).unwrap();
    value["version_id"] = json_codec::json!(pool.entry("b").version_id);
    std::fs::write(
        &tombstone,
        storage_contract::canonical_json(&value).unwrap(),
    )
    .unwrap();
    let observed = inspect(&pool, &a);
    assert!(!observed.all_current_routes_absent());
    assert!(matches!(
        observed.replicas()[0].state(),
        RemovalReplicaCleanupState::Unverified(_)
    ));
    std::fs::write(&tombstone, saved).unwrap();
    std::os::unix::fs::symlink(pool.root.join("not-a-body"), &body).unwrap();
    let observed = inspect(&pool, &a);
    assert!(!observed.all_current_routes_absent());
    assert!(matches!(
        observed.replicas()[0].state(),
        RemovalReplicaCleanupState::Unverified(_)
    ));
    assert!(
        std::fs::symlink_metadata(body)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn removal_cleanup_observation_reports_stopped_disk_without_touching_it() {
    let (mut pool, a, metadata, body) = fixture();
    purge(&mut pool, &a);
    let proof = std::fs::read(metadata.join("purged.json")).unwrap();
    let mut quiesce = pool.engine.inject_disk_quiesce("P01").unwrap();
    quiesce.release_control();
    assert!(quiesce.is_drained().unwrap());
    let observed = inspect(&pool, &a);
    assert!(!observed.all_current_routes_absent());
    assert!(matches!(
        observed.replicas()[0].state(),
        RemovalReplicaCleanupState::Unverified(error) if error.code() == ErrorCode::DiskUnavailable
    ));
    assert_eq!(std::fs::read(metadata.join("purged.json")).unwrap(), proof);
    assert!(!body.exists());
}

#[test]
fn removal_cleanup_observation_rejects_cancelled_and_stale_root_before_disk_evidence() {
    let (mut pool, a, _, _) = fixture();
    let captured = super::permanent_removal::read(&pool, &a.file_id);
    purge(&mut pool, &a);
    let selection = super::permanent_removal::selection(&[&a.version_id]);
    assert_eq!(
        pool.engine
            .inspect_permanent_removal_cleanup(&captured, &selection)
            .unwrap_err()
            .code(),
        ErrorCode::HistoryChanged
    );
    let cancellation = HistoryWindowCancellation::new();
    let request = storage_contract::shared_history::SharedHistoryRequest::parse(
        &a.file_id,
        pool.engine
            .metadata_view()
            .history_root(&pool.history)
            .unwrap(),
    )
    .unwrap();
    let captured = pool
        .engine
        .capture_shared_history(request, cancellation.clone())
        .unwrap();
    cancellation.cancel();
    assert_eq!(
        pool.engine
            .inspect_permanent_removal_cleanup(&captured, &selection)
            .unwrap_err()
            .code(),
        ErrorCode::OperationAborted
    );
}
