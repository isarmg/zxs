//! Exact native mixed-snapshot proofs; these are not metadata-only model tests.
use super::*;
use crate::protection::RecursiveBackupMode;

fn published_tree() -> (
    Fixture,
    Arc<CurrentNamespaces>,
    Arc<State>,
    OperationResult,
    Vec<Prepared>,
) {
    let mut fixture = ready();
    fixture
        .engine
        .file_namespace(
            &fixture.repository.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "docs/empty", None).unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let scope = fixture
        .engine
        .backup_configuration(&fixture.repository.repository_id, "docs")
        .unwrap()
        .scope;
    let view = Arc::clone(fixture.engine.current_namespaces.as_ref().unwrap());
    let previous = Arc::clone(&fixture.engine.state);
    let request = RecursiveBackupConfigurationRequest {
        repository_id: &fixture.repository.repository_id,
        path: "docs",
        expected_scope_id: &scope.id,
        target_copies: 3,
        expected_generation: 0,
        capture: &capture,
        principal: "admin",
        operation_id: op(),
    };
    let plan = view
        .plan_recursive_backup_configuration(
            &previous,
            request,
            RecursiveBackupMode::DirectoryTree,
            || Ok(new_id("policy")?),
        )
        .unwrap();
    let prepared = plan
        .versions
        .iter()
        .map(|version| Prepared {
            version: version.clone(),
            placement: plan.placements[&version.version_id].clone(),
            source: fixture
                .engine
                .valid_backup_replica(&version.version_id)
                .unwrap(),
        })
        .collect();
    let receipt = fixture
        .engine
        .configure_recursive_backup_tree(request)
        .unwrap();
    (fixture, view, previous, receipt, prepared)
}

#[test]
fn mixed_snapshot_native_proof_rejects_rows_identity_omission_and_kind_downgrade() {
    let (fixture, view, previous, receipt, _) = published_tree();
    let next = fixture.engine.state();
    view.advance(&previous, next).unwrap();
    assert_eq!(receipt.kind, "backup.configure-recursive-tree");
    for attack in [
        "missing",
        "path",
        "entry_id",
        "omitted-row",
        "extra-row",
        "binding",
        "legacy-kind",
    ] {
        let mut forged = next.clone();
        let result = forged.operations.get_mut(&receipt.operation_id).unwrap();
        match attack {
            "missing" => {
                result
                    .result
                    .as_object_mut()
                    .unwrap()
                    .remove("directory_namespace");
            }
            "path" | "entry_id" => {
                result.result["directory_namespace"]["replicas"]["C01"]["directories"][0][attack] =
                    json!("forged");
            }
            "omitted-row" => {
                result.result["directory_namespace"]["replicas"]["C01"]["directories"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            }
            "extra-row" => {
                let rows = result.result["directory_namespace"]["replicas"]["C01"]["directories"]
                    .as_array_mut()
                    .unwrap();
                rows.push(rows[0].clone());
            }
            "binding" => {
                result.result["directory_namespace"]["replicas"]["C01"]["identity"]["binding_generation"] =
                    json!("99");
            }
            "legacy-kind" => {
                result.kind = "backup.configure-recursive".into();
                result.request_digest = Engine::native_request_digest(
                    &result.kind,
                    "admin",
                    &result.result["backup_request"],
                )
                .unwrap();
                forged.events.last_mut().unwrap().kind = result.kind.clone();
            }
            _ => unreachable!(),
        }
        rejected(&view, &previous, &forged, attack);
    }
    let mut path_write = next.clone();
    path_write
        .file_locations
        .insert(fixture.first.file_id.clone(), "unrequested/move".into());
    rejected(
        &view,
        &previous,
        &path_write,
        "mixed configuration is not a logical rename",
    );
}

#[test]
fn mixed_snapshot_binds_both_prepared_and_candidate_full_file_payloads() {
    use crate::journal::{Candidate, CandidateData, VersionEvidence};
    let (fixture, view, previous, receipt, mut prepared) = published_tree();
    let next = fixture.engine.state();
    view.advance(&previous, next).unwrap();
    assert!(
        prepared.len() >= 2,
        "fixture must retain multiple actual file bodies"
    );
    view.verify_backup_prepared(&previous, next, &receipt, &prepared)
        .unwrap();
    let snapshot = crate::journal::prepare_snapshot(next, MAX_METADATA).unwrap();
    let (catalog, _) =
        crate::history_package::prepare_history_manifests(next, MAX_METADATA).unwrap();
    let generation = fixture.engine.config.generation().unwrap();
    let candidate = Candidate::new_with_registry(
        CandidateData {
            pool_id: fixture.config.pool_id.clone(),
            sequence: next.sequence,
            transaction_id: new_id("tx").unwrap(),
            operation_id: receipt.operation_id,
            request_digest: receipt.request_digest.clone(),
            previous_snapshot: None,
            snapshot_hash: snapshot.hash.clone(),
            versions: prepared
                .iter()
                .map(|copy| VersionEvidence {
                    version: copy.version.clone(),
                    placement: copy.placement.clone(),
                })
                .collect(),
            metadata_disk_ids: fixture
                .engine
                .members()
                .iter()
                .map(|disk| disk.disk_id.clone())
                .collect(),
            history_roots: catalog.reference().clone(),
        },
        fixture.engine.config.generation_hash().unwrap(),
        generation.portable.exit_registry().unwrap(),
    );
    assert!(
        view.verify_backup_candidate_payloads(&previous, next, &candidate)
            .unwrap()
    );
    for attack in ["omitted", "duplicate", "placement"] {
        let mut forged = candidate.clone();
        match attack {
            "omitted" => {
                forged.versions.pop();
            }
            "duplicate" => {
                forged.versions[1] = forged.versions[0].clone();
            }
            "placement" => {
                forged.versions[0].placement.route_slot_ids.pop();
            }
            _ => unreachable!(),
        }
        assert!(
            view.verify_backup_candidate_payloads(&previous, next, &forged)
                .is_err(),
            "{attack}"
        );
    }
    prepared.pop();
    assert!(
        view.verify_backup_prepared(&previous, next, &receipt, &prepared)
            .is_err()
    );
}

#[test]
fn mixed_snapshot_paths_cannot_be_changed_by_a_downgraded_namespace_envelope() {
    let (mut fixture, _, _, configured, _) = published_tree();
    let previous = Arc::clone(&fixture.engine.state);
    let view = Arc::clone(fixture.engine.current_namespaces.as_ref().unwrap());
    let renamed = fixture
        .engine
        .file_namespace(
            &fixture.repository.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Rename, "docs", Some("moved"))
                .unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    let next = fixture.engine.state();
    view.advance(&previous, next).unwrap();
    let mut forged = next.clone();
    forged.protection_policies = previous.protection_policies.clone();
    let receipt = forged.operations.get_mut(&renamed.operation_id).unwrap();
    let request = receipt.result["namespace_request"].clone();
    receipt.kind = "file.namespace".into();
    receipt.request_digest =
        Engine::native_request_digest(&receipt.kind, "admin", &request).unwrap();
    let body = receipt.result.as_object_mut().unwrap();
    body.remove("recursive_follow");
    body.remove("recursive_directory_follow");
    body.remove("namespace_request");
    forged.events.last_mut().unwrap().kind = "file.namespace".into();
    rejected(
        &view,
        &previous,
        &forged,
        "old envelope with mixed-mode path writes",
    );

    // The legacy envelope retains its fixed semantics only with paths unchanged.
    // This positive control ensures the negative is specifically about path writes.
    forged.file_backup_paths = previous.file_backup_paths.clone();
    view.advance(&previous, &forged).unwrap();
    assert_eq!(next.operations[&configured.operation_id], configured);

    // The mixed-mode guard must remain after following updates the authority ID.
    let view = fixture.engine.current_namespaces.as_ref().unwrap();
    let mut after_follow = next.clone();
    after_follow
        .file_backup_paths
        .get_mut(&fixture.first.file_id)
        .unwrap()
        .insert("B01".into(), "forged/path".into());
    rejected(view, next, &after_follow, "mode guard survives following");
}
