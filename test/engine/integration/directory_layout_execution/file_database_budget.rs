//! Both database consumers contribute to the same pre-decision allowance.
use super::*;

#[test]
fn file_and_directory_databases_share_one_history_retention_budget() {
    let mut fixture = Fixture::new();
    let file = fixture.save("file.txt", b"retained bytes");
    fixture.initialize("");
    fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id)
        .unwrap();
    let root = fixture.root();
    let directory_db = root.join(".zxs-directory.sqlite");
    let file_db = root.join("file.txt/.zxs-file.sqlite");
    let directory_bytes = fs::read(&directory_db).unwrap();
    let file_bytes = fs::read(&file_db).unwrap();
    let retained = fixture
        .primary
        .join("histories")
        .join(&fixture.repository.history_tree_id)
        .join(".zxs-layout/database-installs");
    let used = file_snapshot(&retained)
        .values()
        .map(|bytes| bytes.len() as u64)
        .sum::<u64>();
    // Short note-only edits fit the existing SQLite pages. Either consumer's
    // exact reservation fits independently, but their combined reservation does
    // not. No enormous SQLite/State fixture or fake reservation API is used.
    let remaining = directory_bytes.len().max(file_bytes.len()) as u64 + 8192;
    let marker = retained.join("preserved-quota-material");
    let marker_file = fs::File::create(&marker).unwrap();
    marker_file
        .set_len(256 * 1024 * 1024 - used - remaining)
        .unwrap();
    marker_file.sync_all().unwrap();
    let operation = op();
    let sequence = fixture.engine.state().sequence;
    let result = fixture.engine.set_file_note(
        &file.file_id,
        None,
        &storage_engine::Note::parse("x".into()).unwrap(),
        "admin",
        operation,
    );
    assert_eq!(result.unwrap_err().code(), ErrorCode::ResourceLimit);
    assert_eq!(fixture.engine.state().sequence, sequence);
    assert!(!fixture.engine.state().operations.contains_key(&operation));
    assert_eq!(fs::read(&file_db).unwrap(), file_bytes);
    assert_eq!(fs::read(&directory_db).unwrap(), directory_bytes);
    assert_eq!(
        fs::metadata(&marker).unwrap().len(),
        256 * 1024 * 1024 - used - remaining
    );
}
