//! A damaged index must not turn retained rows into a deceptively valid subset.
use super::*;
use sqlite_access::{
    Connection, SqliteConnection,
    sqlite::{SqliteConnectOptions, SqliteOwnedBuf},
};
use storage_engine::{
    DiskDatabaseReadScope, ObjectDatabaseScope, inspect_disk_database_material,
    inspect_object_database_material,
};

fn hide_last_index_row(path: &Path, original_id_query: &str, insert: &str, index: &str) -> Vec<u8> {
    let original = fs::read(path).unwrap();
    let (original_id, hidden_id, page, mut bytes) =
        platform_io::sqlite::block_on_sqlite_connection(async {
            let mut connection =
                SqliteConnection::connect_with(&SqliteConnectOptions::new().in_memory(true))
                    .await
                    .unwrap();
            connection
                .deserialize(
                    None,
                    SqliteOwnedBuf::try_from(original.as_slice()).unwrap(),
                    false,
                )
                .await
                .unwrap();
            let original_id: String = sqlite_access::query_scalar(original_id_query)
                .fetch_one(&mut connection)
                .await
                .unwrap();
            let hidden_id = "z".repeat(original_id.len());
            sqlite_access::query(insert)
                .bind(&hidden_id)
                .execute(&mut connection)
                .await
                .unwrap();
            let page: i64 =
                sqlite_access::query_scalar("SELECT rootpage FROM sqlite_schema WHERE name=?")
                    .bind(index)
                    .fetch_one(&mut connection)
                    .await
                    .unwrap();
            let bytes = connection.serialize(None).await.unwrap().to_vec();
            connection.close().await.unwrap();
            (
                original_id,
                hidden_id,
                usize::try_from(page).unwrap(),
                bytes,
            )
        });
    let page_size = usize::from(u16::from_be_bytes([bytes[16], bytes[17]]));
    assert_eq!(page_size, 4096);
    let header = (page - 1) * page_size + if page == 1 { 100 } else { 0 };
    assert_eq!(bytes[header], 0x0a, "small fixture uses one index leaf");
    let cells = u16::from_be_bytes([bytes[header + 3], bytes[header + 4]]);
    assert!(cells > 0);
    let pointer = header + 8 + 2 * usize::from(cells - 1);
    let last = u16::from_be_bytes([bytes[pointer], bytes[pointer + 1]]);
    let cell = (page - 1) * page_size + usize::from(last);
    let payload_size = usize::from(bytes[cell]);
    assert!(
        payload_size < 128,
        "fixture index payload uses one-byte length"
    );
    let payload = &mut bytes[cell + 1..cell + 1 + payload_size];
    let matches: Vec<_> = payload
        .windows(hidden_id.len())
        .enumerate()
        .filter_map(|(offset, bytes)| (bytes == hidden_id.as_bytes()).then_some(offset))
        .collect();
    assert_eq!(matches.len(), 1);
    let offset = matches[0];
    // Keep the physical entry count and B-tree ordering, but give the new row
    // a duplicate index key. Keyset reads skip it; quick_check also reports ok.
    // Its larger rowid remains intact, so the original row is still first.
    payload[offset..offset + hidden_id.len()].copy_from_slice(original_id.as_bytes());
    fs::write(path, &bytes).unwrap();
    bytes
}

fn corrupt_fragment_count(path: &Path, table: &str) -> Vec<u8> {
    let mut bytes = fs::read(path).unwrap();
    let page = platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection =
            SqliteConnection::connect_with(&SqliteConnectOptions::new().in_memory(true))
                .await
                .unwrap();
        connection
            .deserialize(
                None,
                SqliteOwnedBuf::try_from(bytes.as_slice()).unwrap(),
                true,
            )
            .await
            .unwrap();
        let page: i64 =
            sqlite_access::query_scalar("SELECT rootpage FROM sqlite_schema WHERE name=?")
                .bind(table)
                .fetch_one(&mut connection)
                .await
                .unwrap();
        connection.close().await.unwrap();
        usize::try_from(page).unwrap()
    });
    let page_size = usize::from(u16::from_be_bytes([bytes[16], bytes[17]]));
    assert_eq!(page_size, 4096);
    let header = (page - 1) * page_size + if page == 1 { 100 } else { 0 };
    assert_eq!(
        bytes[header], 0x0d,
        "small history fixture uses one table leaf"
    );
    assert!(bytes[header + 7] < 60);
    bytes[header + 7] += 1;
    fs::write(path, &bytes).unwrap();
    bytes
}

fn assert_integrity_failure<T>(result: Result<T, storage_engine::Error>) {
    let error = match result {
        Ok(_) => panic!("physically corrupt SQLite was accepted as complete material"),
        Err(error) => error,
    };
    assert_eq!(error.code(), ErrorCode::ContainerLayoutInvalid);
    assert!(error.to_string().contains("physical integrity"), "{error}");
}

fn assert_physical_corruption(bytes: &[u8], quick_check_passes: bool) {
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection =
            SqliteConnection::connect_with(&SqliteConnectOptions::new().in_memory(true))
                .await
                .unwrap();
        connection
            .deserialize(None, SqliteOwnedBuf::try_from(bytes).unwrap(), true)
            .await
            .unwrap();
        if quick_check_passes {
            let report: String = sqlite_access::query_scalar("PRAGMA quick_check(1)")
                .fetch_one(&mut connection)
                .await
                .unwrap();
            assert_eq!(
                report, "ok",
                "index/table mismatch is invisible to quick_check"
            );
        }
        let report: String = sqlite_access::query_scalar("PRAGMA integrity_check(1)")
            .fetch_one(&mut connection)
            .await
            .unwrap();
        assert_ne!(report, "ok");
        connection.close().await.unwrap();
    });
}

#[test]
fn directory_rescue_rejects_index_hidden_rows_and_preserves_original_bytes() {
    let mut fixture = Fixture::new();
    fixture.save("file.txt", b"retained");
    fixture.initialize("");
    fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id)
        .unwrap();
    let root = fixture.root();
    let scope = ObjectDatabaseScope::Directory {
        repository_id: fixture.repository.repository_id.clone(),
        directory_id: fixture.repository.repository_id.clone(),
    };
    inspect_object_database_material(fs::File::open(&root).unwrap(), root.clone(), &scope).unwrap();
    let path = root.join(".zxs-directory.sqlite");
    let damaged = hide_last_index_row(
        &path,
        "SELECT id FROM records WHERE kind='tree' ORDER BY id DESC LIMIT 1",
        "INSERT INTO records VALUES('tree',?,'{\"retained\":true}')",
        "sqlite_autoindex_records_1",
    );
    assert_physical_corruption(&damaged, true);
    assert_integrity_failure(inspect_object_database_material(
        fs::File::open(&root).unwrap(),
        root,
        &scope,
    ));
    assert_eq!(fs::read(path).unwrap(), damaged);
}

#[test]
fn file_rescue_rejects_index_hidden_rows_and_preserves_original_bytes() {
    let mut fixture = Fixture::new();
    let entry = fixture.save("file.txt", b"retained");
    fixture.initialize("");
    fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id)
        .unwrap();
    let paths = container_paths(&fixture.root());
    let root = paths[&entry.file_id][0].clone();
    let scope = ObjectDatabaseScope::File {
        file_id: entry.file_id,
        history_tree_id: fixture.repository.history_tree_id.clone(),
    };
    inspect_object_database_material(fs::File::open(&root).unwrap(), root.clone(), &scope).unwrap();
    let path = root.join(".zxs-file.sqlite");
    let damaged = hide_last_index_row(
        &path,
        "SELECT id FROM records WHERE kind='version' ORDER BY id DESC LIMIT 1",
        "INSERT INTO records VALUES('version',?,'{\"retained\":true}')",
        "sqlite_autoindex_records_1",
    );
    assert_physical_corruption(&damaged, true);
    assert_integrity_failure(inspect_object_database_material(
        fs::File::open(&root).unwrap(),
        root,
        &scope,
    ));
    assert_eq!(fs::read(path).unwrap(), damaged);
}

#[test]
fn disk_rescue_rejects_index_hidden_inventory_but_allows_explicit_other_family() {
    let mut fixture = Fixture::new();
    fixture.save("file.txt", b"retained");
    fixture.engine.rebuild_disk_catalog("P01").unwrap();
    fixture.engine.rebuild_disk_history("P01").unwrap();
    let identity = fixture
        .engine
        .registered_disks()
        .find(|row| row.slot_id == "P01")
        .unwrap()
        .identity();
    let root = fixture.primary.clone();
    let path = root.join(".zxs-disk.sqlite");
    let damaged = hide_last_index_row(
        &path,
        "SELECT version_id FROM versions ORDER BY version_id DESC LIMIT 1",
        "INSERT INTO versions VALUES(?,'{\"retained\":true}')",
        "sqlite_autoindex_versions_1",
    );
    assert_physical_corruption(&damaged, true);
    for scope in [DiskDatabaseReadScope::Current, DiskDatabaseReadScope::Both] {
        assert_integrity_failure(inspect_disk_database_material(
            fs::File::open(&root).unwrap(),
            root.clone(),
            &identity,
            scope,
        ));
    }
    inspect_disk_database_material(
        fs::File::open(&root).unwrap(),
        root,
        &identity,
        DiskDatabaseReadScope::History,
    )
    .unwrap();
    assert_eq!(fs::read(path).unwrap(), damaged);
}

#[test]
fn history_rescue_rejects_page_damage_but_allows_explicit_other_family() {
    let mut fixture = Fixture::new();
    fixture.save("file.txt", b"retained");
    fixture.engine.rebuild_disk_catalog("P01").unwrap();
    fixture.engine.rebuild_disk_history("P01").unwrap();
    let identity = fixture
        .engine
        .registered_disks()
        .find(|row| row.slot_id == "P01")
        .unwrap()
        .identity();
    let root = fixture.primary.clone();
    let path = root.join(".zxs-disk-history.sqlite");
    let damaged = corrupt_fragment_count(&path, "changes");
    assert_physical_corruption(&damaged, false);
    for scope in [DiskDatabaseReadScope::History, DiskDatabaseReadScope::Both] {
        assert_integrity_failure(inspect_disk_database_material(
            fs::File::open(&root).unwrap(),
            root.clone(),
            &identity,
            scope,
        ));
    }
    inspect_disk_database_material(
        fs::File::open(&root).unwrap(),
        root,
        &identity,
        DiskDatabaseReadScope::Current,
    )
    .unwrap();
    assert_eq!(fs::read(path).unwrap(), damaged);
}

#[test]
fn directory_reconciliation_does_not_replace_index_hidden_material() {
    let mut fixture = Fixture::new();
    fixture.save("file.txt", b"retained");
    fixture.initialize("");
    fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id)
        .unwrap();
    let path = fixture.root().join(".zxs-directory.sqlite");
    let damaged = hide_last_index_row(
        &path,
        "SELECT id FROM records WHERE kind='tree' ORDER BY id DESC LIMIT 1",
        "INSERT INTO records VALUES('tree',?,'{\"retained\":true}')",
        "sqlite_autoindex_records_1",
    );
    assert_physical_corruption(&damaged, true);
    let sequence = fixture.engine.state().sequence;
    assert_integrity_failure(
        fixture
            .engine
            .reconcile_directory_layout(&fixture.repository.repository_id),
    );
    assert_eq!(fixture.engine.state().sequence, sequence);
    assert_eq!(fs::read(path).unwrap(), damaged);
}
