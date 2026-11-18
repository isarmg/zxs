use super::*;

#[test]
fn original_nonzero_system_descriptor_uses_all_rows_of_its_actual_state_and_configuration() {
    let tmp = tempfile::tempdir().unwrap();
    let config = crate::Config::development([(
        name_policy::Name::parse("Primary").unwrap(), tmp.path().join("disk"),
    )]).unwrap();
    let primary = config.disks[0].slot_id.clone();
    let mut engine = crate::Engine::initialize(&tmp.path().join("system"), config).unwrap();
    let before = AdmittedState::admit_replayed(engine.state().clone()).unwrap();
    engine.create_disk_root(&primary, "alice", storage_contract::OperationId::generate().unwrap()).unwrap();
    let after = AdmittedState::admit_replayed(engine.state().clone()).unwrap();
    assert!(after.state().sequence > before.state().sequence);
    let pool = engine.planning_context().config.pool_id.parse().unwrap();
    let config_a = MaterialCommitment::from_digest(blake3::hash(b"configuration a"));
    let config_b = MaterialCommitment::from_digest(blake3::hash(b"configuration b"));
    let empty = OrdinalBuilder::new(0).finish().unwrap();
    let original = FrozenSystem::original(pool, config_a, empty.root(), &after, &|| Ok(())).unwrap();
    let rebound = FrozenSystem::original(pool, config_b, empty.root(), &after, &|| Ok(())).unwrap();
    assert_ne!(original.root(), rebound.root());
    let input = SystemEncodingInput::new(&after, &|| Ok(())).unwrap();
    assert!(input.verified_scope_root(original.bytes(), &|| Ok(())).is_ok());
    let prior = SystemEncodingInput::new(&before, &|| Ok(())).unwrap();
    assert!(prior.verified_scope_root(original.bytes(), &|| Ok(())).is_err());
}
