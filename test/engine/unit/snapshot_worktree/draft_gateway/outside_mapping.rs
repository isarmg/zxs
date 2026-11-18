//! A source capture is not an SMB move, .zxs body or new FILE.
use super::{Fixture, fixture, write_draft};
use crate::engine::exposed::HeldOutsideFile;
use std::{io::Read, sync::Arc};

#[test]
fn mounting_source_reads_the_original_selected_name_and_creates_nothing() {
    let (fixture, mut engine) = Fixture::new();
    let target = &fixture.targets[0];
    let media = fixture.media();
    let cut = Arc::clone(&engine.state);
    let held = HeldOutsideFile::capture(&engine, target).unwrap();
    assert_eq!(held.facts().target, *target);
    assert_eq!(held.facts().selection, engine.file_draft_base(target).unwrap());
    assert_eq!(held.facts().logical_name.as_str(), target.path.file_name());
    assert!(matches!(held.facts().site, storage_contract::file_container::site::FileSite::Created {}));
    assert_eq!(fixture.media(), media);
    assert!(Arc::ptr_eq(&cut, &engine.state));

    // Copy permission is separate. The read must return the pinned FULL
    // bytes, not a directory listing, an uncommitted edit, or a storage name.
    let mut bytes = Vec::new();
    held.open_verified_for_copy(&engine).unwrap().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, fixture::FIRST_ORIGINAL);
    assert_eq!(fixture.media(), media);

    let wrong = storage_contract::file_draft::DraftLocation {
        file: fixture.targets[1].file, ..target.clone()
    };
    assert!(HeldOutsideFile::capture(&engine, &wrong).is_err());
    assert_eq!(fixture.media(), media);
    write_draft(&mut engine, target, fixture::FIRST_DRAFT);
    assert!(held.require_current(&engine).is_err());
    assert!(held.open_verified_for_copy(&engine).is_err());
    assert!(HeldOutsideFile::capture(&engine, target).is_err());
}

#[test]
fn matching_bytes_in_a_replaced_body_at_its_old_name_cannot_supply_the_mapping() {
    let (fixture, engine) = Fixture::new();
    let target = &fixture.targets[0];
    let held = HeldOutsideFile::capture(&engine, target).unwrap();
    let (source, _) = engine.valid_replica(&held.facts().selection.version_id().to_string()).unwrap();
    let original = source.path();
    let parked = fixture.temporary.path().join("parked-source-full");
    std::fs::rename(original, &parked).unwrap();
    std::fs::copy(&parked, original).unwrap();
    let after_swap = fixture.media();
    assert!(held.require_current(&engine).is_err());
    assert!(held.open_verified_for_copy(&engine).is_err());
    assert_eq!(fixture.media(), after_swap);
}

#[test]
fn renamed_container_cannot_change_the_committed_logical_name_of_its_old_node() {
    let (fixture, mut engine) = Fixture::new();
    let target = &fixture.targets[0];
    let old = HeldOutsideFile::capture(&engine, target).unwrap();
    let file = target.file;
    let exact = old.facts().selection.node_id();
    let old_name = old.facts().logical_name.clone();
    engine.file_namespace(&target.repository.to_string(), crate::NamespaceOperation::parse(
        crate::NamespaceOperationKind::Rename, target.path.as_str(), Some("Renamed outside.txt"),
    ).unwrap(), "admin", storage_contract::OperationId::generate().unwrap()).unwrap();
    assert!(old.require_current(&engine).is_err());
    assert_eq!(engine.state().file_containers[&file].namespace.name.as_str(), "Renamed outside.txt");
    assert_eq!(crate::business::file_nodes::committed_name(engine.state(), file, exact).unwrap(), old_name);
    assert!(crate::business::file_nodes::committed_name(engine.state(), fixture.targets[1].file, exact).is_err());
}

#[test]
fn projection_in_the_published_snapshot_keeps_its_own_bytes_and_original_name() {
    use crate::snapshot_worktree::copy_source::PublishedSnapshotFile;
    let (fixture, mut engine) = Fixture::new();
    let outside = &fixture.targets[0];
    let node = engine.file_draft_base(outside).unwrap().node_id();
    let saved = PublishedSnapshotFile::capture(&engine, fixture.source, &outside.path).unwrap();
    assert_eq!(saved.material().file_id, outside.file);
    assert_eq!(saved.occurrence(), &outside.path);
    assert_eq!(saved.handle(), fixture.source);
    assert!(saved.open(&saved.address().join("child")).is_err());
    write_draft(&mut engine, outside, fixture::FIRST_DRAFT);
    assert!(saved.require_current(&engine).is_err());

    // New authorized physical capture of the same published .zxs node. The
    // original FILE's later shared save is not its source of bytes.
    let existing = PublishedSnapshotFile::capture(&engine, fixture.source, &outside.path).unwrap();
    assert!(existing.require_current(&engine).is_ok());
    let media = fixture.media();
    let mut bytes = Vec::new();
    existing.open(existing.address()).unwrap().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, fixture::FIRST_ORIGINAL);
    assert_eq!(crate::business::file_nodes::recorded_name(engine.state(), outside.file, node)
        .unwrap().as_str(), "first.txt");
    assert!(PublishedSnapshotFile::capture(&engine, fixture.source,
        &name_policy::PortablePath::parse("First.txt").unwrap()).is_err());
    assert_eq!(fixture.media(), media);
}

#[test]
fn ordinary_copy_plans_a_new_parentless_file_and_never_moves_the_snapshot_or_source() {
    use crate::business::file_draft::independent_first::IndependentFirst;
    use crate::snapshot_worktree::copy_source::PublishedSnapshotFile;
    use storage_contract::file_draft::BirthRequest;
    let (fixture, mut engine) = Fixture::new();
    let outside = &fixture.targets[0];
    let original = Arc::new(PublishedSnapshotFile::capture(&engine, fixture.source, &outside.path).unwrap());
    let body = original.material();
    let repository = outside.repository;
    let repo = &engine.state().repositories[&repository.to_string()];
    let branch = repo.main_branch_id.clone();
    let destination = BirthRequest { repository, path: name_policy::PortablePath::parse("independent.txt").unwrap(),
        expected_namespace_head: engine.state().branches[&branch].head_commit_id.parse().unwrap() };
    let user = "admin".parse().unwrap();
    let operation = storage_contract::OperationId::generate().unwrap();
    let media = fixture.media();
    let before = Arc::clone(&engine.state);
    let occupied = BirthRequest { path: outside.path.clone(), ..destination.clone() };
    assert!(IndependentFirst::plan(&engine, Arc::clone(&original), &occupied, &user, operation).is_err());
    assert!(IndependentFirst::plan(&engine, Arc::clone(&original), &destination, &user,
        fixture.source.publication).is_err());
    let plan = IndependentFirst::plan(&engine, Arc::clone(&original), &destination, &user, operation).unwrap();
    let target = plan.target().clone();
    assert_ne!(target.file, outside.file);
    assert_ne!(target.entry, outside.entry);
    assert_eq!(target.path, destination.path);
    assert_eq!(plan.namespace().entry, target.entry);
    let version = plan.content().version.clone();
    assert_ne!(version.version_id, body.version_id.to_string());
    assert_eq!(version.file_id, target.file.to_string());
    assert_eq!(version.creation_name, destination.path.file_name());
    assert!(version.source_version_ids.is_empty());
    assert_eq!(version.logical_size, body.size);
    assert_eq!(version.content_hash, body.content_hash);
    let (next, physical) = plan.into_parts(&engine).unwrap();
    assert_eq!(next.versions[&version.version_id], version);
    assert!(!next.file_node_parents.contains_key(&version.version_id));
    assert_eq!(next.file_containers.get(&outside.file), before.file_containers.get(&outside.file));
    assert!(!next.file_containers.contains_key(&target.file));
    assert_eq!(physical.version, version);
    let mut bytes = Vec::new();
    physical.source.open().unwrap().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, fixture::FIRST_ORIGINAL);
    assert!(Arc::ptr_eq(&engine.state, &before));
    assert_eq!(fixture.media(), media);

    // An unrelated genuine save moves the preparation's state cut. The
    // previous source remains intact and the new FILE is not published.
    let pending = IndependentFirst::plan(&engine, original, &destination, &user, operation).unwrap();
    write_draft(&mut engine, &fixture.targets[1], fixture::SECOND_DRAFT);
    assert!(pending.into_parts(&engine).is_err());
    assert!(!engine.state().operations.contains_key(&operation));
    assert!(!engine.state().file_containers.contains_key(&target.file));
}

#[test]
fn authenticated_selection_n1_is_not_advanced_when_the_source_file_publishes_n2() {
    use storage_contract::file_draft::DraftCommand;
    let (fixture, mut engine) = Fixture::new();
    let target = &fixture.targets[0];
    let n1 = HeldOutsideFile::capture(&engine, target).unwrap();
    let witness = n1.witness().unwrap();
    let before = Arc::clone(&engine.state);
    let original = engine.snapshot_receipt_source().unwrap();
    let issued = original.index().unwrap();
    issued.require_outside_selection_at_issue(&before, &engine.config, &witness).unwrap();
    let equal_but_unrelated = before.as_ref().clone();
    assert!(issued.require_outside_selection_at_issue(&equal_but_unrelated, &engine.config, &witness).is_err());
    let encoded = json_codec::to_vec(&witness).unwrap();
    type Witness = crate::persist::snapshot_worktree::outside_selection::OutsideSelectionWitness;
    let decoded: Witness = json_codec::from_slice(&encoded).unwrap();
    assert_eq!(decoded, witness); // Descriptive only. No read accepts decoded.
    let mut invalid = json_codec::to_value(&witness).unwrap();
    invalid["unexpected"] = json_codec::json!(true);
    assert!(json_codec::from_value::<Witness>(invalid).is_err());
    let mut replaced = json_codec::to_value(&witness).unwrap();
    replaced["body"] = replaced["body_parent"].clone();
    assert!(json_codec::from_value::<Witness>(replaced).is_err());
    let other = HeldOutsideFile::capture(&engine, &fixture.targets[1]).unwrap().witness().unwrap();
    let old_fields = json_codec::to_value(&witness).unwrap();
    let other_fields = json_codec::to_value(&other).unwrap();
    // Both real selected FILEs share the same genuine storage root. Splicing
    // its sibling's otherwise admissible content or FILE container into
    // a witness cannot borrow that sibling's source selection.
    for key in ["hash", "logical_name", "container", "container_preparation"] {
        assert_ne!(old_fields[key], other_fields[key]);
        let mut changed = old_fields.clone();
        changed[key] = other_fields[key].clone();
        let substitution: Witness = json_codec::from_value(changed).unwrap();
        assert!(issued.require_outside_selection_at_issue(&before, &engine.config, &substitution).is_err());
    }
    write_draft(&mut engine, target, fixture::FIRST_DRAFT);
    let mut old = Vec::new();
    n1.open_pinned_node(&engine).unwrap().read_to_end(&mut old).unwrap();
    assert_eq!(old, fixture::FIRST_ORIGINAL);
    engine.change_file_working(&DraftCommand::Commit {
        target: target.clone(), expected: engine.file_working_state(target).unwrap().revision(),
        base: witness.fields().selection, message: "genuine later unselected N2".into(),
    }, &"admin".parse().unwrap(), storage_contract::OperationId::generate().unwrap()).unwrap();
    let n2 = HeldOutsideFile::capture(&engine, target).unwrap();
    assert_ne!(n2.facts().selection.node_id(), witness.fields().selection.node_id());
    // N1 remains a valid *old* mapping, but cannot be newly issued at N2.
    let now_index = engine.snapshot_receipts().unwrap();
    assert!(now_index.require_outside_selection_at_issue(engine.state(), &engine.config, &witness).is_err());
    now_index.require_outside_selection_at_issue(engine.state(), &engine.config, &n2.witness().unwrap()).unwrap();
    issued.require_outside_selection_at_issue(&before, &engine.config, &witness).unwrap();
    let media = fixture.media();
    old.clear();
    n1.open_pinned_node(&engine).unwrap().read_to_end(&mut old).unwrap();
    let mut now = Vec::new();
    n2.open_verified_for_copy(&engine).unwrap().read_to_end(&mut now).unwrap();
    assert_eq!(old, fixture::FIRST_ORIGINAL);
    assert_eq!(now, fixture::FIRST_DRAFT);
    assert_eq!(fixture.media(), media);
}
