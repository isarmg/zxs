use super::*;
use storage_engine::{ErrorCode, ForeignVersionAvailability};

fn fixture() -> Fixture {
    let authority = tempfile::tempdir().unwrap();
    crate::fixture::initialize_management(&authority, "Foreign fixture password 2026!").unwrap();
    // Fixture owns its own TempDir, not a child of the short-lived authority.
    Fixture::new(&std::env::temp_dir(), &authority.path().join("pool")).unwrap()
}

#[test]
fn real_tertiary_fixture_retains_two_origins_and_unavailable_history() {
    let f = fixture();
    let preview = f.inspect().unwrap();
    assert_eq!(preview.source_level(), 3);
    assert_eq!(preview.source().registration().slot_id, "C01");
    assert_eq!(preview.histories().len(), 2);
    let origins = preview
        .histories()
        .iter()
        .map(|history| history.origin().clone())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(origins.len(), 2);
    let available = preview
        .histories()
        .iter()
        .flat_map(|history| history.availability().values())
        .copied()
        .collect::<Vec<_>>();
    assert!(available.contains(&ForeignVersionAvailability::VerifiedLocal));
    assert!(available.contains(&ForeignVersionAvailability::TemporarilyUnavailable));
    assert!(!available.contains(&ForeignVersionAvailability::RecordedPurged));
    assert!(!available.contains(&ForeignVersionAvailability::RecordedMigrated));
    assert!(!preview.checkpoint().has_unpublished_tail());
    assert_eq!(preview.checkpoint().publication_hash().len(), 64);
    drop(preview);
    let reopened = f.inspect().unwrap();
    assert_eq!(reopened.histories().len(), 2);
    f.source_unchanged().unwrap();
}

#[test]
fn real_review_maps_every_history_without_touching_either_data_disk() {
    let f = fixture();
    let preview = f.inspect().unwrap();
    let engine = Engine::open(&f.pool).unwrap();
    let before = snapshot(&f.target).unwrap();
    let parent = engine
        .list_tree(&f.repository.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.path() == PARENT)
        .unwrap()
        .path;
    let mapping = preview
        .map_to_primary(&engine, "p01", Some(&parent), &f.names(&preview))
        .unwrap();
    assert_eq!(mapping.target().slot_id, "P01");
    assert_eq!(mapping.target_sequence(), engine.state().sequence);
    assert_eq!(mapping.histories().len(), preview.histories().len());
    for history in preview.histories() {
        let mapped = mapping
            .histories()
            .iter()
            .find(|item| item.group_id() == history.group_id())
            .unwrap();
        assert_eq!(
            mapped.history_tree_id(),
            history.manifest().history.history_tree_id
        );
        assert!(mapped.path().as_str().starts_with("Reviewed/origin-"));
    }
    assert_eq!(snapshot(&f.target).unwrap(), before);
    f.source_unchanged().unwrap();
}

#[test]
fn real_review_rejects_nonempty_target_incomplete_and_colliding_names_without_writes() {
    let f = fixture();
    let preview = f.inspect().unwrap();
    let engine = Engine::open(&f.pool).unwrap();
    let before = snapshot(&f.target).unwrap();
    let parent = engine
        .list_tree(&f.repository.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.path() == PARENT)
        .unwrap()
        .path;
    let names = f.names(&preview);
    let error = preview
        .map_to_primary(&engine, "P01", None, &names)
        .err()
        .unwrap();
    assert_eq!(error.code(), ErrorCode::DirectoryNotEmpty);
    let mut incomplete = names.clone();
    incomplete.pop_first();
    assert_eq!(
        preview
            .map_to_primary(&engine, "P01", Some(&parent), &incomplete)
            .err()
            .unwrap()
            .code(),
        ErrorCode::HistoryPackageInvalid
    );
    let mut collisions = names;
    for (index, value) in collisions.values_mut().enumerate() {
        *value = if index == 0 { "Same" } else { "same" }.into();
    }
    assert_eq!(
        preview
            .map_to_primary(&engine, "P01", Some(&parent), &collisions)
            .err()
            .unwrap()
            .code(),
        ErrorCode::NameCollision
    );
    assert_eq!(snapshot(&f.target).unwrap(), before);
    f.source_unchanged().unwrap();
}

#[test]
fn disk_snapshot_detects_empty_directory_and_same_bytes_inode_replacement() {
    let root = tempfile::tempdir().unwrap();
    let original = root.path().join("original");
    fs::write(&original, b"same bytes").unwrap();
    let before = snapshot(root.path()).unwrap();
    fs::create_dir(root.path().join("unexpected-empty")).unwrap();
    assert_ne!(snapshot(root.path()).unwrap(), before);
    fs::remove_dir(root.path().join("unexpected-empty")).unwrap();
    fs::rename(&original, root.path().join("retained-original")).unwrap();
    fs::write(&original, b"same bytes").unwrap();
    let replaced = snapshot(root.path()).unwrap();
    assert_eq!(
        replaced[Path::new("original")].body,
        before[Path::new("original")].body
    );
    assert_ne!(
        replaced[Path::new("original")].inode,
        before[Path::new("original")].inode
    );
    assert_ne!(replaced, before);
}

#[test]
fn real_target_namespace_publication_changes_mapping_without_touching_source() {
    let f = fixture();
    let source = f.inspect().unwrap();
    let mut engine = Engine::open(&f.pool).unwrap();
    let parent = engine
        .list_tree(&f.repository.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.path() == PARENT)
        .unwrap()
        .path;
    let names = f.names(&source);
    let original = source
        .map_to_primary(&engine, "P01", Some(&parent), &names)
        .unwrap();
    let before = snapshot(&f.target).unwrap();
    let operation = OperationId::generate().unwrap();
    let publication = engine
        .file_namespace(
            &f.repository.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "OtherActor", None).unwrap(),
            PRINCIPAL,
            operation,
        )
        .unwrap();
    assert_eq!(publication.operation_id, operation);
    assert_ne!(snapshot(&f.target).unwrap(), before);
    let changed = source
        .map_to_primary(&engine, "P01", Some(&parent), &names)
        .unwrap();
    assert!(changed.target_sequence() > original.target_sequence());
    assert_ne!(changed.digest(), original.digest());
    assert_eq!(changed.target(), original.target());
    f.source_unchanged().unwrap();
}
