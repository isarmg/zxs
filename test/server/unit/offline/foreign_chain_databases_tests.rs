use super::*;
use json_codec::json;
fn report(level: u32) -> ForeignOriginalDatabasesReport {
    json_codec::from_value(json!({"original":{"pool_id":"pool_source","configuration_hash":"a".repeat(64),
        "registrations":[],"sequence":"2","snapshot_hash":"b".repeat(64),"state_hash":"c".repeat(64),
        "prefix_hash":"d".repeat(64),"publications":2,"operations":2,"principals":2,"events":2},
        "capsule_digest":"e".repeat(64),"chain_digest":"f".repeat(64),"target_level":level,
        "manifest_hash":"a".repeat(64),"original_disks":3,"file_databases":1,"directory_databases":0,
        "databases":8,"database_bytes":"4096"})).unwrap()
}
fn input() -> Original {
    Original {
        service_user: "zxs".into(),
        operation_id: "op_11111111111111111111111111111111".parse().unwrap(),
        request_digest: "a".repeat(64),
        external_writers_stopped: false,
    }
}
#[test]
fn all_lower_database_scopes_require_exact_verified_reports_not_progress_flags() {
    let a = report(1);
    let b = report(2);
    let mut observation = Observation {
        targets: vec![a.clone(), b.clone()],
        statuses: vec![
            ForeignDatabaseStatus::Verified { report: a },
            ForeignDatabaseStatus::Preparing,
        ],
    };
    assert!(!observation.completed());
    observation.statuses[1] = ForeignDatabaseStatus::Verified { report: b };
    assert!(observation.completed());
    observation.targets[1].original.state_hash = "0".repeat(64);
    assert!(!observation.completed());
}
#[test]
fn database_confirmation_and_stop_ack_precede_system_access() {
    assert!(
        preview(input())
            .unwrap_err()
            .to_string()
            .contains("stop ZXS")
    );
    let expected = Expected {
        request_digest: "a".repeat(64),
        targets: vec![report(1)],
    };
    let request = Prepare {
        input: input(),
        database_digest: digest(&expected).unwrap(),
        expected,
        confirmation: "admit-foreign-now".into(),
    };
    assert!(
        prepare(request)
            .unwrap_err()
            .to_string()
            .contains("before system access")
    );
}
#[test]
fn original_database_completion_never_claims_new_identity_or_membership() {
    let target = report(1);
    let expected = Expected {
        request_digest: "a".repeat(64),
        targets: vec![target.clone()],
    };
    let observation = Observation {
        targets: vec![target.clone()],
        statuses: vec![ForeignDatabaseStatus::Verified { report: target }],
    };
    let value = response(&input(), expected, observation, true).unwrap();
    assert_eq!(value["original_database_scopes_prepared"], true);
    for key in [
        "target_identity_databases_installed",
        "foreign_admitted",
        "changes_existing_pool_membership",
        "starts_service",
        "complete_installation_ready",
    ] {
        assert_eq!(value[key], false);
    }
}
