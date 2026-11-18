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
