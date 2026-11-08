//! Storage addition admission, durable receipts, cancellation and replay regressions.
use super::*;
use storage_contract::storage_directories::{CandidateSelection, InspectRequest};

struct Fixture {
    backend: Arc<ManagementBackend>,
    identity: Identity,
    token: String,
    auth: Arc<Mutex<AuthStore>>,
    discovery: PathBuf,
    data: PathBuf,
    _root: tempfile::TempDir,
}
fn fixture() -> Fixture {
    let root = crate::tests::private_tempdir();
    let data = root.path().join("system");
    let discovery = root.path().join("directories");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&discovery)
        .unwrap();
    let engine = storage_engine::Engine::initialize(
        &data,
        Config::development([discovery.join("original")]).unwrap(),
    )
    .unwrap();
    // Exercise the product's actual default, not an external archive workaround.
    let management = data.join("management");
    drop(AuthStore::initialize_management(&management, "admin", "addition-test-password").unwrap());
    let (backend, mut auth) = open_service_with_options(
        &data,
        &management,
        ManagementOptions {
            discovery_root: discovery.to_str().unwrap().parse().unwrap(),
            ..Default::default()
        },
    )
    .unwrap();
    let login = auth.login("admin", "addition-test-password").unwrap();
    let identity = auth.authenticate(&login.token).unwrap();
    let engine = Arc::new(EngineBackend::new(engine));
    engine.set_management_auth(auth.reader()).unwrap();
    let auth = Arc::new(Mutex::new(auth));
    *crate::lock(&backend.inner.auth) = Some(auth.clone());
    backend.inner.set(Some(engine), StorageState::Ready);
    Fixture {
        backend,
        identity,
        token: login.token,
        auth,
        discovery,
        data,
        _root: root,
    }
}
fn selection(f: &Fixture, name: &str) -> CandidateSelection {
    let path = f.discovery.join(name);
    fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
    let listing = crate::storage_directories::browse(path.to_str().unwrap()).unwrap();
    crate::storage_directories::inspect_within(
        f.discovery.to_str().unwrap(),
        &InspectRequest {
            mount_path: listing.mount_path,
            root_path: path.to_str().unwrap().into(),
        },
        std::slice::from_ref(&f.data),
    )
    .unwrap()
}
fn command(f: &Fixture, action: &'static str, value: Value) -> Result<Value, ApiError> {
    f.backend.storage_addition(action, value, &f.identity)
}
fn request(selection: CandidateSelection, slot: &str, metadata: bool) -> AdditionRequest {
    AdditionRequest {
        operation_id: OperationId::generate().unwrap(),
        additions: vec![DirectoryAddition {
            selection,
            slot_id: slot.into(),
            level: 1,
            upstream_slot_id: None,
            metadata_member: metadata,
        }],
    }
}
fn preview(f: &Fixture, request: AdditionRequest) -> AdditionPreview {
    json_codec::from_value(command(f, "disk_add_preview", json!(request)).unwrap()).unwrap()
}

#[test]
fn typed_addition_uses_the_same_durable_receipt_as_wire_and_exact_resume() {
    let f = fixture();
    let request = request(selection(&f, "typed-new-root"), "N01", true);
    let preview = f
        .backend
        .preview_storage_addition(&f.identity, request)
        .unwrap();
    let commit = AdditionCommit { preview };
    let result = f
        .backend
        .commit_storage_addition(&f.identity, commit.clone())
        .unwrap();
    assert_eq!(result.state, AdditionState::Completed);
    assert_eq!(
        f.backend
            .resume_storage_addition(
                &f.identity,
                AdditionResume {
                    operation_id: result.operation_id,
                    continuation: AdditionContinuation::Add,
                },
            )
            .unwrap(),
        result
    );
    assert_eq!(
        command(&f, "disk_add_commit", json!(commit)).unwrap(),
        json!(result)
    );
    assert_eq!(
        f.backend
            .inner
            .addition_backend()
            .unwrap()
            .addition_configuration(&f.identity)
            .unwrap()
            .disks
            .len(),
        2
    );
}

#[test]
fn typed_addition_keeps_host_scope_gate_and_current_session_checks() {
    let f = fixture();
    let selected = selection(&f, "typed-selected");
    let request = request(selected, "N01", true);
    let gate = crate::lock(&f.backend.inner.storage_addition);
    assert_eq!(
        f.backend
            .preview_storage_addition(&f.identity, request.clone())
            .unwrap_err()
            .code,
        ErrorCode::StorageRetryRunning
    );
    drop(gate);
    let mut outside = request.clone();
    outside.additions[0].selection.root_path = f._root.path().join("outside").display().to_string();
    assert!(
        f.backend
            .preview_storage_addition(&f.identity, outside)
            .is_err()
    );
    assert!(!f._root.path().join("outside").exists());
    let preview = f
        .backend
        .preview_storage_addition(&f.identity, request)
        .unwrap();
    crate::lock(&f.auth).logout(&f.token).unwrap();
    assert!(
        f.backend
            .commit_storage_addition(&f.identity, AdditionCommit { preview })
            .is_err()
    );
    assert!(
        fs::read_dir(f.discovery.join("typed-selected"))
            .unwrap()
            .next()
            .is_none()
    );
}

#[test]
fn addition_rejects_unbrowseable_disk_names_without_persisting_a_plan() {
    for (name, expected) in [
        ("ORIGINAL", ErrorCode::NameConflict),
        ("CON", ErrorCode::InvalidRequest),
        ("cafe\u{301}", ErrorCode::InvalidRequest),
    ] {
        let f = fixture();
        let configuration = fs::read(f.data.join("config.json")).unwrap();
        let seal = fs::read(f.discovery.join("original/disk.json")).unwrap();
        let request = request(selection(&f, name), "N01", true);
        let operation = request.operation_id;
        assert_eq!(
            f.backend
                .preview_storage_addition(&f.identity, request)
                .unwrap_err()
                .code,
            expected,
            "{name}"
        );
        assert!(
            !f.data
                .join("management/storage-additions")
                .join(format!("{operation}.json"))
                .try_exists()
                .unwrap()
        );
        assert_eq!(fs::read(f.data.join("config.json")).unwrap(), configuration);
        assert_eq!(
            fs::read(f.discovery.join("original/disk.json")).unwrap(),
            seal
        );
        assert!(
            fs::read_dir(f.discovery.join(name))
                .unwrap()
                .next()
                .is_none()
        );
        assert_eq!(
            f.backend.topology_snapshot(&f.identity).unwrap().slots[0]
                .directory_name()
                .unwrap()
                .as_str(),
            "original"
        );
    }
}

#[test]
fn addition_rejects_colliding_new_primary_and_backup_roots() {
    let f = fixture();
    let first = selection(&f, "Straße");
    let second = selection(&f, "STRASSE");
    // Creating the second directory changes the parent observation; both
    // selections must be captured against the same unchanged parent.
    let first = crate::storage_directories::inspect_within(
        f.discovery.to_str().unwrap(),
        &InspectRequest {
            mount_path: first.mount_path,
            root_path: first.root_path,
        },
        std::slice::from_ref(&f.data),
    )
    .unwrap();
    let mut request = request(first, "N01", true);
    request.additions.push(DirectoryAddition {
        selection: second,
        slot_id: "N02".into(),
        level: 2,
        upstream_slot_id: Some("N01".into()),
        metadata_member: false,
    });
    assert_eq!(
        f.backend
            .preview_storage_addition(&f.identity, request)
            .unwrap_err()
            .code,
        ErrorCode::NameConflict
    );
    for name in ["Straße", "STRASSE"] {
        assert!(
            fs::read_dir(f.discovery.join(name))
                .unwrap()
                .next()
                .is_none()
        );
    }
}

#[test]
fn existing_pool_accepts_data_only_then_mixed_addition_and_exact_replay() {
    let f = fixture();
    let first = preview(&f, request(selection(&f, "data-only"), "N01", false));
    assert_eq!(
        first.original_metadata_members,
        first.target_metadata_members
    );
    let submitted = json!(AdditionCommit {
        preview: first.clone()
    });
    let done: AdditionStatus =
        json_codec::from_value(command(&f, "disk_add_commit", submitted.clone()).unwrap()).unwrap();
    assert_eq!(done.state, AdditionState::Completed);
    assert_eq!(
        command(&f, "disk_add_commit", submitted).unwrap(),
        json!(done)
    );
    let data_selection = selection(&f, "mixed-data");
    let metadata_selection = selection(&f, "mixed-metadata");
    // The discovery root ctime changed when the second directory was created.
    let data_selection = crate::storage_directories::inspect_within(
        f.discovery.to_str().unwrap(),
        &InspectRequest {
            mount_path: data_selection.mount_path,
            root_path: data_selection.root_path,
        },
        std::slice::from_ref(&f.data),
    )
    .unwrap();
    let mut mixed = request(data_selection, "N02", false);
    mixed.additions.push(DirectoryAddition {
        selection: metadata_selection,
        slot_id: "N03".into(),
        level: 1,
        upstream_slot_id: None,
        metadata_member: true,
    });
    let next = preview(&f, mixed);
    assert_eq!(
        next.target_metadata_members,
        next.original_metadata_members + 1
    );
    let done: AdditionStatus = json_codec::from_value(
        command(
            &f,
            "disk_add_commit",
            json!(AdditionCommit { preview: next }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(done.state, AdditionState::Completed);
    let config = f
        .backend
        .inner
        .addition_backend()
        .unwrap()
        .addition_configuration(&f.identity)
        .unwrap();
    assert_eq!(config.disks.len(), 4);
    assert!(
        !config
            .disks
            .iter()
            .find(|disk| disk.slot_id == "N01")
            .unwrap()
            .metadata_member
    );
    assert!(
        config
            .disks
            .iter()
            .find(|disk| disk.slot_id == "N03")
            .unwrap()
            .metadata_member
    );
}

#[test]
fn changed_preview_and_reused_operation_are_not_new_additions() {
    let f = fixture();
    let mut plan = preview(&f, request(selection(&f, "changed"), "N01", false));
    plan.target_metadata_members += 1;
    assert_eq!(
        command(
            &f,
            "disk_add_commit",
            json!(AdditionCommit {
                preview: plan.clone()
            })
        )
        .unwrap_err()
        .code,
        ErrorCode::GenerationMismatch
    );
    plan.target_metadata_members -= 1;
    command(
        &f,
        "disk_add_commit",
        json!(AdditionCommit {
            preview: plan.clone()
        }),
    )
    .unwrap();
    plan.request.additions[0].metadata_member = true;
    assert_eq!(
        command(
            &f,
            "disk_add_commit",
            json!(AdditionCommit { preview: plan })
        )
        .unwrap_err()
        .code,
        ErrorCode::IdempotencyKeyReused
    );
}

#[test]
fn durable_pending_intent_blocks_a_different_plan_and_survives_reopen() {
    let f = fixture();
    let selected = selection(&f, "pending");
    let request = request(selected, "N01", false);
    let backend = f.backend.inner.addition_backend().unwrap();
    let (preview, target, _) = f
        .backend
        .prepare_addition(
            Some(&backend),
            &f.identity,
            request,
            f.discovery.to_str().unwrap(),
        )
        .unwrap();
    let directory = PrivateDirectory::open_existing(&f.backend.inner.management_root)
        .unwrap()
        .create_child(&EntryName::new(DIRECTORY).unwrap())
        .unwrap();
    let record = StoredAddition {
        source: AdditionSource::ActiveMembership,
        format: Format::Addition,
        principal: f.identity.principal.clone(),
        target,
        archive: None,
        status: AdditionStatus {
            operation_id: preview.request.operation_id,
            preview,
            state: AdditionState::Adding,
            phase: AdditionPhase::Prepared,
            continuation: AdditionContinuation::Add,
            result: None,
            failure: None,
        },
    };
    save(&directory, &record).unwrap();
    write(&directory, &latest_name(), &record.status.operation_id).unwrap();
    assert_eq!(
        f.backend
            .require_settled_addition(&directory)
            .unwrap_err()
            .code,
        ErrorCode::RecoveryRequired
    );
    let observed = f
        .backend
        .addition_status(&f.identity, None)
        .unwrap()
        .operation
        .unwrap();
    assert_eq!(observed.state, AdditionState::RecoveryRequired);
    assert_eq!(observed.operation_id, record.status.operation_id);
    drop(backend);
    crate::lock(&f.backend.inner.state).backend = BackendState::Unopened;
    let reopened = storage_engine::Engine::open(&f.data).unwrap();
    let backend = Arc::new(EngineBackend::new(reopened));
    backend
        .set_management_auth(crate::lock(&f.auth).reader())
        .unwrap();
    f.backend.inner.set(Some(backend), StorageState::ReadOnly);
    let response = command(
        &f,
        "disk_add_resume",
        json!(AdditionResume {
            operation_id: record.status.operation_id,
            continuation: AdditionContinuation::Add,
        }),
    )
    .unwrap();
    assert_eq!(response["state"], "completed");
}

#[test]
fn current_session_is_rechecked_before_fencing() {
    let f = fixture();
    let plan = preview(&f, request(selection(&f, "revoked"), "N01", true));
    crate::lock(&f.auth).logout(&f.token).unwrap();
    assert!(
        command(
            &f,
            "disk_add_commit",
            json!(AdditionCommit { preview: plan })
        )
        .is_err()
    );
}

#[test]
fn active_addition_gate_rejects_another_plan_without_changing_storage_admission() {
    let f = fixture();
    let _gate = crate::lock(&f.backend.inner.storage_addition);
    let before = crate::lock(&f.backend.inner.state).admitted;
    assert_eq!(
        f.backend.retry_storage(&f.identity).unwrap_err().code,
        ErrorCode::StorageRetryRunning
    );
    assert_eq!(crate::lock(&f.backend.inner.state).admitted, before);
}

/// Stop through the real coordinator's persistence boundary, not a business hook.
fn pending_at(f: &Fixture, phase: AdditionPhase) -> StoredAddition {
    let request = request(selection(f, "interrupted-addition"), "N01", true);
    let backend = f.backend.inner.addition_backend().unwrap();
    let (preview, target, directories) = f
        .backend
        .prepare_addition(
            Some(&backend),
            &f.identity,
            request,
            f.discovery.to_str().unwrap(),
        )
        .unwrap();
    let directory = PrivateDirectory::open_existing(&f.backend.inner.management_root)
        .unwrap()
        .create_child(&EntryName::new(DIRECTORY).unwrap())
        .unwrap();
    let mut record = StoredAddition {
        source: AdditionSource::ActiveMembership,
        format: Format::Addition,
        principal: f.identity.principal.clone(),
        target,
        archive: None,
        status: AdditionStatus {
            operation_id: preview.request.operation_id,
            state: AdditionState::Adding,
            phase: AdditionPhase::Prepared,
            continuation: AdditionContinuation::Add,
            result: None,
            preview,
            failure: None,
        },
    };
    save(&directory, &record).unwrap();
    write(&directory, &latest_name(), &record.status.operation_id).unwrap();
    let intent = record.clone();
    let failure = backend
        .run_storage_addition(
            &f.identity,
            &intent,
            Some(directories),
            |observed, archive| {
                record.status.phase = observed;
                if let Some(archive) = archive {
                    record.archive = Some(archive);
                }
                save(&directory, &record)?;
                if observed == phase {
                    Err(ApiError::conflict(
                        ErrorCode::RecoveryRequired,
                        "test interrupts admitted coordinator",
                    ))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
    assert_eq!(failure.code, ErrorCode::RecoveryRequired);
    record.status.state = AdditionState::RecoveryRequired;
    save(&directory, &record).unwrap();
    record
}
fn cancel(f: &Fixture, record: &StoredAddition) -> Result<Value, ApiError> {
    command(
        f,
        "disk_add_cancel",
        json!(AdditionCancel {
            operation_id: record.status.operation_id,
            preview_digest: record.status.preview.preview_digest.clone(),
        }),
    )
}
fn resume(
    f: &Fixture,
    record: &StoredAddition,
    continuation: AdditionContinuation,
) -> Result<Value, ApiError> {
    command(
        f,
        "disk_add_resume",
        json!(AdditionResume {
            operation_id: record.status.operation_id,
            continuation,
        }),
    )
}
fn reopen(f: &Fixture) {
    crate::lock(&f.backend.inner.state).backend = BackendState::Unopened;
    let engine = storage_engine::Engine::open(&f.data).unwrap();
    let backend = Arc::new(EngineBackend::new(engine));
    backend
        .set_management_auth(crate::lock(&f.auth).reader())
        .unwrap();
    f.backend.inner.set(Some(backend), StorageState::ReadOnly);
}

#[test]
fn cancelled_addition_retains_original_receipt_and_does_not_reopen_target() {
    let f = fixture();
    let record = pending_at(&f, AdditionPhase::Enrolling);
    let target = PathBuf::from(
        &record.status.preview.request.additions[0]
            .selection
            .root_path,
    );
    let preserved = target.with_extension("preserved");
    fs::rename(&target, &preserved).unwrap();
    std::os::unix::fs::symlink(f.discovery.join("unavailable-target"), &target).unwrap();
    let cancelled: AdditionStatus = json_codec::from_value(cancel(&f, &record).unwrap()).unwrap();
    assert_eq!(cancelled.state, AdditionState::Cancelled);
    assert_eq!(cancelled.phase, AdditionPhase::Cancelled);
    let result = cancelled.result.as_ref().unwrap();
    assert_eq!(result.operation_id, record.status.operation_id);
    assert_eq!(result.kind, "membership-add-cancelled");
    assert_eq!(
        result.commit_state,
        storage_contract::CommitState::Published
    );
    assert_eq!(
        result.request_digest,
        record.archive.as_ref().unwrap().transition_hash
    );
    assert!(
        fs::symlink_metadata(&target)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(preserved.is_dir());
    assert_eq!(cancel(&f, &record).unwrap(), json!(cancelled));
    assert_eq!(
        resume(&f, &record, AdditionContinuation::Add).unwrap(),
        json!(cancelled)
    );
    assert_eq!(
        command(
            &f,
            "disk_add_commit",
            json!(AdditionCommit {
                preview: record.status.preview.clone()
            })
        )
        .unwrap(),
        json!(cancelled)
    );
    reopen(&f);
    assert_eq!(
        resume(&f, &record, AdditionContinuation::Cancel).unwrap(),
        json!(cancelled)
    );
    let config = f
        .backend
        .inner
        .addition_backend()
        .unwrap()
        .addition_configuration(&f.identity)
        .unwrap();
    assert_eq!(config.disks.len(), 1);
    let next = preview(&f, request(selection(&f, "next-addition"), "N02", false));
    assert_eq!(
        command(
            &f,
            "disk_add_commit",
            json!(AdditionCommit { preview: next })
        )
        .unwrap()["state"],
        "completed"
    );
}

#[test]
fn joint_quorum_rejects_cancel_without_reclassifying_add_or_silently_resuming_it() {
    let f = fixture();
    let record = pending_at(&f, AdditionPhase::Activating);
    assert_eq!(
        cancel(&f, &record).unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
    let status = f
        .backend
        .addition_status(&f.identity, Some(record.status.operation_id))
        .unwrap()
        .operation
        .unwrap();
    assert_eq!(status.state, AdditionState::RecoveryRequired);
    assert_eq!(status.continuation, AdditionContinuation::Cancel);
    assert!(status.result.is_none());
    // Replaying the original commit body must preserve the cancellation choice.
    assert!(
        command(
            &f,
            "disk_add_commit",
            json!(AdditionCommit {
                preview: record.status.preview.clone()
            })
        )
        .is_err()
    );
    let completed = resume(&f, &record, AdditionContinuation::Add).unwrap();
    assert_eq!(completed["state"], "completed");
    assert_eq!(completed["result"]["kind"], "membership-add");
    assert_eq!(cancel(&f, &record).unwrap(), completed);
}

#[test]
fn published_cancellation_wins_after_control_projection_loss_and_engine_reopen() {
    let f = fixture();
    let mut record = pending_at(&f, AdditionPhase::Enrolling);
    let directory =
        PrivateDirectory::open_existing(f.backend.inner.management_root.join(DIRECTORY)).unwrap();
    record.status.continuation = AdditionContinuation::Cancel;
    record.status.state = AdditionState::Cancelling;
    save(&directory, &record).unwrap();
    let intent = record.clone();
    let result = f
        .backend
        .inner
        .addition_backend()
        .unwrap()
        .run_storage_addition(&f.identity, &intent, None, |phase, archive| {
            record.status.phase = phase;
            if let Some(archive) = archive {
                record.archive = Some(archive);
            }
            save(&directory, &record)
        })
        .unwrap();
    assert_eq!(result.kind, "membership-add-cancelled");
    // Deliberately omit the terminal management projection, as after response loss.
    assert!(
        read_record(&directory, record.status.operation_id)
            .unwrap()
            .unwrap()
            .status
            .result
            .is_none()
    );
    reopen(&f);
    let status = f
        .backend
        .addition_status(&f.identity, Some(record.status.operation_id))
        .unwrap()
        .operation
        .unwrap();
    assert_eq!(status.state, AdditionState::Cancelled);
    assert_eq!(status.result, Some(result));
    assert_eq!(
        resume(&f, &record, AdditionContinuation::Cancel).unwrap(),
        json!(status)
    );
}

#[test]
fn cancellation_binds_original_preview_and_current_session_without_fake_pre_fence_success() {
    let f = fixture();
    let record = pending_at(&f, AdditionPhase::Fencing);
    let mismatch = command(
        &f,
        "disk_add_cancel",
        json!(AdditionCancel {
            operation_id: record.status.operation_id,
            preview_digest: "0".repeat(64),
        }),
    )
    .unwrap_err();
    assert_eq!(mismatch.code, ErrorCode::IdempotencyKeyReused);
    assert!(cancel(&f, &record).is_err());
    let status = f
        .backend
        .addition_status(&f.identity, Some(record.status.operation_id))
        .unwrap()
        .operation
        .unwrap();
    assert!(!status.state.is_terminal());
    assert_eq!(status.continuation, AdditionContinuation::Cancel);
    crate::lock(&f.auth).logout(&f.token).unwrap();
    assert_eq!(
        cancel(&f, &record).unwrap_err().code,
        ErrorCode::Unauthorized
    );
}

#[test]
fn native_current_administrator_uses_shared_addition_without_fabricating_a_session() {
    let f = fixture();
    let request = request(selection(&f, "native-new-root"), "N01", true);
    let identity = crate::lock(&f.auth).sole_identity().unwrap();
    assert!(identity.admission_session.is_none());
    // HTTP still rejects the same non-session native identity.
    assert!(
        f.backend
            .preview_storage_addition(&identity, request.clone())
            .is_err()
    );
    let backend = crate::lock(&f.backend.inner.state)
        .backend
        .engine()
        .unwrap()
        .clone();
    let parent = PrivateDirectory::open_existing(&f.backend.inner.management_root).unwrap();
    let control =
        ActiveAddition::open(&backend, &identity, parent, f.discovery.to_str().unwrap()).unwrap();
    let preview = control.preview(request).unwrap();
    let operation = preview.request.operation_id;
    let result = control.commit(AdditionCommit { preview }).unwrap();
    assert_eq!(result.state, AdditionState::Completed);
    assert_eq!(control.status(operation).unwrap(), result);
    assert_eq!(control.resume(operation).unwrap(), result);
    drop(control);
    assert_eq!(
        f.backend
            .resume_storage_addition(
                &f.identity,
                AdditionResume {
                    operation_id: operation,
                    continuation: AdditionContinuation::Add
                }
            )
            .unwrap(),
        result
    );
}
#[test]
fn native_admission_does_not_relax_http_session_revocation() {
    let f = fixture();
    let identity = crate::lock(&f.auth).sole_identity().unwrap();
    let backend = crate::lock(&f.backend.inner.state)
        .backend
        .engine()
        .unwrap()
        .clone();
    // Revocation is checked against the primary AuthStore, not a cached
    // disaster-recovery snapshot or a fabricated management session.
    crate::lock(&f.auth).logout(&f.token).unwrap();
    let current = crate::lock(&f.auth).sole_identity().unwrap();
    let parent = PrivateDirectory::open_existing(&f.backend.inner.management_root).unwrap();
    let control =
        ActiveAddition::open(&backend, &current, parent, f.discovery.to_str().unwrap()).unwrap();
    assert!(current.admission_session.is_none());
    assert_eq!(current.principal, identity.principal);
    drop(control);
    assert!(
        f.backend
            .preview_storage_addition(
                &f.identity,
                request(selection(&f, "revoked-session-root"), "N02", true)
            )
            .is_err()
    );
}
