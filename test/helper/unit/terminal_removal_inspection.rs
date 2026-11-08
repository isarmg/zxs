use super::*;
use storage_contract::storage_readmission::RegistrationMode;
fn fixture() -> (
    TerminalBoundary,
    storage_contract::Config,
    Vec<RegistrationEntry>,
) {
    let config = storage_contract::Config::development([
        Path::new("/zxs/data/disks/first").to_owned(),
        Path::new("/zxs/data/disks/second").to_owned(),
    ])
    .unwrap();
    let registrations = config
        .disks
        .iter()
        .map(|disk| RegistrationEntry {
            slot_id: disk.slot_id.clone(),
            disk_id: disk.disk_id.clone(),
            binding_generation: disk.binding_generation,
            metadata_member: false,
            mode: RegistrationMode::Enabled,
            readmission: None,
        })
        .collect();
    let registry = json_codec::from_value(
        json_codec::json!({"root_hash":"a".repeat(64),"view_generation":"3"}),
    )
    .unwrap();
    (
        TerminalBoundary {
            operation_id: OperationId::generate().unwrap(),
            authority_hash: "b".repeat(64),
            registry,
            sequence: 12,
            topology_generation: 5,
            pending_retirement: None,
        },
        config,
        registrations,
    )
}
#[test]
fn terminal_report_is_bound_metadata_not_an_executable_plan_or_receipt() {
    let (boundary, config, registrations) = fixture();
    let target = registrations[0].clone();
    let report = inspect(
        boundary,
        config,
        registrations,
        &target.slot_id,
        &target.disk_id,
    )
    .unwrap();
    assert_eq!(report.selection.current_registered_directories, 2);
    assert_eq!(report.selection.proposed_remaining_directories, 1);
    assert_eq!(report.selection.target, target);
    assert!(!report.executable);
    assert!(!report.operation_created);
    assert!(!report.target_io_probed);
    assert!(!report.physical_bytes_erased);
    assert_eq!(report.inspection_digest.len(), 64);
    let wire = json_codec::to_value(report).unwrap();
    assert_eq!(wire["blocker"], "terminal_registry_successor_unavailable");
    assert!(wire.get("operation_id").is_none());
    assert!(wire.get("request_digest").is_none());
    assert!(wire.get("plan").is_none());
}
#[test]
fn terminal_inspection_refuses_stale_registration_and_active_voter() {
    let (boundary, config, mut registrations) = fixture();
    let target = registrations[0].clone();
    registrations[0].binding_generation += 1;
    assert!(
        inspect(
            boundary,
            config,
            registrations,
            &target.slot_id,
            &target.disk_id
        )
        .is_err()
    );
    let (boundary, config, mut registrations) = fixture();
    let target = registrations[0].clone();
    registrations[1].metadata_member = true;
    assert!(
        inspect(
            boundary,
            config,
            registrations,
            &target.slot_id,
            &target.disk_id
        )
        .is_err()
    );
}
#[test]
fn terminal_inspection_accepts_no_force_or_operation_and_needs_stop_ack_before_io() {
    let mut body = json_codec::json!({"service_user":"zxs","slot_id":"P02","expected_disk_id":"disk_original",
        "external_writers_stopped":false});
    let input: Input = json_codec::from_value(body.clone()).unwrap();
    assert!(run(input).is_err());
    body["operation_id"] = "op_11111111111111111111111111111111".into();
    assert!(json_codec::from_value::<Input>(body).is_err());
}
