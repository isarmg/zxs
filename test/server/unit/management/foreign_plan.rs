use super::super::foreign_disk_fixture::{Fixture, bytes, fixture, inspect, source};
use super::*;
use storage_contract::CommitRequest;

fn request(f: &Fixture) -> ForeignPlanRequest {
    let preview = inspect(f, source(f)).unwrap();
    ForeignPlanRequest {
        operation_id: OperationId::generate().unwrap(),
        source: preview.source,
        source_review_digest: preview.review_digest,
        target: ForeignPlanTarget {
            slot_id: "P01".into(),
            parent: None,
            history_names: preview
                .histories
                .iter()
                .enumerate()
                .map(|(index, history)| (history.group_id.clone(), format!("origin-{}", index + 1)))
                .collect(),
        },
    }
}
fn call(
    f: &Fixture,
    action: &'static str,
    input: impl data_serialization::Serialize,
) -> Result<Value, ApiError> {
    f.backend.setup_operation(
        action,
        json_codec::to_value(input).unwrap(),
        None,
        Some(&f.identity),
    )
}
fn preview(f: &Fixture) -> ForeignPlanPreview {
    json_codec::from_value(call(f, "foreign_plan_preview", request(f)).unwrap()).unwrap()
}
fn save(
    f: &Fixture,
    preview: &ForeignPlanPreview,
    acknowledge_plan_only: bool,
) -> Result<ForeignPlanStatus, ApiError> {
    call(
        f,
        "foreign_plan_save",
        ForeignPlanSave {
            preview: preview.clone(),
            acknowledge_plan_only,
        },
    )
    .map(|value| json_codec::from_value(value).unwrap())
}
fn selection(preview: &ForeignPlanPreview) -> ForeignPlanSelection {
    ForeignPlanSelection {
        operation_id: preview.request.operation_id,
        review_digest: preview.review_digest.clone(),
    }
}
fn sequence(f: &Fixture) -> u64 {
    f.backend
        .inner
        .backend()
        .unwrap()
        .test_with_engine_locked(|engine| engine.state().sequence)
}
fn reopen(f: &Fixture) {
    crate::lock(&f.backend.inner.state).backend = BackendState::Unopened;
    let engine = Arc::new(EngineBackend::new(
        storage_engine::Engine::open(&f.backend.inner.data_root).unwrap(),
    ));
    let auth = crate::lock(&f.backend.inner.auth).clone().unwrap();
    engine
        .set_management_auth(crate::lock(&auth).reader())
        .unwrap();
    f.backend.inner.set(Some(engine), StorageState::Ready);
}
fn create_root(f: &Fixture, content: bool) {
    f.backend
        .inner
        .backend()
        .unwrap()
        .test_with_engine_locked(|engine| {
            let root = engine
                .create_disk_root(
                    "P01",
                    &f.identity.principal,
                    OperationId::generate().unwrap(),
                )
                .unwrap();
            if content {
                let workspace = engine
                    .create_workspace(&root.main_branch_id, &f.identity.principal)
                    .unwrap();
                engine
                    .stage_bytes(&workspace.workspace_id, "occupied.txt", b"existing data")
                    .unwrap();
                engine
                    .commit_workspace(CommitRequest {
                        workspace_id: workspace.workspace_id,
                        operation_id: OperationId::generate().unwrap(),
                        principal: f.identity.principal.clone(),
                        expected_head: workspace.base_head,
                        fencing_token: workspace.fencing_token,
                        message: String::new(),
                        request_digest: None,
                    })
                    .unwrap();
            }
        });
}

#[test]
fn foreign_plan_is_immutable_review_only_and_survives_cold_reopen() {
    let f = fixture();
    let original_source = bytes(&f.source);
    let original_target = bytes(&f.known);
    let before = sequence(&f);
    let p = preview(&f);
    assert_eq!(p.execution, ForeignExecutionCapability::Unavailable);
    assert_eq!(p.target.histories.len(), p.source.histories.len());
    assert_eq!(p.target.histories.len(), 2);
    assert!(
        p.source
            .histories
            .iter()
            .any(|history| !history.temporarily_unavailable_versions.is_empty())
    );
    assert!(!f.backend.inner.management_root.join(DIRECTORY).exists());
    assert!(save(&f, &p, false).is_err());
    assert!(!f.backend.inner.management_root.join(DIRECTORY).exists());
    let saved = save(&f, &p, true).unwrap();
    assert_eq!(saved.validation, ForeignPlanValidation::Required);
    assert_eq!(saved.preview, p);
    assert_eq!(save(&f, &p, true).unwrap(), saved);
    assert_eq!(bytes(&f.source), original_source);
    assert_eq!(bytes(&f.known), original_target);
    assert_eq!(sequence(&f), before);
    reopen(&f);
    let query: ForeignPlanStatus = json_codec::from_value(
        call(
            &f,
            "foreign_plan_status",
            json!({"operation_id":p.request.operation_id}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(query, saved);
    let checked: ForeignPlanStatus =
        json_codec::from_value(call(&f, "foreign_plan_revalidate", selection(&p)).unwrap())
            .unwrap();
    assert_eq!(
        checked.validation,
        ForeignPlanValidation::MatchedAtObservation
    );
    assert_eq!(
        checked.preview.execution,
        ForeignExecutionCapability::Unavailable
    );
    assert_eq!(
        call(&f, "foreign_plan_execute", selection(&p))
            .unwrap_err()
            .code,
        ErrorCode::FormatUnsupported
    );
    assert_eq!(bytes(&f.source), original_source);
    assert_eq!(sequence(&f), before);
    let queried: ForeignPlanStatus = json_codec::from_value(
        call(
            &f,
            "foreign_plan_status",
            json!({"operation_id":p.request.operation_id}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(queried.validation, ForeignPlanValidation::Required);
}

#[test]
fn foreign_plan_rechecks_empty_target_and_never_replaces_an_existing_review() {
    let f = fixture();
    let p = preview(&f);
    create_root(&f, false);
    assert_eq!(save(&f, &p, true).unwrap_err().code, ErrorCode::PlanStale);
    assert!(!f.backend.inner.management_root.join(DIRECTORY).exists());
    let p = preview(&f);
    let saved = save(&f, &p, true).unwrap();
    let directory = open(&f.backend.inner.management_root).unwrap().unwrap();
    let name = storage_addition::name(p.request.operation_id);
    let original = directory.read_bounded(&name, MAX_RECORD_BYTES).unwrap();
    create_root(&f, true);
    assert_eq!(
        call(&f, "foreign_plan_revalidate", selection(&p))
            .unwrap_err()
            .code,
        ErrorCode::DirectoryNotEmpty
    );
    // Replaying a saved audit record makes no claim that the target remains empty.
    assert_eq!(save(&f, &p, true).unwrap(), saved);
    let mut changed = p.clone();
    *changed
        .request
        .target
        .history_names
        .values_mut()
        .next()
        .unwrap() = "different".into();
    changed.review_digest = digest(&changed);
    assert_eq!(
        save(&f, &changed, true).unwrap_err().code,
        ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(
        directory.read_bounded(&name, MAX_RECORD_BYTES).unwrap(),
        original
    );
}

#[test]
fn foreign_plan_requires_complete_noncolliding_mapping_and_exact_source() {
    let f = fixture();
    let mut r = request(&f);
    for name in r.target.history_names.values_mut() {
        *name = "same".into();
    }
    assert_eq!(
        call(&f, "foreign_plan_preview", &r).unwrap_err().code,
        ErrorCode::NameCollision
    );
    r.target.history_names.pop_first();
    assert!(call(&f, "foreign_plan_preview", &r).is_err());
    let mut r = request(&f);
    r.source.identity.inode += 1;
    assert!(call(&f, "foreign_plan_preview", &r).is_err());
    let mut r = request(&f);
    r.source_review_digest = "0".repeat(64);
    assert_eq!(
        call(&f, "foreign_plan_preview", &r).unwrap_err().code,
        ErrorCode::PlanStale
    );
    let mut r = request(&f);
    r.target.parent = Some("missing".into());
    assert_eq!(
        call(&f, "foreign_plan_preview", &r).unwrap_err().code,
        ErrorCode::ParentNotDirectory
    );
    let mut r = json_codec::to_value(request(&f)).unwrap();
    r["target"]["level"] = json!(2);
    assert!(call(&f, "foreign_plan_preview", r).is_err());
    assert!(!f.backend.inner.management_root.join(DIRECTORY).exists());
}

#[test]
fn foreign_plan_unknown_corrupt_and_revoked_records_never_gain_execution() {
    let f = fixture();
    let unknown = call(
        &f,
        "foreign_plan_status",
        json!({"operation_id": OperationId::generate().unwrap()}),
    )
    .unwrap_err();
    assert_eq!(unknown.code, ErrorCode::OperationNotFound);
    assert!(!f.backend.inner.management_root.join(DIRECTORY).exists());
    let p = preview(&f);
    save(&f, &p, true).unwrap();
    let directory = open(&f.backend.inner.management_root).unwrap().unwrap();
    let name = storage_addition::name(p.request.operation_id);
    let original = directory.read_bounded(&name, MAX_RECORD_BYTES).unwrap();
    AtomicFile::replace(&directory, &name.as_relative(), b"{").unwrap();
    assert!(save(&f, &p, true).is_err());
    assert!(
        call(
            &f,
            "foreign_plan_status",
            json!({"operation_id":p.request.operation_id})
        )
        .is_err()
    );
    assert_eq!(
        directory.read_bounded(&name, MAX_RECORD_BYTES).unwrap(),
        b"{"
    );
    AtomicFile::replace(&directory, &name.as_relative(), &original).unwrap();
    let mut wrong = selection(&p);
    wrong.review_digest = "0".repeat(64);
    assert_eq!(
        call(&f, "foreign_plan_execute", wrong).unwrap_err().code,
        ErrorCode::PlanStale
    );
    let auth = crate::lock(&f.backend.inner.auth).clone().unwrap();
    crate::lock(&auth).logout(&f.token).unwrap();
    assert_eq!(
        call(&f, "foreign_plan_execute", selection(&p))
            .unwrap_err()
            .status,
        http::StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        directory.read_bounded(&name, MAX_RECORD_BYTES).unwrap(),
        original
    );
}

#[test]
fn foreign_plan_targets_only_the_chosen_empty_parent_and_keeps_origins_separate() {
    let f = fixture();
    f.backend
        .inner
        .backend()
        .unwrap()
        .test_with_engine_locked(|engine| {
            let root = engine
                .create_disk_root(
                    "P01",
                    &f.identity.principal,
                    OperationId::generate().unwrap(),
                )
                .unwrap();
            let workspace = engine
                .create_workspace(&root.main_branch_id, &f.identity.principal)
                .unwrap();
            engine
                .namespace_operation(
                    &workspace.workspace_id,
                    storage_engine::NamespaceOperation::Mkdir {
                        path: name_policy::PortablePath::parse("Restored").unwrap(),
                    },
                )
                .unwrap();
            engine
                .stage_bytes(
                    &workspace.workspace_id,
                    "untouched.txt",
                    b"unrelated local content",
                )
                .unwrap();
            engine
                .commit_workspace(CommitRequest {
                    workspace_id: workspace.workspace_id,
                    operation_id: OperationId::generate().unwrap(),
                    principal: f.identity.principal.clone(),
                    expected_head: workspace.base_head,
                    fencing_token: workspace.fencing_token,
                    message: String::new(),
                    request_digest: None,
                })
                .unwrap();
        });
    let mut r = request(&f);
    assert_eq!(
        call(&f, "foreign_plan_preview", &r).unwrap_err().code,
        ErrorCode::DirectoryNotEmpty
    );
    r.target.parent = Some("Restored".into());
    let p: ForeignPlanPreview =
        json_codec::from_value(call(&f, "foreign_plan_preview", r).unwrap()).unwrap();
    assert!(
        p.target
            .histories
            .iter()
            .all(|history| history.destination.starts_with("Restored/origin-"))
    );
    let original = bytes(&f.known);
    save(&f, &p, true).unwrap();
    assert_eq!(bytes(&f.known), original);
    assert_ne!(
        p.source.histories[0].origin.disk_id,
        p.source.histories[1].origin.disk_id
    );
}

#[test]
fn foreign_plan_source_change_and_other_owner_fail_closed_without_overwrite() {
    let f = fixture();
    let p = preview(&f);
    save(&f, &p, true).unwrap();
    let directory = open(&f.backend.inner.management_root).unwrap().unwrap();
    let name = storage_addition::name(p.request.operation_id);
    let original = directory.read_bounded(&name, MAX_RECORD_BYTES).unwrap();
    // No source path is read while returning an already saved audit record.
    fs::rename(&f.source, f.source.with_file_name("detached-foreign")).unwrap();
    assert_eq!(
        save(&f, &p, true).unwrap().validation,
        ForeignPlanValidation::Required
    );
    assert!(call(&f, "foreign_plan_revalidate", selection(&p)).is_err());
    assert_eq!(
        directory.read_bounded(&name, MAX_RECORD_BYTES).unwrap(),
        original
    );
    let mut other_owner: Value = storage_contract::strict_json(&original).unwrap();
    other_owner["principal"] = json!("different-owner");
    let other_bytes = json_codec::to_vec(&other_owner).unwrap();
    AtomicFile::replace(&directory, &name.as_relative(), &other_bytes).unwrap();
    assert_eq!(
        call(
            &f,
            "foreign_plan_status",
            json!({"operation_id":p.request.operation_id})
        )
        .unwrap_err()
        .status,
        http::StatusCode::FORBIDDEN
    );
    assert_eq!(
        save(&f, &p, true).unwrap_err().status,
        http::StatusCode::FORBIDDEN
    );
    assert_eq!(
        directory.read_bounded(&name, MAX_RECORD_BYTES).unwrap(),
        other_bytes
    );
}
