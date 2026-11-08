//! Source-only regression cases for the original-binding receipt boundary.
use super::*;
use json_codec::json;

fn version() -> HistoryVersionId { HistoryVersionId::parse(&format!("ver_{}", "a".repeat(32))).unwrap() }
fn root() -> HistoryRoot { HistoryRoot::parse(&"b".repeat(64)).unwrap() }
fn receipt(bindings: json_codec::Value) -> json_codec::Value {
    json!({"result":{"retirement":{"cleanup_routes":{"bindings":bindings}}}})
}
fn replica(slot: &str, generation: u64, original: bool, status: ReplicaState) -> Replica {
    Replica { version_id: version(), slot_id: slot.into(), binding_generation: generation, original_requirement: original, status }
}
fn required(slot: &str, generation: u64) -> json_codec::Value {
    json!({"version_id":version(),"slot_id":slot,"binding_generation":generation.to_string()})
}
#[test]
fn retirement_reply_cannot_omit_an_original_offline_copy() {
    let observed = Cleanup::Observed {
        history_root: root(), scope: CleanupScope::OriginalAndCurrentRoutes,
        replicas: vec![replica("P01", 1, true, ReplicaState::VerifiedAbsent)],
    };
    let receipt = receipt(json!([required("B01", 1), required("P01", 1)]));
    assert!(observed.validate_for_receipt(&[version()], &receipt).is_err());
}
#[test]
fn replacing_a_slot_generation_does_not_discharge_the_original_binding() {
    let observed = Cleanup::Observed {
        history_root: root(), scope: CleanupScope::OriginalAndCurrentRoutes,
        replicas: vec![replica("P01", 2, true, ReplicaState::VerifiedAbsent)],
    };
    assert!(observed.validate_for_receipt(&[version()], &receipt(json!([required("P01", 1)]))).is_err());
}
#[test]
fn original_pending_and_new_current_generation_remain_distinct_rows() {
    let observed = Cleanup::Observed {
        history_root: root(), scope: CleanupScope::OriginalAndCurrentRoutes,
        replicas: vec![
            replica("P01", 1, true, ReplicaState::Unverified { error_code: "DISK_IDENTITY_MISMATCH".into() }),
            replica("P01", 2, false, ReplicaState::VerifiedAbsent),
        ],
    };
    observed.validate_for_receipt(&[version()], &receipt(json!([required("P01", 1)]))).unwrap();
    assert!(!observed.all_required_bindings_absent());
}
#[test]
fn current_only_observation_is_not_an_original_obligation_completion() {
    let observed = Cleanup::Observed {
        history_root: root(), scope: CleanupScope::CurrentRoutes,
        replicas: vec![replica("P01", 1, false, ReplicaState::VerifiedAbsent)],
    };
    assert!(observed.validate_for_receipt(&[version()], &receipt(json!([required("P01", 1)]))).is_err());
    assert!(!observed.all_required_bindings_absent());
}

#[test]
fn termination_observation_must_match_the_exact_original_receipt() {
    let file = HistoryFileId::parse(&format!("file_{}", "c".repeat(32))).unwrap();
    let selection = Selection::new(file.clone(), root(), vec![version()]).unwrap();
    let separation = Separation::Published {
        history_root: root(), original_files: vec![file.clone()], terminated_files: vec![file.clone()],
        file_components: vec![vec![file.clone()]],
    };
    let exact = json!({"result":{"retirement":{"file_terminations":{"files":[{"file_id":file,"version_ids":[version()]}]}}}});
    separation.validate_for_receipt(&selection, &exact).unwrap();
    assert!(separation.validate_for_receipt(&selection, &json!({"result":{"retirement":{"file_terminations":{"files":[]}}}})).is_err());
    assert!(separation.validate_for_receipt(&selection, &json!({"result":{"retirement":{}}})).is_err());
}
