use super::*;

fn replace_configuration(engine: &mut Engine, change: impl FnOnce(&mut Config)) {
    let mut config = engine.config().clone();
    change(&mut config);
    engine.config = ValidatedConfig::try_from(config).unwrap();
}

#[test]
fn aborted_payload_records_consume_global_and_principal_budgets_after_restart() {
    let mut f = Fixture::new();
    f.config.max_retained_workspaces = 3;
    f.config.max_retained_workspaces_per_principal = 2;
    let mut e = f.initialize();
    let repo = repository(&mut e, 1);
    for _ in 0..2 {
        let w = e.create_workspace(&repo.main_branch_id, "writer").unwrap();
        e.stage_bytes(&w.workspace_id, "retained", b"evidence")
            .unwrap();
        e.abort_workspace(&w.workspace_id, "writer").unwrap();
    }
    e.assert_workspace_budget_consistent();
    drop(e);
    let mut e = Engine::open(&f.root).unwrap();
    assert_eq!(e.workspaces.len(), 2);
    assert_eq!(
        e.create_workspace(&repo.main_branch_id, "writer")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ResourceLimit
    );
    let w = e.create_workspace(&repo.main_branch_id, "admin").unwrap();
    e.stage_bytes(&w.workspace_id, "retained", b"evidence")
        .unwrap();
    e.abort_workspace(&w.workspace_id, "admin").unwrap();
    assert_eq!(
        e.create_workspace(&repo.main_branch_id, "admin")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ResourceLimit
    );
    e.assert_workspace_budget_consistent();
    drop(e);
    let mut e = Engine::open(&f.root).unwrap();
    assert_eq!(e.workspaces.len(), 3);
    assert_eq!(
        e.create_workspace(&repo.main_branch_id, "admin")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ResourceLimit
    );
}

#[test]
fn namespace_growth_is_charged_and_rejection_preserves_durable_evidence() {
    for principal_limit in [false, true] {
        let mut f = Fixture::new();
        if principal_limit {
            f.config.max_retained_workspace_metadata_bytes_per_principal = 2048;
        } else {
            f.config.max_retained_workspace_metadata_bytes = 2048;
        }
        let mut e = f.initialize();
        let repo = repository(&mut e, 1);
        let w = e.create_workspace(&repo.main_branch_id, "admin").unwrap();
        let mut accepted = 0;
        loop {
            let before = canonical_json(&e.workspace(&w.workspace_id).unwrap()).unwrap();
            match e.namespace_operation(
                &w.workspace_id,
                NamespaceOperation::parse(
                    storage_contract::NamespaceOperationKind::Mkdir,
                    &(format!("directory-{accepted:04}")),
                    None,
                )
                .unwrap(),
            ) {
                Ok(_) => {
                    accepted += 1;
                    assert!(accepted < 100, "manifest admission must be bounded");
                }
                Err(error) => {
                    assert_eq!(error.code(), storage_contract::ErrorCode::ResourceLimit);
                    assert_eq!(
                        canonical_json(&e.workspace(&w.workspace_id).unwrap()).unwrap(),
                        before
                    );
                    break;
                }
            }
        }
        assert!(accepted > 0);
        // Reserved lifecycle headroom permits closure even at the byte ceiling.
        e.abort_workspace(&w.workspace_id, "admin").unwrap();
        e.assert_workspace_budget_consistent();
        drop(e);
        let mut e = Engine::open(&f.root).unwrap();
        let saved = e.workspace(&w.workspace_id).unwrap();
        assert_eq!(saved.state.as_str(), "aborted");
        assert_eq!(saved.namespace_operations.len(), accepted);
        assert_eq!(
            e.create_workspace(&repo.main_branch_id, "admin")
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::ResourceLimit
        );
    }
}

#[test]
fn uncertain_manifest_write_blocks_further_admission_until_restart() {
    let f = Fixture::new();
    let mut e = f.initialize();
    let repo = repository(&mut e, 1);
    let w = e.create_workspace(&repo.main_branch_id, "admin").unwrap();
    let manifest = f.config.disks[0]
        .root
        .join("staging/workspaces")
        .join(&w.workspace_id)
        .join("workspace.json");
    let bytes = fs::read(&manifest).unwrap();
    fs::remove_file(&manifest).unwrap();
    fs::create_dir(&manifest).unwrap();
    assert!(
        e.namespace_operation(
            &w.workspace_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Mkdir,
                "unacknowledged",
                None
            )
            .unwrap()
        )
        .is_err()
    );
    assert!(
        e.workspace(&w.workspace_id)
            .unwrap()
            .namespace_operations
            .is_empty()
    );
    assert!(e.create_workspace(&repo.main_branch_id, "admin").is_err());
    assert!(e.blocked);
    fs::remove_dir(&manifest).unwrap();
    fs::write(&manifest, bytes).unwrap();
    e.assert_workspace_budget_consistent();
    drop(e);
    let e = Engine::open(&f.root).unwrap();
    assert!(
        e.workspace(&w.workspace_id)
            .unwrap()
            .namespace_operations
            .is_empty()
    );
}

#[test]
fn over_budget_restart_fails_without_discarding_records() {
    let f = Fixture::new();
    let mut e = f.initialize();
    let repo = repository(&mut e, 1);
    for _ in 0..2 {
        let w = e.create_workspace(&repo.main_branch_id, "admin").unwrap();
        e.stage_bytes(&w.workspace_id, "retained", b"evidence")
            .unwrap();
        e.abort_workspace(&w.workspace_id, "admin").unwrap();
    }
    e.assert_workspace_budget_consistent();
    drop(e);
    let mut smaller = f.config.clone();
    smaller.max_retained_workspaces = 1;
    crate::disk::write_json(&f.root.join("config.json"), &smaller, true).unwrap();
    let error = match Engine::open(&f.root) {
        Ok(_) => panic!("startup must reject over-budget retained evidence"),
        Err(error) => error,
    };
    assert_eq!(error.code(), storage_contract::ErrorCode::ResourceLimit);
    crate::disk::write_json(&f.root.join("config.json"), &f.config, true).unwrap();
    assert_eq!(Engine::open(&f.root).unwrap().workspaces.len(), 2);
}

#[test]
fn rejected_initial_manifest_does_not_consume_durable_record_capacity() {
    let mut f = Fixture::new();
    f.config.max_retained_workspace_metadata_bytes = 1;
    let mut e = f.initialize();
    let repo = repository(&mut e, 1);
    for _ in 0..3 {
        assert_eq!(
            e.create_workspace(&repo.main_branch_id, "admin")
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::ResourceLimit
        );
        assert!(e.workspaces.is_empty());
    }
    for root in [
        f.root.join("workspaces"),
        f.config.disks[0].root.join("staging/workspaces"),
    ] {
        if root.exists() {
            assert_eq!(fs::read_dir(root).unwrap().count(), 0);
        }
    }
    e.assert_workspace_budget_consistent();
    drop(e);
    assert!(Engine::open(&f.root).unwrap().workspaces.is_empty());
}

#[test]
fn committed_file_context_receipt_fits_exact_reserved_metadata_budget() {
    let f = Fixture::new();
    let mut e = f.initialize();
    let repo = repository(&mut e, 1);
    save(&mut e, &repo.main_branch_id, "existing", b"earlier payload");
    let w = e.create_workspace(&repo.main_branch_id, "admin").unwrap();
    let old_contexts = w.selected_file_contexts.clone();
    assert!(!old_contexts.is_empty());
    e.namespace_operation(
        &w.workspace_id,
        NamespaceOperation::parse(
            storage_contract::NamespaceOperationKind::Mkdir,
            "extra-directory",
            None,
        )
        .unwrap(),
    )
    .unwrap();
    e.stage_bytes(&w.workspace_id, "nested/new-file", b"retained payload")
        .unwrap();
    let charge = e.retained_workspace_metadata_bytes();
    replace_configuration(&mut e, |config| {
        config.max_retained_workspace_metadata_bytes = charge;
        config.max_retained_workspace_metadata_bytes_per_principal = charge
    });
    crate::disk::write_json(&f.root.join("config.json"), &e.config, true).unwrap();
    e.commit_workspace(CommitRequest {
        workspace_id: w.workspace_id.clone(),
        operation_id: storage_contract::OperationId::generate(),
        principal: "admin".into(),
        expected_head: w.base_head,
        fencing_token: w.fencing_token,
        message: "exact metadata budget".into(),
        request_digest: None,
    })
    .unwrap();
    assert_eq!(
        e.workspace(&w.workspace_id).unwrap().state.as_str(),
        "committed"
    );
    assert_eq!(e.retained_workspace_metadata_bytes(), charge);
    let completed = e.workspace(&w.workspace_id).unwrap();
    for (key, context) in &old_contexts {
        assert_eq!(completed.selected_file_contexts.get(key), Some(context));
    }
    assert_eq!(completed.namespace_operations.len(), 1);
    e.assert_workspace_budget_consistent();
    drop(e);
    let e = Engine::open(&f.root).unwrap();
    assert_eq!(
        e.workspace(&w.workspace_id).unwrap().state.as_str(),
        "committed"
    );
    assert_eq!(e.retained_workspace_metadata_bytes(), charge);
    e.assert_workspace_budget_consistent();
}

#[test]
fn retained_record_owner_cannot_move_its_charge_to_another_principal() {
    let f = Fixture::new();
    let mut e = f.initialize();
    let repo = repository(&mut e, 1);
    let mut w = e.create_workspace(&repo.main_branch_id, "admin").unwrap();
    let charge = e.retained_workspace_metadata_bytes();
    w.principal = "another-writer".into();
    assert_eq!(
        e.persist_workspace(&w).map(|_| ()).unwrap_err().code(),
        storage_contract::ErrorCode::StateInvalid
    );
    assert_eq!(e.retained_workspace_metadata_bytes(), charge);
    e.assert_workspace_budget_consistent();
}

#[test]
fn zero_byte_generations_consume_retained_entry_budget_without_extra_artifacts() {
    let mut f = Fixture::new();
    f.config.max_entries = 2;
    let mut e = f.initialize();
    let repo = repository(&mut e, 1);
    let w = e.create_workspace(&repo.main_branch_id, "admin").unwrap();
    let source = f.temp.path().join("empty");
    fs::write(&source, b"").unwrap();
    let item = new_id("entry");
    let first = e
        .stage_upload(&w.workspace_id, &item, "empty", &source, 1)
        .unwrap();
    let second = e
        .stage_upload(&w.workspace_id, &item, "empty", &source, 2)
        .unwrap();
    let directory = first.source_path.parent().unwrap();
    let names = || {
        let mut entries: Vec<_> = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        entries.sort();
        entries
    };
    let before = names();
    for _ in 0..3 {
        assert_eq!(
            e.stage_upload(&w.workspace_id, &item, "empty", &source, 3)
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::ResourceLimit
        );
        assert_eq!(
            names(),
            before,
            "rejected generation must not create a scratch artifact"
        );
    }
    assert_eq!(fs::metadata(&first.source_path).unwrap().len(), 0);
    assert_eq!(fs::metadata(&second.source_path).unwrap().len(), 0);
    e.stage_upload(&w.workspace_id, &item, "empty", &source, 2)
        .unwrap();
    e.assert_workspace_budget_consistent();
    drop(e);
    let mut e = Engine::open(&f.root).unwrap();
    assert_eq!(
        e.stage_upload(&w.workspace_id, &item, "empty", &source, 3)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ResourceLimit
    );
    assert_eq!(names(), before);
}

// Assert the public wire representation as well as the typed observations.
fn capacity_report(engine: &Engine, principal: &str) -> json_codec::Value {
    json_codec::to_value(engine.workspace_capacity(principal)).unwrap()
}

#[test]
fn capacity_reports_resident_lifecycle_counts_and_only_selected_principal() {
    let mut f = Fixture::new();
    f.config.max_retained_workspaces = 10;
    f.config.max_retained_workspaces_per_principal = 5;
    let mut e = f.initialize();
    let repo = repository(&mut e, 1);
    assert_eq!(capacity_report(&e, "admin")["warning"], "ok");
    // Published records remain cold and byte-charged. Unpublished payloads
    // still consume resident slots, including after abort and restart.
    let saved = save(&mut e, &repo.main_branch_id, "saved.txt", b"saved");
    let published = capacity_report(&e, "admin");
    assert_eq!(published["global"]["records"]["used"], "0");
    assert_eq!(published["global"]["cold_records"], "1");
    assert!(e.retained_workspace_metadata_bytes() > 0);
    for _ in 0..4 {
        let w = e.create_workspace(&repo.main_branch_id, "admin").unwrap();
        e.stage_bytes(&w.workspace_id, "retained", b"evidence")
            .unwrap();
        e.abort_workspace(&w.workspace_id, "admin").unwrap();
    }
    let report = capacity_report(&e, "admin");
    assert_eq!(report["global"]["records"]["used"], "4");
    assert_eq!(report["global"]["cold_records"], "1");
    assert_eq!(report["principal"]["cold_records"], "1");
    assert_eq!(report["principal"]["records"]["remaining"], "1");
    assert_eq!(report["warning"], "near_limit");
    assert_eq!(report["automatic_retirement"], true);
    let unknown = capacity_report(&e, "absent-actor");
    assert_eq!(unknown["principal"]["records"]["used"], "0");
    assert_eq!(unknown["principal"]["metadata_bytes"]["used"], "0");
    assert_eq!(unknown["warning"], "ok");
    assert!(
        !canonical_json(&unknown)
            .unwrap()
            .windows(5)
            .any(|w| w == b"admin")
    );
    let charged = e.retained_workspace_metadata_bytes();
    assert_eq!(
        report["global"]["metadata_bytes"]["used"],
        charged.to_string()
    );
    drop(e);
    let mut e = Engine::open(&f.root).unwrap();
    assert_eq!(capacity_report(&e, "admin"), report);
    assert_eq!(e.published_receipt(saved.operation_id).unwrap(), Some(saved));
    let w = e.create_workspace(&repo.main_branch_id, "admin").unwrap();
    e.stage_bytes(&w.workspace_id, "retained", b"evidence")
        .unwrap();
    e.abort_workspace(&w.workspace_id, "admin").unwrap();
    assert_eq!(capacity_report(&e, "admin")["warning"], "exhausted");
    assert_eq!(
        capacity_report(&e, "admin")["principal"]["records"]["remaining"],
        "0"
    );
    let before = capacity_report(&e, "admin");
    let error = e
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::ResourceLimit);
    assert!(error.to_string().contains("principal record count"));
    assert_eq!(capacity_report(&e, "admin"), before);
}

#[test]
fn capacity_metadata_warning_boundaries_are_exact_and_overflow_safe() {
    let f = Fixture::new();
    let mut e = f.initialize();
    let repo = repository(&mut e, 1);
    e.create_workspace(&repo.main_branch_id, "admin").unwrap();
    let used = e.retained_workspace_metadata_bytes();
    replace_configuration(&mut e, |config| {
        config.max_retained_workspace_metadata_bytes = used * 5 / 4 + 1
    });
    assert_eq!(capacity_report(&e, "admin")["warning"], "ok");
    replace_configuration(&mut e, |config| {
        config.max_retained_workspace_metadata_bytes = used * 5 / 4
    });
    assert_eq!(capacity_report(&e, "admin")["warning"], "near_limit");
    replace_configuration(&mut e, |config| {
        config.max_retained_workspace_metadata_bytes = used
    });
    assert_eq!(capacity_report(&e, "admin")["warning"], "exhausted");
    replace_configuration(&mut e, |config| {
        config.max_retained_workspace_metadata_bytes = u64::MAX;
        config.max_retained_workspace_metadata_bytes_per_principal = used
    });
    assert_eq!(capacity_report(&e, "admin")["warning"], "exhausted");
    assert_eq!(capacity_report(&e, "absent-actor")["warning"], "ok");
    replace_configuration(&mut e, |config| {
        config.max_retained_workspace_metadata_bytes_per_principal = u64::MAX
    });
    assert_eq!(capacity_report(&e, "admin")["warning"], "ok");
}

#[test]
fn capacity_reports_global_pressure_even_without_actor_charges() {
    let f = Fixture::new();
    let mut e = f.initialize();
    let repo = repository(&mut e, 1);
    e.create_workspace(&repo.main_branch_id, "admin").unwrap();
    replace_configuration(&mut e, |config| config.max_retained_workspaces = 1);
    let report = capacity_report(&e, "absent-actor");
    assert_eq!(report["global"]["records"]["used"], "1");
    assert_eq!(report["global"]["records"]["remaining"], "0");
    assert_eq!(report["principal"]["records"]["used"], "0");
    assert_eq!(report["warning"], "exhausted");
}

#[test]
fn admission_errors_identify_each_exhausted_dimension_without_changing_evidence() {
    for dimension in [
        "global record count",
        "principal record count",
        "global metadata bytes",
        "principal metadata bytes",
    ] {
        let f = Fixture::new();
        let mut e = f.initialize();
        let repo = repository(&mut e, 1);
        let w = e.create_workspace(&repo.main_branch_id, "admin").unwrap();
        e.stage_bytes(&w.workspace_id, "retained", b"evidence")
            .unwrap();
        e.abort_workspace(&w.workspace_id, "admin").unwrap();
        let used = e.retained_workspace_metadata_bytes();
        replace_configuration(&mut e, |config| match dimension {
            "global record count" => config.max_retained_workspaces = 1,
            "principal record count" => config.max_retained_workspaces_per_principal = 1,
            "global metadata bytes" => config.max_retained_workspace_metadata_bytes = used,
            "principal metadata bytes" => {
                config.max_retained_workspace_metadata_bytes_per_principal = used
            }
            other => panic!("unexpected test dimension: {other}"),
        });
        let before = capacity_report(&e, "admin");
        let manifest_before = canonical_json(e.workspaces.get(&w.workspace_id).unwrap()).unwrap();
        let error = e
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap_err();
        assert_eq!(error.code(), storage_contract::ErrorCode::ResourceLimit);
        assert!(error.to_string().contains(dimension), "{error}");
        assert_eq!(capacity_report(&e, "admin"), before);
        assert_eq!(
            canonical_json(e.workspaces.get(&w.workspace_id).unwrap()).unwrap(),
            manifest_before
        );
        e.assert_workspace_budget_consistent();
    }
}

#[test]
fn typed_capacity_projection_always_contains_global_and_actor_scopes() {
    let mut f = Fixture::new();
    f.config.disks.truncate(1);
    let e = f.initialize();
    let report = e.workspace_capacity("admin");
    assert_eq!(report.global.records.used, 0);
    assert_eq!(report.global.records.limit, 4096);
    assert_eq!(report.global.records.remaining, report.global.records.limit);
    assert_eq!(report.global.metadata_bytes.used, 0);
    assert_eq!(report.principal.records.used, 0);
    assert_eq!(report.warning, CapacityWarning::Ok);
    assert!(report.automatic_retirement);
}
