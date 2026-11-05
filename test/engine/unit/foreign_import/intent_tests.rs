//! Real source/target filesystem boundaries for immutable execution intents.
use super::*;
use crate::foreign_import::tests::{Fixture, fixture, inspect};
use std::fs;

fn target(f: &Fixture) -> Engine {
    Engine::initialize(
        &f._temp.path().join("target-system"),
        Config::development([f._temp.path().join("target-primary")]).unwrap(),
    )
    .unwrap()
}
fn source(f: &Fixture) -> ForeignImportIntentSource {
    ForeignImportIntentSource::capture(inspect(&f.source).unwrap()).unwrap()
}
fn names(source: &ForeignImportIntentSource) -> BTreeMap<String, String> {
    source
        .preview()
        .histories()
        .iter()
        .enumerate()
        .map(|(index, history)| (history.group_id().into(), format!("history-{index}")))
        .collect()
}
fn request(operation: OperationId, actor: &str) -> ForeignImportIntentRequest {
    ForeignImportIntentRequest {
        operation_id: operation,
        review_operation_id: "op_00000000000000000000000000000000".parse().unwrap(),
        review_digest: "e".repeat(64).parse().unwrap(),
        principal: actor.into(),
    }
}
fn prepare(
    f: &Fixture,
    engine: &Engine,
    operation: OperationId,
    actor: &str,
) -> PreparedForeignImportIntent {
    let source = source(f);
    let target = ForeignImportIntentTarget::new("P01".into(), None, names(&source)).unwrap();
    engine
        .prepare_foreign_import_intent(source, target, request(operation, actor))
        .unwrap()
}
fn save(engine: &mut Engine, prepared: PreparedForeignImportIntent) -> ForeignImportIntentInfo {
    let digest = *prepared.digest();
    engine
        .save_foreign_import_intent(prepared.confirm_save(&digest).unwrap(), "admin")
        .unwrap()
}
fn path(engine: &Engine, operation: OperationId) -> PathBuf {
    engine.config().disks[0]
        .root
        .join(DIRECTORY)
        .join(operation.to_string())
}

#[test]
fn complete_multi_origin_intent_is_target_local_and_survives_without_its_source() {
    let f = fixture();
    let mut engine = target(&f);
    let operation = OperationId::generate().unwrap();
    let prepared = prepare(&f, &engine, operation, "admin");
    assert_eq!(prepared.history_count(), 2);
    assert!(prepared.temporarily_unavailable_versions() > 0);
    let target = prepared.target_identity().clone();
    let before = canonical_json(engine.state()).unwrap();
    let journal = log_path(&engine.config().disks[0]);
    let journal_before = fs::read(&journal).unwrap();
    let seal = fs::read(f.source.join("disk.json")).unwrap();
    let saved = save(&mut engine, prepared);
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert_eq!(fs::read(journal).unwrap(), journal_before);
    assert_eq!(fs::read(f.source.join("disk.json")).unwrap(), seal);
    assert!(engine.state().operations.get(&operation).is_none());
    let record = fs::read(path(&engine, operation).join("intent.json")).unwrap();
    let duplicate = prepare(&f, &engine, operation, "admin");
    assert_eq!(save(&mut engine, duplicate), saved);
    assert_eq!(
        fs::read(path(&engine, operation).join("intent.json")).unwrap(),
        record
    );
    let root = engine.root.clone();
    drop(engine);
    fs::rename(&f.source, f.source.with_extension("detached")).unwrap();
    let mut engine = Engine::open(&root).unwrap();
    assert_eq!(
        engine
            .foreign_import_intent(&target, operation, "admin")
            .unwrap(),
        Some(saved.clone())
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    engine
        .create_repository(
            &RepositoryName::parse("later-unrelated").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    assert_eq!(
        engine
            .reconcile_foreign_import_intent(&target, operation, "admin", saved.digest())
            .unwrap(),
        Some(saved)
    );
    assert_eq!(
        engine
            .foreign_import_intent(&target, operation, "other")
            .unwrap_err()
            .code(),
        ErrorCode::PermissionDenied
    );
}

#[test]
fn omitted_history_or_wrong_confirmation_never_creates_a_target_intent() {
    let f = fixture();
    let engine = target(&f);
    let source = source(&f);
    let mut mapping = names(&source);
    mapping.pop_first();
    let target = ForeignImportIntentTarget::new("P01".into(), None, mapping).unwrap();
    assert!(
        engine
            .prepare_foreign_import_intent(
                source,
                target,
                request(OperationId::generate().unwrap(), "admin"),
            )
            .is_err()
    );
    let operation = OperationId::generate().unwrap();
    let prepared = prepare(&f, &engine, operation, "admin");
    assert_eq!(
        prepared
            .confirm_save(&"f".repeat(64).parse().unwrap())
            .err()
            .unwrap()
            .code(),
        ErrorCode::PlanStale
    );
    assert!(!engine.config().disks[0].root.join(DIRECTORY).exists());
}

#[test]
fn current_actor_and_changed_namespace_are_rechecked_before_intent_creation() {
    let f = fixture();
    let mut engine = target(&f);
    let operation = OperationId::generate().unwrap();
    let prepared = prepare(&f, &engine, operation, "admin");
    let digest = *prepared.digest();
    assert_eq!(
        engine
            .save_foreign_import_intent(prepared.confirm_save(&digest).unwrap(), "other")
            .unwrap_err()
            .code(),
        ErrorCode::PermissionDenied
    );
    let prepared = prepare(&f, &engine, operation, "admin");
    let digest = *prepared.digest();
    engine
        .create_repository(
            &RepositoryName::parse("unrelated").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    assert_eq!(
        engine
            .save_foreign_import_intent(prepared.confirm_save(&digest).unwrap(), "admin")
            .unwrap_err()
            .code(),
        ErrorCode::PlanStale
    );
    assert!(!path(&engine, operation).exists());
}

#[test]
fn interrupted_operation_directory_is_retained_and_never_adopted_as_absence() {
    let f = fixture();
    let mut engine = target(&f);
    let operation = OperationId::generate().unwrap();
    let prepared = prepare(&f, &engine, operation, "admin");
    let target = prepared.target_identity().clone();
    let digest = *prepared.digest();
    let root = &engine
        .disk_resources
        .get(&engine.config().disks[0])
        .unwrap()
        .root;
    root.private_dir(&path(&engine, operation)).unwrap();
    assert_eq!(
        engine
            .foreign_import_intent(&target, operation, "admin")
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        engine
            .save_foreign_import_intent(prepared.confirm_save(&digest).unwrap(), "admin")
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    assert!(path(&engine, operation).is_dir());
    assert!(!path(&engine, operation).join("intent.json").exists());
}

#[test]
fn operation_reuse_never_overwrites_existing_intent_or_published_business_receipts() {
    let f = fixture();
    let mut engine = target(&f);
    let operation = OperationId::generate().unwrap();
    let prepared = prepare(&f, &engine, operation, "admin");
    save(&mut engine, prepared);
    let bytes = fs::read(path(&engine, operation).join("intent.json")).unwrap();
    let changed = prepare(&f, &engine, operation, "other");
    let digest = *changed.digest();
    assert_eq!(
        engine
            .save_foreign_import_intent(changed.confirm_save(&digest).unwrap(), "other")
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(
        fs::read(path(&engine, operation).join("intent.json")).unwrap(),
        bytes
    );
    let published = OperationId::generate().unwrap();
    engine
        .create_repository(
            &RepositoryName::parse("existing").unwrap(),
            "P01",
            1,
            "admin",
            published,
        )
        .unwrap();
    let source = source(&f);
    let target = ForeignImportIntentTarget::new("P01".into(), None, names(&source)).unwrap();
    assert_eq!(
        engine
            .prepare_foreign_import_intent(source, target, request(published, "admin"))
            .err()
            .unwrap()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );
    assert!(!path(&engine, published).exists());
}

#[test]
fn recomputed_hash_cannot_hide_an_incomplete_mapping_and_required_nulls_stay_required() {
    let f = fixture();
    let mut engine = target(&f);
    let operation = OperationId::generate().unwrap();
    let prepared = prepare(&f, &engine, operation, "admin");
    let target = prepared.target_identity().clone();
    save(&mut engine, prepared);
    let file = path(&engine, operation).join("intent.json");
    let original: json_codec::Value = decode_strict(&fs::read(&file).unwrap()).unwrap();
    for change in ["mapping", "parent", "authority_hash"] {
        let mut value = original.clone();
        match change {
            "mapping" => {
                let mapping = value["record"]["mapping"].as_object_mut().unwrap();
                let key = mapping.keys().next().unwrap().clone();
                mapping.remove(&key);
            }
            "parent" => {
                value["record"]["target"]
                    .as_object_mut()
                    .unwrap()
                    .remove("parent");
            }
            "authority_hash" => {
                value["record"]["source"]
                    .as_object_mut()
                    .unwrap()
                    .remove("authority_hash");
            }
            _ => unreachable!(),
        }
        value["digest"] = object_hash("foreign-import-intent", &value["record"])
            .unwrap()
            .into();
        fs::write(&file, canonical_json(&value).unwrap()).unwrap();
        assert!(
            engine
                .foreign_import_intent(&target, operation, "admin")
                .is_err(),
            "accepted {change}"
        );
        assert_eq!(
            decode_strict::<json_codec::Value>(&fs::read(&file).unwrap()).unwrap(),
            value
        );
    }
}

#[test]
fn incomplete_retained_operations_charge_the_aggregate_budget() {
    let f = fixture();
    let mut engine = target(&f);
    let operation = OperationId::generate().unwrap();
    let prepared = prepare(&f, &engine, operation, "admin");
    let digest = *prepared.digest();
    let root = &engine
        .disk_resources
        .get(&engine.config().disks[0])
        .unwrap()
        .root;
    for _ in 0..MAX_RETAINED_INTENTS {
        root.private_dir(&path(&engine, OperationId::generate().unwrap()))
            .unwrap();
    }
    assert_eq!(
        engine
            .save_foreign_import_intent(prepared.confirm_save(&digest).unwrap(), "admin")
            .unwrap_err()
            .code(),
        ErrorCode::ResourceLimit
    );
    assert!(!path(&engine, operation).exists());
    assert_eq!(
        fs::read_dir(engine.config().disks[0].root.join(DIRECTORY))
            .unwrap()
            .count(),
        MAX_RETAINED_INTENTS
    );
}

#[test]
fn partial_bytes_and_real_metadata_reserve_are_checked_without_deleting_residue() {
    let f = fixture();
    let engine = target(&f);
    let operation = OperationId::generate().unwrap();
    let prepared = prepare(&f, &engine, operation, "admin");
    let root = &engine
        .disk_resources
        .get(&engine.config().disks[0])
        .unwrap()
        .root;
    assert_eq!(
        persist::save(root, &prepared.record, root.available().unwrap() + 1)
            .unwrap_err()
            .code(),
        ErrorCode::NoSpace
    );
    assert!(!path(&engine, operation).exists());
    let residue = path(&engine, OperationId::generate().unwrap());
    root.private_dir(&residue).unwrap();
    let file = residue.join("incomplete.tmp");
    root.write_atomic(&file, b"retained", false).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&file)
        .unwrap()
        .set_len(MAX_RETAINED_BYTES + 1)
        .unwrap();
    assert_eq!(
        persist::save(root, &prepared.record, 0).unwrap_err().code(),
        ErrorCode::ResourceLimit
    );
    assert_eq!(fs::metadata(file).unwrap().len(), MAX_RETAINED_BYTES + 1);
    assert!(!path(&engine, operation).exists());
}

#[test]
fn a_replaced_verified_intent_file_or_directory_cannot_be_synchronized_as_the_original() {
    let f = fixture();
    let mut engine = target(&f);
    let operation = OperationId::generate().unwrap();
    let prepared = prepare(&f, &engine, operation, "admin");
    save(&mut engine, prepared);
    let root = &engine
        .disk_resources
        .get(engine.disk("P01").unwrap())
        .unwrap()
        .root;
    let held = persist::read(root, operation).unwrap().unwrap();
    let directory = path(&engine, operation);
    let file = directory.join("intent.json");
    let bytes = fs::read(&file).unwrap();
    fs::rename(&file, directory.join("retained-original.json")).unwrap();
    fs::write(&file, &bytes).unwrap();
    assert_eq!(
        held.synchronize(root).unwrap_err().code(),
        ErrorCode::PlanStale
    );
    assert_eq!(fs::read(&file).unwrap(), bytes);
    let held = persist::read(root, operation).unwrap().unwrap();
    let detached = directory.with_extension("retained");
    fs::rename(&directory, &detached).unwrap();
    fs::create_dir(&directory).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(&file, &bytes).unwrap();
    assert_eq!(
        held.synchronize(root).unwrap_err().code(),
        ErrorCode::PlanStale
    );
    assert_eq!(fs::read(&file).unwrap(), bytes);
    assert_eq!(fs::read(detached.join("intent.json")).unwrap(), bytes);
}

#[test]
fn intent_ids_are_target_local_and_do_not_reserve_future_business_operation_ids() {
    let f = fixture();
    let mut config = Config::development([
        f._temp.path().join("target-one"),
        f._temp.path().join("target-two"),
    ])
    .unwrap();
    config.disks[1].level = 1;
    config.disks[1].upstream_slot_id = None;
    let mut engine = Engine::initialize(&f._temp.path().join("target-system"), config).unwrap();
    let operation = OperationId::generate().unwrap();
    let first = prepare(&f, &engine, operation, "admin");
    let first = save(&mut engine, first);
    let input = source(&f);
    let mapping = names(&input);
    let second = engine
        .prepare_foreign_import_intent(
            input,
            ForeignImportIntentTarget::new("B01".into(), None, mapping).unwrap(),
            request(operation, "admin"),
        )
        .unwrap();
    let second = save(&mut engine, second);
    assert_ne!(first.target(), second.target());
    assert_ne!(first.digest(), second.digest());
    assert_eq!(first.operation_id(), second.operation_id());
    assert_eq!(
        engine
            .foreign_import_intent(first.target(), operation, "admin")
            .unwrap(),
        Some(first.clone())
    );
    assert_eq!(
        engine
            .foreign_import_intent(second.target(), operation, "admin")
            .unwrap(),
        Some(second)
    );
    engine
        .create_repository(
            &RepositoryName::parse("later-business").unwrap(),
            "P01",
            1,
            "admin",
            operation,
        )
        .unwrap();
    // A local intent is still observable, but cannot claim this later business
    // receipt or be reconciled as if it had reserved the global operation ID.
    assert_eq!(
        engine
            .foreign_import_intent(first.target(), operation, "admin")
            .unwrap(),
        Some(first.clone())
    );
    assert_eq!(
        engine
            .reconcile_foreign_import_intent(first.target(), operation, "admin", first.digest())
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );
}

#[test]
fn a_history_proof_lost_after_preview_cannot_be_captured_as_a_complete_intent() {
    let f = fixture();
    let engine = target(&f);
    let preview = inspect(&f.source).unwrap();
    let disk = f
        .engine
        .config()
        .disks
        .iter()
        .find(|disk| disk.root == f.source)
        .unwrap();
    let missing = crate::journal::object_path(
        disk,
        "history-manifest",
        preview.histories()[0].manifest_hash(),
    );
    fs::remove_file(&missing).unwrap();
    assert!(ForeignImportIntentSource::capture(preview).is_err());
    assert!(!missing.exists());
    assert!(!engine.config().disks[0].root.join(DIRECTORY).exists());
}

#[test]
fn observed_intent_cannot_synchronize_a_valid_in_place_replacement() {
    use std::os::unix::fs::MetadataExt;
    let f = fixture();
    let mut engine = target(&f);
    let prepared = prepare(&f, &engine, OperationId::generate().unwrap(), "admin");
    let saved = save(&mut engine, prepared);
    let owner = engine
        .disk_resources
        .get(engine.disk("P01").unwrap())
        .unwrap();
    let observed = persist::read(&owner.root, saved.operation_id())
        .unwrap()
        .unwrap();
    let path = owner
        .root
        .path()
        .join(DIRECTORY)
        .join(saved.operation_id().to_string())
        .join("intent.json");
    let identity = fs::metadata(&path).unwrap().ino();
    let mut wire: json_codec::Value = decode_strict(&fs::read(&path).unwrap()).unwrap();
    let mut record = observed.record.clone();
    record.principal = "other".into();
    record.validate().unwrap();
    wire["record"] = decode_strict(&canonical_json(&record).unwrap()).unwrap();
    wire["digest"] = record.digest().unwrap().to_string().into();
    let replacement = canonical_json(&wire).unwrap();
    fs::write(&path, &replacement).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().ino(), identity);
    assert_eq!(
        observed.synchronize(&owner.root).unwrap_err().code(),
        ErrorCode::PlanStale
    );
    assert_eq!(fs::read(&path).unwrap(), replacement);
}
