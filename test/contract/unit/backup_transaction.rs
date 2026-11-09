use super::*;
use json_codec::json;

#[test]
fn each_irreversible_command_requires_its_own_exact_confirmation() {
    let commands = [
        ("fence_source", "stop_all_original_pool_writes"),
        (
            "decide_registry",
            "select_original_irreversible_registry_decision",
        ),
        ("activate", "activate_only_the_original_published_backup"),
        (
            "resume_original_task",
            "recover_only_the_original_retained_task",
        ),
    ];
    for (action, confirmation) in commands {
        assert!(
            json_codec::from_value::<Command>(json!({"action":action,"confirmation":confirmation}))
                .is_ok()
        );
        assert!(json_codec::from_value::<Command>(json!({"action":action})).is_err());
        assert!(
            json_codec::from_value::<Command>(json!({"action":action,"confirmation":true}))
                .is_err()
        );
        assert!(
            json_codec::from_value::<Command>(json!({"action":action,"confirmation":"yes"}))
                .is_err()
        );
    }
}

#[test]
fn plain_continuation_cannot_smuggle_a_principal_path_or_decision() {
    let plain = json!({"action":"continue_preparation"});
    assert_eq!(
        json_codec::from_value::<Command>(plain.clone()).unwrap(),
        Command::ContinuePreparation {}
    );
    for (key, value) in [
        ("principal", json!("other")),
        ("target_paths", json!(["/elsewhere"])),
        ("confirmation", json!("stop_all_original_pool_writes")),
    ] {
        let mut altered = plain.clone();
        altered[key] = value;
        assert!(json_codec::from_value::<Command>(altered).is_err());
    }
}

#[test]
fn duplicate_action_and_unknown_mutation_are_rejected() {
    assert!(
        crate::decode_strict::<Command>(
            br#"{"action":"continue_preparation","action":"activate"}"#
        )
        .is_err()
    );
    assert!(crate::decode_strict::<Command>(br#"{"action":"backup.configure"}"#).is_err());
}

#[test]
fn start_has_no_client_principal_and_sequence_is_exact_decimal_text() {
    let mut request = json!({
        "operation_id":"op_11111111111111111111111111111111",
        "source_slot_id":"P01", "expected_sequence":"18446744073709551615",
        "targets":[], "confirmation":"prepare_independent_complete_copies"
    });
    let decoded: StartRequest = json_codec::from_value(request.clone()).unwrap();
    assert_eq!(decoded.expected_sequence, u64::MAX);
    request["principal"] = json!("other");
    assert!(json_codec::from_value::<StartRequest>(request.clone()).is_err());
    request.as_object_mut().unwrap().remove("principal");
    request["expected_sequence"] = json!(7);
    assert!(json_codec::from_value::<StartRequest>(request).is_err());
}

#[test]
fn progress_preserves_full_width_counts_as_text() {
    let progress = Progress {
        observation: Observation::Preparing,
        stage: "GenerationFirstCandidatePublished".into(),
        completed_files: u64::MAX,
        total_files: u64::MAX,
        completed_bytes: u64::MAX,
        total_bytes: u64::MAX,
        error_code: None,
        detail: "not serving".into(),
    };
    let value = json_codec::to_value(&progress).unwrap();
    assert_eq!(value["completed_bytes"], u64::MAX.to_string());
    assert_eq!(json_codec::from_value::<Progress>(value).unwrap(), progress);
}

#[test]
fn queued_recovery_is_not_a_snapshot_or_a_receipt() {
    let operation_id = "op_11111111111111111111111111111111".parse().unwrap();
    let queued = CommandResponse::RecoveryQueued { operation_id };
    let value = json_codec::to_value(&queued).unwrap();
    assert_eq!(
        value,
        json!({"state":"recovery_queued","operation_id":operation_id})
    );
    assert!(json_codec::from_value::<Snapshot>(value.clone()).is_err());
    assert_eq!(
        json_codec::from_value::<CommandResponse>(value).unwrap(),
        queued
    );
}
