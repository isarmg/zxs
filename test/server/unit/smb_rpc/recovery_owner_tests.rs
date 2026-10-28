use super::*;

#[test]
fn record_bytes_are_checked_even_when_the_saved_stamp_matches() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = PrivateDirectory::create(temporary.path().join("spool")).unwrap();
    let name = EntryName::new("record").unwrap();
    let mut owner = ActiveRecord::default();
    owner
        .persist(&directory, &name, b"expected".to_vec())
        .unwrap();
    fs::write(directory.path().join(name.as_path()), b"corrupt!").unwrap();
    // Model damage invisible to the stamp: only the byte check can now reject
    // reuse. This is not a claim to emulate physical storage corruption.
    let saved = owner.persisted.as_mut().unwrap();
    saved.stamp = Stamp::read(&saved.file).unwrap();
    let start = persist::replacements();
    owner
        .persist(&directory, &name, b"expected".to_vec())
        .unwrap();
    assert_eq!(persist::replacements() - start, 1);
    assert_eq!(directory.read_bounded(&name, 8).unwrap(), b"expected");
}

#[test]
fn cache_bounds_do_not_reject_larger_records_or_accumulate_generations() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = PrivateDirectory::create(temporary.path().join("spool")).unwrap();
    let name = EntryName::new("record").unwrap();
    let mut owner = ActiveRecord::default();
    for size in [MAX_CACHED_BYTES, MAX_CACHED_BYTES + 1, 8] {
        let bytes = vec![b'x'; size];
        let start = persist::replacements();
        owner.persist(&directory, &name, bytes.clone()).unwrap();
        owner.persist(&directory, &name, bytes).unwrap();
        assert_eq!(
            persist::replacements() - start,
            if size > MAX_CACHED_BYTES { 2 } else { 1 }
        );
        assert_eq!(owner.persisted.is_some(), size <= MAX_CACHED_BYTES);
        if let Some(saved) = &owner.persisted {
            assert_eq!(saved.bytes.len(), size);
        }
    }
}

#[test]
fn failed_atomic_completion_leaves_no_reusable_owner() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = PrivateDirectory::create(temporary.path().join("spool")).unwrap();
    let name = EntryName::new("record").unwrap();
    let mut owner = ActiveRecord::default();
    persist::fail_after_replace();
    assert!(
        owner
            .persist(&directory, &name, b"expected".to_vec())
            .is_err()
    );
    assert!(owner.persisted.is_none());
    assert_eq!(directory.read_bounded(&name, 8).unwrap(), b"expected");
    let start = persist::replacements();
    owner
        .persist(&directory, &name, b"expected".to_vec())
        .unwrap();
    assert_eq!(persist::replacements() - start, 1);
}
