//! The physical boundary must cover an entire mixed directory, not merely fit
//! one first FILE. Both rejections use normal planning, not injected failures.
use super::*;
use name_policy::PortablePath;
use storage_contract::OperationId;
use storage_contract::file_draft::{BirthRequest, DraftLocation, WorkingExpectation};

fn first_selection(engine: &Engine, target: &DraftLocation) -> WorkingSelection {
    WorkingSelection {
        target: target.clone(),
        expected: WorkingExpectation::from_working(&engine.state().file_containers[&target.file].working).unwrap(),
    }
}

#[test]
fn first_full_waits_for_the_native_envelope_and_prospective_metadata() {
    let (fixture, mut engine) = Fixture::new();
    let principal = "admin".parse().unwrap();
    let repository = fixture.targets[0].repository;
    let mut targets = Vec::new();
    for name in ["new-one", "new-two"] {
        let root = &engine.state().repositories[&repository.to_string()];
        let head = &engine.state().branches[&root.main_branch_id].head_commit_id;
        let birth = engine.create_unborn_file(&BirthRequest {
            repository, path: PortablePath::parse(name).unwrap(),
            expected_namespace_head: head.parse().unwrap(),
        }, &principal, OperationId::generate().unwrap()).unwrap();
        let target = crate::persist::file_draft::birth::decode(&birth).unwrap().target().clone();
        targets.push(target);
    }
    // One independent empty FULL, one discontiguous actual body including
    // zeros. A namespace-wide matching name or constant digest is insufficient.
    let writer_op = OperationId::generate().unwrap();
    let mut writer = engine.begin_unborn_write(&targets[1], writer_op).unwrap();
    writer.write_at(&engine.planning_context(), 0, b"a").unwrap();
    let tail = 65_541u64;
    writer.write_at(&engine.planning_context(), tail, b"b").unwrap();
    let saved = Arc::new(writer.prepare(&engine.planning_context()).unwrap().unwrap());
    let request = saved.request(&targets[1]).unwrap();
    engine.save_unborn_working(&request, &principal, Some(saved)).unwrap();
    let selected = targets.iter().map(|target| first_selection(&engine, target)).collect::<Vec<_>>();
    let before = fixture.media();
    let state = Arc::clone(engine.planning_context().state);
    let slot = engine.planning_context().next_slot;
    for (envelope_limit, metadata_limit) in [
        (1, crate::disk::MAX_METADATA), (crate::disk::MAX_METADATA, 1),
    ] {
        let pending = PendingDirectoryDrafts::capture(&engine, fixture.intent(&selected, &principal)).unwrap();
        let directory = pending.directory();
        let context = engine.planning_context();
        let batch = PreparedWorkingBatch::prepare(
            &context, Arc::clone(pending.selected()), directory.operation(),
            &principal, directory.message(),
        ).unwrap();
        assert_eq!(batch.first_measurements().count(), 2);
        assert!(batch.prepared().is_empty(), "measuring does not mint first-file ReadSources");
        let mut expected = vec![0u8; tail as usize + 1];
        expected[0] = b'a'; expected[tail as usize] = b'b';
        assert_eq!(batch.items()[1].version().content_hash,
            storage_contract::ContentHash::from_digest(blake3::hash(&expected)));
        assert_eq!(fixture.media(), before, "no working FULL before the envelope");
        let plan = pending.bind_working_batch(
            &context, batch, SnapshotNodeId::generate().unwrap(),
        ).unwrap();
        let input = plan.into_publication(&context, envelope_limit);
        if envelope_limit == 1 {
            assert!(input.is_err());
        } else {
            assert!(context.prepare_directory_material(input.unwrap(), metadata_limit).is_err());
        }
        assert_eq!(fixture.media(), before, "a rejected budget leaves no orphan FULL anywhere");
        assert!(Arc::ptr_eq(&state, engine.planning_context().state));
        assert_eq!(slot, engine.planning_context().next_slot);
    }
    let original = fixture.intent(&selected, &principal);
    let operation = original.operation;
    let source = original.source;
    let message = original.message;
    let result = engine.commit_directory_working_set(original).unwrap();
    for target in targets {
        assert!(engine.state().file_containers[&target.file].working.unborn().is_none());
    }
    // Permanent exact retry is still answered from the completed decision,
    // not by recreating either old independent working FULL.
    let after = fixture.media();
    assert_eq!(engine.commit_directory_working_set(DirectoryDraftIntent {
        source, selected: &selected, operation, principal: &principal, message,
    }).unwrap(), result);
    assert_eq!(fixture.media(), after);
}

#[test]
fn two_individually_fitting_first_files_cannot_spend_the_same_owner_capacity() {
    // Each sparse source occupies only one working block. A new independent
    // working FULL, its immutable version, and the independent directory copy
    // must all be costed at logical FULL size on this same owner.
    const SIZE: u64 = 2 << 20;
    const CAPACITY: u64 = 9 << 20;
    let (fixture, mut engine) = Fixture::configured(false, Some(CAPACITY));
    let principal = "admin".parse().unwrap();
    let repository = fixture.targets[0].repository;
    let mut selected = Vec::new();
    for name in ["large-a", "large-b"] {
        let root = &engine.state().repositories[&repository.to_string()];
        let head = engine.state().branches[&root.main_branch_id].head_commit_id.parse().unwrap();
        let birth = engine.create_unborn_file(&BirthRequest {
            repository, path: PortablePath::parse(name).unwrap(), expected_namespace_head: head,
        }, &principal, OperationId::generate().unwrap()).unwrap();
        let target = crate::persist::file_draft::birth::decode(&birth).unwrap().target().clone();
        let mut writer = engine.begin_unborn_write(&target, OperationId::generate().unwrap()).unwrap();
        writer.write_at(&engine.planning_context(), SIZE - 1, b"x").unwrap();
        let saved = Arc::new(writer.prepare(&engine.planning_context()).unwrap().unwrap());
        let request = saved.request(&target).unwrap();
        engine.save_unborn_working(&request, &principal, Some(saved)).unwrap();
        selected.push(first_selection(&engine, &target));
    }
    let owner = &engine.config.disks[0];
    let remaining = CAPACITY.saturating_sub(engine.planning_context().used_bytes(&owner.slot_id));
    assert!(SIZE * 2 <= remaining && SIZE * 6 > remaining);
    let before = fixture.media();
    let original = Arc::clone(engine.planning_context().state);
    let slot = engine.planning_context().next_slot;
    let intent = fixture.intent(&selected, &principal);
    let operation = intent.operation;
    let error = engine.commit_directory_working_set(intent).unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::NoSpace);
    assert_eq!(fixture.media(), before);
    assert!(Arc::ptr_eq(&original, engine.planning_context().state));
    assert_eq!(engine.planning_context().next_slot, slot);
    assert!(!engine.state().operations.contains_key(&operation));
}
