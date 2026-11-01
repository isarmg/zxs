use super::*;
use json_codec::{Value, json};
use std::os::unix::fs::PermissionsExt;

fn finalize() -> Value {
    json!({"candidate_hash":"candidate", "accepts":[{
        "disk_id":"disk", "slot_id":"P01", "frame_hash":"frame",
        "record":{"record_type":"ACCEPT", "sequence":"1", "candidate_hash":"candidate",
                  "object_hash":"candidate", "previous_frame_hash":"previous"}
    }]})
}

#[test]
fn proof_digest_authenticates_complete_input_before_typed_projection() {
    let original = finalize();
    let hash = object_hash("finalize", &original).unwrap();
    for pointer in ["", "/accepts/0"] {
        let mut changed = original.clone();
        changed
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unsupported".into(), json!(true));
        assert_ne!(object_hash("finalize", &changed).unwrap(), hash);
        let bytes = canonical_json(&changed).unwrap();
        let error = decode_object::<Finalize>("finalize", &hash, &bytes).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ObjectCorrupt, "{pointer}");
        let actual = object_hash("finalize", &changed).unwrap();
        let error = decode_object::<Finalize>("finalize", &actual, &bytes).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidJson, "{pointer}");
        assert!(std::error::Error::source(&error).is_some());
    }
}

#[test]
fn proof_digest_does_not_hash_the_omitted_optional_field_projection() {
    let original = finalize();
    let hash = object_hash("finalize", &original).unwrap();
    let mut changed = original;
    changed["recovery_id"] = Value::Null;
    let error = decode_object::<Finalize>("finalize", &hash, &canonical_json(&changed).unwrap())
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
}

fn fixture() -> (tempfile::TempDir, Engine, FileVersion, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = Config::development([temp.path().join("disk")]).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("state"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("proof boundary").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "body", b"original")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "proof".into(),
            request_digest: None,
        })
        .unwrap();
    let id = engine.list_tree(&repo.main_branch_id).unwrap()[0]
        .version_id
        .clone();
    let version = engine.state().versions[&id].clone();
    let directory = version_dir(&engine.config.disks[0], &version.history_tree_id, &id);
    (temp, engine, version, directory)
}

#[test]
fn local_proof_rejects_unknown_fields_recursively_without_rewriting() {
    let (temp, engine, version, directory) = fixture();
    let path = directory.join("publication-proof.json");
    let original = std::fs::read(&path).unwrap();
    let value: Value = decode_strict(&original).unwrap();
    // Closed objects in a normal local proof, including structs already strict before this fix.
    for pointer in [
        "",
        "/candidate",
        "/candidate/versions/0",
        "/candidate/versions/0/version",
        "/candidate/versions/0/placement",
        "/candidate/history_roots",
        "/publication",
        "/publication/finalize",
        "/publication/finalize/accepts/0",
        "/publication/finalize/accepts/0/record",
        "/publication/finalize_receipts/0",
        "/publication/payload_receipts/0",
        "/publication/result",
        "/commit_proof",
        "/commit_proof/finalize",
        "/publication_receipts/0",
    ] {
        let mut changed = value.clone();
        changed
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unsupported".into(), json!("retain this evidence"));
        let bytes = canonical_json(&changed).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let error = verify_local_publication(&directory, &version).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidJson, "{pointer}");
        assert!(std::error::Error::source(&error).is_some());
        assert!(
            engine.open_content(&version.version_id).is_err(),
            "{pointer}"
        );
        let report = offline_check(&engine.config.disks[0].root).unwrap();
        assert_eq!(report.healthy_replicas, 0, "{pointer}");
        assert_eq!(report.issues.len(), 1, "{pointer}");
        let output = temp.path().join("export");
        assert!(
            offline_export(&engine.config.disks[0].root, &version.version_id, &output).is_err()
        );
        assert!(!output.exists());
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "{pointer}");
    }
    std::fs::write(path, original).unwrap();
    assert!(engine.open_content(&version.version_id).is_ok());
}

#[test]
fn recovery_rejects_modified_candidate_without_reencoding_or_repair() {
    let (temp, engine, _version, directory) = fixture();
    let local: LocalPublication = read_json(&directory.join("publication-proof.json")).unwrap();
    let disk = engine.config.disks[0].clone();
    let hash = object_hash("candidate", &local.candidate).unwrap();
    let path = object_path(&disk, "candidate", &hash);
    let mut changed: Value = decode_strict(&std::fs::read(&path).unwrap()).unwrap();
    changed["versions"][0]["unsupported"] = json!(true);
    let damaged = canonical_json(&changed).unwrap();
    std::fs::write(&path, &damaged).unwrap();
    let journal = std::fs::read(log_path(&disk)).unwrap();
    let publication = std::fs::read(directory.join("publication-proof.json")).unwrap();
    drop(engine);
    let error = match Engine::open(&temp.path().join("state")) {
        Ok(_) => panic!("modified candidate was accepted"),
        Err(error) => error,
    };
    assert_eq!(error.code(), ErrorCode::ObjectCorrupt, "{error:?}");
    assert_eq!(std::fs::read(path).unwrap(), damaged);
    assert_eq!(std::fs::read(log_path(&disk)).unwrap(), journal);
    assert_eq!(
        std::fs::read(directory.join("publication-proof.json")).unwrap(),
        publication
    );
}

#[test]
fn local_compound_proof_rejects_lossy_known_field_representations() {
    let (_temp, _engine, version, directory) = fixture();
    let path = directory.join("publication-proof.json");
    let original = std::fs::read(&path).unwrap();
    let value: Value = decode_strict(&original).unwrap();
    for (pointer, replacement) in [
        ("/publication/recovery", Value::Null),
        ("/publication/finalize/recovery_id", Value::Null),
        ("/commit_proof/finalize/recovery_id", Value::Null),
        ("/candidate/history_roots/kind", json!({"state-page":null})),
    ] {
        let mut changed = value.clone();
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        changed.pointer_mut(parent).unwrap()[key] = replacement;
        let bytes = canonical_json(&changed).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let error = verify_local_publication(&directory, &version).unwrap_err();
        assert_eq!(
            error.code(),
            ErrorCode::ObjectCorrupt,
            "{pointer}: {error:?}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn legal_proof_bytes_hashes_and_open_result_values_are_preserved() {
    let (_temp, _engine, _version, directory) = fixture();
    let bytes = std::fs::read(directory.join("publication-proof.json")).unwrap();
    let proof: LocalPublication = decode_proof(&bytes).unwrap();
    assert_eq!(canonical_json(&proof).unwrap(), bytes);
    let mut value: Value = decode_strict(&bytes).unwrap();
    value["publication"]["result"]["result"] =
        json!({"extension": {"payload": [null, "retained"]}});
    let decoded: LocalPublication = decode_proof(&canonical_json(&value).unwrap()).unwrap();
    assert_eq!(json_codec::to_value(decoded).unwrap(), value);
    let original = finalize();
    let expected = object_hash("finalize", &original).unwrap();
    let pretty = json_codec::to_vec_pretty(&original).unwrap();
    let decoded: Finalize = decode_object("finalize", &expected, &pretty).unwrap();
    assert_eq!(object_hash("finalize", &decoded).unwrap(), expected);
    assert_eq!(
        canonical_json(&decoded).unwrap(),
        canonical_json(&original).unwrap()
    );
}

#[test]
fn standalone_commit_and_exported_package_keep_rejected_evidence() {
    let (temp, engine, version, directory) = fixture();
    let commit_path = directory.join("commit-proof.json");
    let original = std::fs::read(&commit_path).unwrap();
    let mut changed: Value = decode_strict(&original).unwrap();
    changed["finalize"]["unsupported"] = json!(true);
    let damaged = canonical_json(&changed).unwrap();
    std::fs::write(&commit_path, &damaged).unwrap();
    let error = verify_local_publication(&directory, &version).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidJson);
    assert_eq!(std::fs::read(&commit_path).unwrap(), damaged);
    std::fs::write(&commit_path, original).unwrap();

    let package = temp.path().join("package");
    engine
        .export_history_package(&version.history_tree_id, &package)
        .unwrap();
    let path = package.join("publication.json");
    let original = std::fs::read(&path).unwrap();
    for pointer in ["/publication/finalize/accepts/0", "/candidate/versions/0"] {
        let mut changed: Value = decode_strict(&original).unwrap();
        changed.pointer_mut(pointer).unwrap()["unsupported"] = json!(true);
        let bytes = canonical_json(&changed).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let error = crate::offline_check_history_package(&package).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidJson, "{error:?}");
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    std::fs::write(path, original).unwrap();
    assert!(crate::offline_check_history_package(&package).is_ok());
}
