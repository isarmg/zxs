use super::*;
use crate::{Config, Engine};
use name_policy::{Name, PortablePath};
use storage_contract::{Mode, Principal, file_draft::BirthRequest};

#[test]
fn packed_working_quota_is_bound_to_one_state_and_charges_retained_plus_replacement() {
    const DATA_BYTES: u64 = 2 << 20;
    let one = physical_bytes_for(DATA_BYTES).unwrap();
    let cap = one.checked_mul(2).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let mut config = Config::independent_roots(Mode::Development,
        [(Name::parse("Primary").unwrap(), temp.path().join("primary"))],
    ).unwrap();
    config.disks[0].capacity_bytes = Some(cap);
    let slot = config.disks[0].slot_id.clone();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let repo = engine.create_disk_root(&slot, "admin", OperationId::generate().unwrap()).unwrap();
    let principal: Principal = "admin".parse().unwrap();
    let birth = BirthRequest {
        repository: repo.repository_id.parse().unwrap(),
        path: PortablePath::parse("packed.txt").unwrap(),
        expected_namespace_head: engine.state().branches[&repo.main_branch_id].head_commit_id.parse().unwrap(),
    };
    let born = engine.create_unborn_file(&birth, &principal, OperationId::generate().unwrap()).unwrap();
    let target = crate::persist::file_draft::birth::decode(&born).unwrap().target().clone();

    let mut stale = engine.begin_unborn_write(&target, OperationId::generate().unwrap()).unwrap();
    let mut first = engine.begin_unborn_write(&target, OperationId::generate().unwrap()).unwrap();
    assert_eq!(first.configured_usage.as_ref().unwrap().used, 0);
    for frame in 0..DATA_BYTES / WORKING_IO_BYTES {
        first.write_at(&engine.planning_context(), frame * WORKING_IO_BYTES,
            &[0xa1; WORKING_IO_BYTES as usize]).unwrap();
    }
    let prepared = Arc::new(first.prepare(&engine.planning_context()).unwrap().unwrap());
    let original = prepared.request(&target).unwrap();
    let receipt = engine.save_unborn_working(&original, &principal, Some(prepared)).unwrap();
    assert_eq!(engine.planning_context().used_bytes(&slot), one);
    assert!(matches!(stale.write_at(&engine.planning_context(), 0, b"x"),
        Err(Error::FileDraft(Failure::SourceChanged))));

    let second_id = OperationId::generate().unwrap();
    let mut second = engine.begin_unborn_write(&target, second_id).unwrap();
    assert_eq!(second.configured_usage.as_ref().unwrap().used, one);
    // Overwrite A completely. Its old pack is still retained during B, even
    // though none of A will appear in B's prospective logical range table.
    for frame in 0..DATA_BYTES / WORKING_IO_BYTES {
        second.write_at(&engine.planning_context(), frame * WORKING_IO_BYTES,
            &[0xb2; WORKING_IO_BYTES as usize]).unwrap();
    }
    // A+B including BOTH physical trailers fits exactly. Another overwriting
    // byte adds private data (and a checksum); it is not free logical reuse.
    assert!(matches!(second.write_at(&engine.planning_context(), 0, b"x"),
        Err(Error::FileDraft(Failure::WorkingCapacity))));
    let prepared = second.prepare(&engine.planning_context()).unwrap().unwrap();
    assert_eq!(prepared.manifest().packs().len(), 1);
    assert_eq!(prepared.manifest().packs()[0].physical_bytes(), one);
    assert_ne!(prepared.manifest().packs()[0].id(), original.manifest.packs()[0].id());
    assert_eq!(engine.save_unborn_working(&original, &principal, None).unwrap(), receipt);
    assert!(!engine.state().operations.contains_key(&second_id));
}

#[test]
fn packed_configured_peak_rejects_overflow_and_preexisting_excess_even_for_an_empty_stage() {
    let quota = ConfiguredUsage { used: u64::MAX - 1, limit: u64::MAX };
    assert!(!quota.exceeds(1));
    assert!(quota.exceeds(2));
    assert!(ConfiguredUsage { used: 4, limit: 3 }.exceeds(0));
}
