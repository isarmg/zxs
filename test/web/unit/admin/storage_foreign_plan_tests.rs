use super::*;
use storage_contract::storage_directories::{CandidateSelection, DiskRoleEvidence};

#[path = "storage_foreign_plan/recovery_tests.rs"]
mod recovery;

fn id(prefix: &str, n: u8) -> String {
    format!("{prefix}_{n:032x}")
}
fn operation(n: u8) -> OperationId {
    id("op", n).parse().unwrap()
}
fn candidate(path: &str, inode: u64) -> CandidateSelection {
    let mut selection = super::super::super::tests::candidate(path);
    selection.root_identity.inode = inode;
    selection
}
fn identity(n: u8) -> ForeignDiskIdentity {
    ForeignDiskIdentity {
        pool_id: id("pool", n),
        disk_id: id("disk", n),
        slot_id: "P01".into(),
        binding_generation: u64::from(n),
    }
}
fn source() -> ForeignDiskPreview {
    let selected = candidate("/mnt/source", 8);
    ForeignDiskPreview {
        source: DirectorySource {
            path: selected.root_path,
            discovery_identity: selected.discovery_identity,
            identity: selected.root_identity,
        },
        identity: identity(3),
        source_level: 3,
        role_evidence: DiskRoleEvidence::GenerationEvidence,
        seal_digest: "a".repeat(64),
        review_digest: "b".repeat(64),
        checkpoint: ForeignDiskCheckpoint {
            candidate_hash: "c".repeat(64),
            publication_hash: "d".repeat(64),
            sequence: 9007199254740993,
            authority_hash: None,
            unpublished_tail: false,
        },
        histories: (1..=2)
            .map(|n| ForeignHistorySummary {
                group_id: format!("{n:064x}"),
                history_tree_id: id("history", n),
                repository_name: "Same name".into(),
                origin: identity(n),
                manifest_hash: "f".repeat(64),
                branches: 1,
                directory_branches: 1,
                directory_trees: 2,
                commit_records: 3,
                versions: 4,
                verified_local_versions: 1,
                temporarily_unavailable_versions: vec![id("ver", n)],
                recorded_purged_versions: 1,
                recorded_migrated_versions: 1,
            })
            .collect(),
        execution: ForeignExecutionCapability::Unavailable,
    }
}
fn draft(source: &ForeignDiskPreview) -> Draft {
    Draft {
        slot: "P09".into(),
        parent: "empty-parent".into(),
        names: source
            .histories
            .iter()
            .enumerate()
            .map(|(n, h)| (h.group_id.clone(), format!("origin-{n}")))
            .collect(),
        ..Default::default()
    }
}
pub(super) fn review() -> ForeignPlanPreview {
    let source = source();
    let request = draft(&source).request(&source, &[], operation(1)).unwrap();
    let mut target = identity(9);
    target.slot_id = "P09".into();
    ForeignPlanPreview {
        target: ForeignTargetMapping {
            identity: target,
            sequence: 9007199254740995,
            source_digest: source.review_digest.clone(),
            mapping_digest: "e".repeat(64),
            histories: source
                .histories
                .iter()
                .enumerate()
                .map(|(n, h)| ForeignMappedHistory {
                    group_id: h.group_id.clone(),
                    history_tree_id: h.history_tree_id.clone(),
                    destination: format!("empty-parent/origin-{n}"),
                })
                .collect(),
        },
        request,
        source,
        review_digest: "f".repeat(64),
        execution: ForeignExecutionCapability::Unavailable,
        blocker: ForeignPlanBlocker::PortableImportWriterUnavailable,
    }
}
fn failure(status: u16) -> api::Error {
    api::Error {
        code: storage_contract::ErrorCode::PlanStale,
        message: "exact cause".into(),
        retryable: false,
        status,
        request_id: Some("original-request".into()),
        source: None,
    }
}
fn reviewed_model() -> Model {
    let mut model = Model {
        draft: draft(&source()),
        ..Default::default()
    };
    model.accept_preview(review());
    model
}
#[test]
fn same_names_and_slots_keep_distinct_complete_origins_and_histories() {
    let review = review();
    let result = read_preview(json!(review), &review.request, &review.source).unwrap();
    assert_eq!(result, review);
    assert_eq!(result.target.histories.len(), 2);
    assert_ne!(
        result.source.histories[0].origin.pool_id,
        result.source.histories[1].origin.pool_id
    );
    assert_eq!(
        result.source.histories[0].origin.slot_id,
        result.source.histories[1].origin.slot_id
    );
    assert_eq!(
        result.source.histories[0].temporarily_unavailable_versions,
        vec![id("ver", 1)]
    );
    assert_eq!(result.target.sequence, 9007199254740995);
}
#[test]
fn missing_or_extra_source_group_is_not_a_complete_mapping() {
    for extra in [false, true] {
        let source = source();
        let mut draft = draft(&source);
        if extra {
            draft.names.insert("0".repeat(64), "extra".into());
        } else {
            draft.names.remove(&source.histories[0].group_id);
        }
        assert_eq!(draft.intent(&source, &[]), Err(DraftError::CompleteMapping));
    }
}
#[test]
fn normalized_duplicate_destination_names_are_rejected() {
    let source = source();
    let mut draft = draft(&source);
    for (name, value) in draft.names.values_mut().zip(["Folder", "folder"]) {
        *name = value.into();
    }
    assert_eq!(draft.intent(&source, &[]), Err(DraftError::Collision));
}
#[test]
fn invalid_name_parent_or_slot_stays_a_local_input_error() {
    for n in 0..3 {
        let source = source();
        let mut draft = draft(&source);
        let expected = match n {
            0 => {
                draft.slot.clear();
                DraftError::Slot
            }
            1 => {
                draft.parent = "../existing".into();
                DraftError::Parent
            }
            _ => {
                *draft.names.values_mut().next().unwrap() = "../escape".into();
                DraftError::Name
            }
        };
        assert_eq!(draft.intent(&source, &[]), Err(expected));
    }
}
#[test]
fn root_target_is_explicit_null_and_each_history_still_has_its_own_child() {
    let source = source();
    let mut draft = draft(&source);
    draft.parent.clear();
    let request = draft.request(&source, &[], operation(2)).unwrap();
    assert_eq!(request.target.parent, None);
    assert_eq!(request.target.history_names.len(), 2);
    assert!(json!(request)["target"]["parent"].is_null());
}
#[test]
fn new_primary_draft_does_not_impose_a_copy_target_or_become_a_current_request() {
    let source = source();
    let mut draft = draft(&source);
    draft.mode = Mode::AdmitNewPrimary;
    assert!(matches!(
        draft.intent(&source, &[]),
        Ok(Intent::AdmitNewPrimary {
            copy_target: None,
            ..
        })
    ));
    assert_eq!(
        draft.request(&source, &[], operation(3)),
        Err(DraftError::PreviewUnavailable)
    );
    draft.empty_target = Some(candidate("/mnt/target", 9));
    assert!(matches!(
        draft.intent(&source, &[]),
        Ok(Intent::AdmitNewPrimary { .. })
    ));
    assert_eq!(
        draft.request(&source, &[], operation(3)),
        Err(DraftError::PreviewUnavailable)
    );
    draft.empty_target = Some(candidate("/mnt/source", 8));
    assert_eq!(
        draft.intent(&source, &[]),
        Err(DraftError::SourceTargetAlias)
    );
}
#[test]
fn third_to_second_requires_explicit_exact_primary_without_default_selection() {
    let source = source();
    let mut model = reviewed_model();
    let mut draft = draft(&source);
    draft.mode = Mode::AdmitSecondary;
    draft.slot = "B01".into();
    assert_eq!(
        draft.intent(&source, &model.known_primary),
        Err(DraftError::ExplicitPrimary)
    );
    draft.primary = Some(model.known_primary[0].clone());
    assert!(matches!(
        draft.intent(&source, &model.known_primary),
        Ok(Intent::AdmitSecondary {
            copy_target: None,
            ..
        })
    ));
    assert_eq!(
        draft.request(&source, &model.known_primary, operation(4)),
        Err(DraftError::PreviewUnavailable)
    );
    model.known_primary[0].identity.binding_generation += 1;
    assert_eq!(
        draft.intent(&source, &model.known_primary),
        Err(DraftError::PrimaryChanged)
    );
}
#[test]
fn secondary_intent_rejects_other_source_tiers_or_same_primary_slot() {
    let model = reviewed_model();
    let mut source = source();
    let mut draft = draft(&source);
    draft.mode = Mode::AdmitSecondary;
    draft.empty_target = Some(candidate("/mnt/target", 9));
    draft.primary = Some(model.known_primary[0].clone());
    assert_eq!(
        draft.intent(&source, &model.known_primary),
        Err(DraftError::PrimaryChanged)
    );
    draft.slot = "B01".into();
    for level in [1, 2, 4] {
        source.source_level = level;
        assert_eq!(
            draft.intent(&source, &model.known_primary),
            Err(DraftError::SourceTier)
        );
    }
}
#[test]
fn primary_binding_includes_pool_disk_slot_generation_and_checkpoint() {
    for field in 0..5 {
        let model = reviewed_model();
        let source = source();
        let mut draft = draft(&source);
        draft.mode = Mode::AdmitSecondary;
        draft.slot = "B01".into();
        draft.empty_target = Some(candidate("/mnt/target", 9));
        let mut primary = model.known_primary[0].clone();
        match field {
            0 => primary.identity.pool_id = id("pool", 22),
            1 => primary.identity.disk_id = id("disk", 22),
            2 => primary.identity.slot_id = "P22".into(),
            3 => primary.identity.binding_generation += 1,
            _ => primary.sequence += 1,
        };
        draft.primary = Some(primary);
        assert_eq!(
            draft.intent(&source, &model.known_primary),
            Err(DraftError::PrimaryChanged)
        );
    }
}
#[test]
fn response_must_bind_original_request_source_and_exact_complete_paths() {
    let expected = review();
    for field in 0..11 {
        let mut actual = expected.clone();
        match field {
            0 => actual.request.operation_id = operation(9),
            1 => actual.source.source.path = "/mnt/replaced".into(),
            2 => actual.target.source_digest = "0".repeat(64),
            3 => actual.target.identity.slot_id = "P88".into(),
            4 => actual.target.identity.binding_generation = 0,
            5 => actual.review_digest = "bad".into(),
            6 => actual.target.mapping_digest = "bad".into(),
            7 => {
                actual.target.histories.pop();
            }
            8 => actual.target.histories[1] = actual.target.histories[0].clone(),
            9 => actual.target.histories[0].history_tree_id = id("history", 55),
            _ => actual.target.histories[0].destination = "other/origin-0".into(),
        };
        assert!(
            read_preview(json!(actual), &expected.request, &expected.source).is_err(),
            "field {field}"
        );
    }
}
#[test]
fn unknown_execution_capability_or_success_fields_cannot_forge_import_completion() {
    let expected = review();
    for field in 0..4 {
        let mut value = json!(expected);
        match field {
            0 => value["execution"] = "ready".into(),
            1 => value["blocker"] = "none".into(),
            2 => value["result"] = json!({"state":"published"}),
            _ => value["source"]["execution"] = "completed".into(),
        };
        assert!(read_preview(value, &expected.request, &expected.source).is_err());
    }
}
#[test]
fn editing_any_meaningful_input_clears_review_and_acknowledgment() {
    let mut model = reviewed_model();
    model.acknowledged = true;
    let generation = model.generation;
    model.edit(|draft| draft.parent = "another-empty-parent".into());
    assert!(model.review.is_none());
    assert!(!model.acknowledged);
    assert!(model.generation > generation);
    assert!(!model.can_save());
}
#[test]
fn equal_input_keeps_review_and_does_not_reauthorize_changes() {
    let mut model = reviewed_model();
    model.acknowledged = true;
    let generation = model.generation;
    let review = model.review.clone();
    model.edit(|draft| draft.parent = "empty-parent".into());
    assert_eq!(model.generation, generation);
    assert_eq!(model.review, review);
    assert!(model.acknowledged);
}

fn selected_review() -> super::super::super::Model {
    let mut storage = super::super::super::Model {
        snapshot: Some(super::super::super::Snapshot {
            state: super::super::super::StorageState::Active,
            _disk_ids: vec![],
            completion_available: false,
            allow_development: false,
            terminal: None,
        }),
        ..Default::default()
    };
    let source = source();
    super::super::super::wizard::tests::open_discovered_source(&mut storage, source.source.clone());
    storage.foreign.preview = Some(source);
    storage.foreign.plan = reviewed_model();
    storage
}

#[test]
fn repeated_candidate_selection_preserves_reviewed_and_local_drafts() {
    for mode in [
        Mode::ReconstructExistingPrimary,
        Mode::AdmitNewPrimary,
        Mode::AdmitSecondary,
    ] {
        let mut storage = selected_review();
        storage.foreign.plan.edit(|draft| draft.mode = mode);
        storage.foreign.plan.acknowledged = mode == Mode::ReconstructExistingPrimary;
        let before = storage.foreign.clone();
        for _ in 0..2 {
            super::super::super::wizard::choose(&mut storage, "/mnt/source");
        }
        let actual = &storage.foreign;
        assert_eq!(actual.generation, before.generation);
        assert_eq!(actual.preview, before.preview);
        assert_eq!(actual.plan.generation, before.plan.generation);
        assert_eq!(actual.plan.draft, before.plan.draft);
        assert_eq!(actual.plan.review, before.plan.review);
        assert_eq!(actual.plan.known_primary, before.plan.known_primary);
        assert_eq!(actual.plan.acknowledged, before.plan.acknowledged);
        assert_eq!(actual.plan.save, SaveState::Unsubmitted);
    }
}

#[test]
fn selecting_a_different_candidate_invalidates_the_original_source_and_review() {
    let mut storage = selected_review();
    storage.foreign.plan.acknowledged = true;
    let source_generation = storage.foreign.generation;
    let plan_generation = storage.foreign.plan.generation;
    super::super::super::wizard::choose(&mut storage, "/mnt/ordinary");
    assert!(storage.wizard.selected("/mnt/ordinary"));
    assert!(storage.foreign.generation > source_generation);
    assert!(storage.foreign.preview.is_none());
    assert!(storage.foreign.plan.generation > plan_generation);
    assert_eq!(storage.foreign.plan.draft, Draft::default());
    assert!(storage.foreign.plan.review.is_none());
    assert!(storage.foreign.plan.known_primary.is_empty());
    assert!(!storage.foreign.plan.acknowledged);
}

#[test]
fn repeated_candidate_selection_preserves_the_saved_immutable_review() {
    let mut storage = selected_review();
    let saved = ForeignPlanStatus {
        preview: review(),
        validation: ForeignPlanValidation::MatchedAtObservation,
    };
    storage.foreign.plan.accept_saved(saved.clone());
    super::super::super::wizard::choose(&mut storage, "/mnt/source");
    assert_eq!(storage.foreign.plan.save, SaveState::Stored);
    assert_eq!(storage.foreign.plan.review, Some(saved.preview.clone()));
    assert_eq!(storage.foreign.plan.stored, Some(saved));
    assert!(!storage.foreign.plan.can_save());
}

#[test]
fn candidate_clicks_do_not_replace_a_sending_or_unknown_review_save() {
    for unknown in [false, true] {
        let mut storage = selected_review();
        storage.foreign.plan.acknowledged = true;
        let original = storage.foreign.plan.save_request().unwrap();
        if unknown {
            storage.foreign.plan.save_failed(failure(503));
        }
        let before = storage.foreign.clone();
        for path in ["/mnt/source", "/mnt/ordinary", "/mnt/not-discovered"] {
            super::super::super::wizard::choose(&mut storage, path);
        }
        assert!(storage.wizard.selected("/mnt/source"));
        assert_eq!(storage.foreign.generation, before.generation);
        assert_eq!(storage.foreign.preview, before.preview);
        assert_eq!(storage.foreign.plan.generation, before.plan.generation);
        assert_eq!(storage.foreign.plan.review, Some(original.preview));
        assert_eq!(storage.foreign.plan.save, before.plan.save);
        assert!(storage.foreign.plan.blocks_edit());
    }
}

#[test]
fn saving_requires_explicit_acknowledgment_and_duplicate_clicks_do_not_send_again() {
    let mut model = reviewed_model();
    assert!(model.save_request().is_none());
    model.acknowledged = true;
    let request = model.save_request().unwrap();
    assert!(request.acknowledge_plan_only);
    assert_eq!(request.preview, review());
    assert!(model.save_request().is_none());
    assert!(model.blocks_edit());
}
#[test]
fn unknown_save_retains_exact_operation_body_and_survives_navigation() {
    let mut model = reviewed_model();
    model.acknowledged = true;
    let original = model.save_request().unwrap();
    model.save_failed(failure(503));
    model.leave();
    model.invalidate();
    model.edit(|draft| draft.slot = "P22".into());
    assert_eq!(model.save, SaveState::Unknown);
    assert_eq!(model.review, Some(original.preview.clone()));
    assert_eq!(model.draft.slot, "P09");
    assert_eq!(model.save_request(), Some(original));
}
#[test]
fn malformed_success_is_unknown_but_explicit_rejection_requires_new_review() {
    for status in [0, 200, 503] {
        let mut model = reviewed_model();
        model.acknowledged = true;
        model.save_request().unwrap();
        model.save_failed(failure(status));
        assert_eq!(model.save, SaveState::Unknown);
    }
    let mut model = reviewed_model();
    model.acknowledged = true;
    model.save_request().unwrap();
    model.save_failed(failure(409));
    assert_eq!(model.save, SaveState::Rejected);
    assert!(!model.blocks_edit());
    assert!(!model.can_save());
}
#[test]
fn saving_navigation_keeps_the_response_generation_and_original_receipt_fence() {
    let mut model = reviewed_model();
    model.acknowledged = true;
    model.save_request().unwrap();
    let generation = model.generation;
    model.leave();
    model.invalidate();
    assert_eq!(model.generation, generation);
    assert_eq!(model.save, SaveState::Sending);
}
#[test]
fn late_read_only_preview_is_invalidated_on_navigation() {
    let mut model = reviewed_model();
    model.previewing = true;
    let generation = model.generation;
    model.leave();
    assert!(!model.previewing);
    assert!(model.generation > generation);
    model.invalidate();
    assert!(model.review.is_none());
}
#[test]
fn status_and_revalidation_require_the_original_complete_review_and_correct_validation() {
    let expected = review();
    let status = ForeignPlanStatus {
        preview: expected.clone(),
        validation: ForeignPlanValidation::Required,
    };
    assert!(read_status(json!(status), &expected, ForeignPlanValidation::Required).is_ok());
    assert!(
        read_status(
            json!(status),
            &expected,
            ForeignPlanValidation::MatchedAtObservation
        )
        .is_err()
    );
    let mut changed = status;
    changed.preview.target.sequence += 1;
    assert!(read_status(json!(changed), &expected, ForeignPlanValidation::Required).is_err());
}
#[test]
fn saved_plan_and_matched_observation_never_enable_import() {
    let mut model = reviewed_model();
    for validation in [
        ForeignPlanValidation::Required,
        ForeignPlanValidation::MatchedAtObservation,
    ] {
        model.accept_saved(ForeignPlanStatus {
            preview: review(),
            validation,
        });
        assert_eq!(model.save, SaveState::Stored);
        assert!(!model.can_save());
        assert_eq!(
            model.stored.as_ref().unwrap().preview.execution,
            ForeignExecutionCapability::Unavailable
        );
    }
}
#[test]
fn ui_has_no_execute_request_and_retains_explicit_plan_only_copy() {
    let actions = include_str!("../../../../src/web/src/admin/storage/foreign/plan/actions.rs");
    let view = include_str!("../../../../src/web/src/admin/storage/foreign/plan/view.rs");
    assert!(!actions.contains("/execute"));
    assert!(!actions.contains("/commit"));
    assert!(view.contains("data-foreign-plan-confirm"));
    assert!(view.contains("data-foreign-plan-source"));
    assert!(view.contains("data-foreign-plan-execution-unavailable"));
    assert!(view.contains("this is not an import completion receipt"));
    assert!(view.contains("data-foreign-plan-retry"));
}

#[test]
fn failed_status_read_cannot_clear_an_uncertain_save_or_manufacture_non_creation() {
    let mut model = reviewed_model();
    model.acknowledged = true;
    let original = model.save_request().unwrap();
    model.save_failed(failure(503));
    model.reading = true;
    let mut not_found = failure(404);
    not_found.code = storage_contract::ErrorCode::OperationNotFound;
    model.read_failed(not_found);
    assert_eq!(model.save, SaveState::Unknown);
    assert!(!model.reading);
    assert_eq!(model.review, Some(original.preview.clone()));
    assert_eq!(model.save_request(), Some(original));
}
#[test]
fn failed_revalidation_clears_a_previous_observation_match_without_discarding_the_review() {
    let mut model = reviewed_model();
    model.accept_saved(ForeignPlanStatus {
        preview: review(),
        validation: ForeignPlanValidation::MatchedAtObservation,
    });
    model.reading = true;
    model.read_failed(failure(409));
    assert_eq!(model.save, SaveState::Stored);
    assert_eq!(
        model.stored.as_ref().unwrap().validation,
        ForeignPlanValidation::Required
    );
    assert_eq!(model.review, Some(review()));
    assert!(!model.can_save());
}
#[test]
fn unknown_foreign_save_blocks_storage_input_changes_and_application_reload() {
    let mut state = State {
        authenticated: true,
        ..Default::default()
    };
    state.admin_console.storage.snapshot = Some(super::super::super::Snapshot {
        state: super::super::super::StorageState::Active,
        _disk_ids: vec![],
        completion_available: false,
        allow_development: false,
        terminal: None,
    });
    state.admin_console.storage.foreign.preview = Some(source());
    state.admin_console.storage.foreign.plan = reviewed_model();
    assert!(editable(&state));
    let model = &mut state.admin_console.storage.foreign.plan;
    model.acknowledged = true;
    model.save_request().unwrap();
    model.save_failed(failure(503));
    assert!(!editable(&state));
    assert!(state.admin_console.storage.reload_blocked());
    assert!(state.admin_console.reload_blocked());
}

#[test]
fn local_new_role_drafts_reject_an_alias_of_the_source_even_after_ctime_changes() {
    let source = source();
    for mode in [Mode::AdmitNewPrimary, Mode::AdmitSecondary] {
        let mut draft = draft(&source);
        draft.mode = mode;
        let mut alias = candidate("/mnt/source-alias", source.source.identity.inode);
        alias.root_identity.device = source.source.identity.device;
        alias.root_identity.ctime_seconds = "200".into();
        draft.empty_target = Some(alias);
        assert_eq!(
            draft.intent(&source, &[]),
            Err(DraftError::SourceTargetAlias)
        );
    }
}

#[test]
fn slot_lookup_uses_backend_case_rules_but_the_saved_review_identity_stays_exact() {
    let mut preview = review();
    preview.request.target.slot_id = "p09".into();
    let request = preview.request.clone();
    assert_eq!(
        read_preview(json!(preview), &request, &preview.source).unwrap(),
        preview
    );

    let mut wrong_target = preview.clone();
    wrong_target.target.identity.slot_id = "P10".into();
    assert!(read_preview(json!(wrong_target), &request, &preview.source).is_err());
    let mut changed_echo = preview.clone();
    changed_echo.request.target.slot_id = "P09".into();
    assert!(read_preview(json!(changed_echo), &request, &preview.source).is_err());

    let mut status = ForeignPlanStatus {
        preview: preview.clone(),
        validation: ForeignPlanValidation::Required,
    };
    status.preview.target.identity.slot_id = "p09".into();
    assert!(read_status(json!(status), &preview, ForeignPlanValidation::Required).is_err());

    let mut model = Model::default();
    let mut earlier = preview.clone();
    earlier.target.identity.slot_id = "p09".into();
    model.accept_preview(earlier);
    model.accept_preview(preview.clone());
    assert_eq!(model.known_primary.len(), 1);
    assert_eq!(model.known_primary[0].identity, preview.target.identity);
}
