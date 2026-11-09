//! The ordinary and rescue readers reject the same complete local material.
//! All disk mutations below are isolated temporary fixtures, never live disks.
use super::*;
use data_serialization::Serialize;
use sqlite_access::{
    Connection, Executor, Row, SqliteConnection,
    sqlite::{SqliteConnectOptions, SqliteOwnedBuf},
};
use storage_engine::{
    DiskDatabaseReadScope, ObjectDatabaseScope, inspect_disk_database_material,
    inspect_object_database_material,
};

fn rewrite(path: &Path, statements: &[&'static str]) -> Vec<u8> {
    let original = fs::read(path).unwrap();
    let bytes = platform_io::sqlite::block_on_sqlite_connection(async {
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
        for statement in statements {
            connection.execute(*statement).await.unwrap();
        }
        let bytes = connection.serialize(None).await.unwrap().to_vec();
        connection.close().await.unwrap();
        bytes
    });
    fs::write(path, &bytes).unwrap();
    bytes
}

#[derive(Serialize)]
struct Record {
    kind: String,
    id: String,
    body: String,
}

fn rehash_directory(path: &Path, edit: impl FnOnce(&mut Vec<Record>)) -> Vec<u8> {
    let original = fs::read(path).unwrap();
    let bytes = platform_io::sqlite::block_on_sqlite_connection(async {
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
        let rows = sqlite_access::query("SELECT kind,id,body FROM records ORDER BY kind,id")
            .fetch_all(&mut connection)
            .await
            .unwrap();
        let mut records = rows
            .into_iter()
            .map(|row| Record {
                kind: row.try_get(0).unwrap(),
                id: row.try_get(1).unwrap(),
                body: row.try_get(2).unwrap(),
            })
            .collect::<Vec<_>>();
        edit(&mut records);
        connection.execute("DELETE FROM records").await.unwrap();
        for row in &records {
            sqlite_access::query("INSERT INTO records VALUES(?,?,?)")
                .bind(&row.kind)
                .bind(&row.id)
                .bind(&row.body)
                .execute(&mut connection)
                .await
                .unwrap();
        }
        let digest = storage_contract::object_hash("directory-local-records", &records).unwrap();
        sqlite_access::query("UPDATE identity SET records_hash=?")
            .bind(digest)
            .execute(&mut connection)
            .await
            .unwrap();
        let bytes = connection.serialize(None).await.unwrap().to_vec();
        connection.close().await.unwrap();
        bytes
    });
    fs::write(path, &bytes).unwrap();
    bytes
}

fn object_fixture() -> (Fixture, [(PathBuf, ObjectDatabaseScope, &'static str); 2]) {
    let mut fixture = Fixture::new();
    let file = fixture.save("box/文本.txt", b"retained body");
    fixture.initialize("");
    let directory = fixture.initialize("box");
    fixture.checkpoint("box");
    fixture.create_branch("box", "topic");
    fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id)
        .unwrap();
    let paths = container_paths(&fixture.root());
    let objects = [
        (
            paths[&file.file_id][0].clone(),
            ObjectDatabaseScope::File {
                file_id: file.file_id,
                history_tree_id: fixture.repository.history_tree_id.clone(),
            },
            ".zxs-file.sqlite",
        ),
        (
            fixture.root().join("box"),
            ObjectDatabaseScope::Directory {
                repository_id: fixture.repository.repository_id.clone(),
                directory_id: directory.directory_id,
            },
            ".zxs-directory.sqlite",
        ),
    ];
    (fixture, objects)
}

#[test]
fn valid_directory_closure_survives_ordinary_reconcile_and_cold_rescue() {
    let (fixture, objects) = object_fixture();
    let before = objects.each_ref().map(|(root, scope, _)| {
        inspect_object_database_material(fs::File::open(root).unwrap(), root.clone(), scope)
            .unwrap()
            .archive_bytes()
            .unwrap()
    });
    let mut fixture = fixture.reopen();
    fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id)
        .unwrap();
    for ((root, scope, _), expected) in objects.iter().zip(before) {
        let actual =
            inspect_object_database_material(fs::File::open(root).unwrap(), root.clone(), scope)
                .unwrap();
        assert_eq!(actual.archive_bytes().unwrap(), expected);
    }
}

#[test]
fn ordinary_directory_reconcile_preserves_rehashed_invalid_local_closure() {
    for case in 0..5 {
        let (mut fixture, objects) = object_fixture();
        let (root, scope, leaf) = &objects[1];
        let path = root.join(leaf);
        let unknown = rehash_directory(&path, |rows| match case {
            0 => rows.retain(|row| row.kind != "tree"),
            1 => rows.retain(|row| row.kind != "repository"),
            _ => {
                let kind = if case == 3 { "branch" } else { "tree" };
                let row = rows.iter_mut().find(|row| row.kind == kind).unwrap();
                let mut body: json_codec::Value = json_codec::from_str(&row.body).unwrap();
                match case {
                    2 => body["path"] = json_codec::json!("outside/root"),
                    3 => body["parent_branch_id"] = json_codec::json!(row.id.clone()),
                    4 => body["unknown_location"] = json_codec::json!("/not/local"),
                    _ => unreachable!(),
                }
                row.body =
                    String::from_utf8(storage_engine::canonical_json(&body).unwrap()).unwrap();
            }
        });
        let state = storage_engine::canonical_json(fixture.engine.state()).unwrap();
        assert!(
            fixture
                .engine
                .reconcile_directory_layout(&fixture.repository.repository_id)
                .is_err(),
            "case {case} was accepted merely because its self-hash matched"
        );
        assert!(
            inspect_object_database_material(fs::File::open(root).unwrap(), root.clone(), scope)
                .is_err()
        );
        assert_eq!(fs::read(path).unwrap(), unknown);
        assert_eq!(
            storage_engine::canonical_json(fixture.engine.state()).unwrap(),
            state
        );
        assert!(!fixture.engine.is_read_only());
    }
}

#[test]
fn object_fixed_headers_preserve_nul_tails_and_reject_long_values() {
    let (_fixture, objects) = object_fixture();
    for (root, scope, leaf) in objects {
        let path = root.join(leaf);
        let original = fs::read(&path).unwrap();
        for sql in [
            "UPDATE identity SET format=format||char(0)||'tail'",
            "UPDATE identity SET records_hash=records_hash||char(0)||'tail'",
            "UPDATE identity SET source_sequence=source_sequence||char(0)||'tail'",
            "UPDATE identity SET format=CAST(zeroblob(131072) AS TEXT)",
        ] {
            fs::write(&path, &original).unwrap();
            let unknown = rewrite(&path, &[sql]);
            assert!(
                inspect_object_database_material(
                    fs::File::open(&root).unwrap(),
                    root.clone(),
                    &scope
                )
                .is_err()
            );
            assert_eq!(fs::read(&path).unwrap(), unknown);
        }
        fs::write(path, original).unwrap();
    }
}

#[test]
fn object_record_budget_is_checked_before_invalid_utf8_body_is_copied() {
    let (_fixture, objects) = object_fixture();
    for (root, scope, leaf) in objects {
        let path = root.join(leaf);
        let unknown = rewrite(
            &path,
            &[
                "UPDATE records SET body=CAST(x'80'||zeroblob(12582912) AS TEXT) WHERE (kind,id)=(SELECT kind,id FROM records ORDER BY kind,id LIMIT 1)",
            ],
        );
        assert!(unknown.len() <= 16 * 1024 * 1024);
        let error =
            match inspect_object_database_material(fs::File::open(&root).unwrap(), root, &scope) {
                Ok(_) => panic!("oversized object record was accepted"),
                Err(error) => error,
            };
        assert_eq!(error.code(), ErrorCode::ResourceLimit);
        assert_eq!(fs::read(path).unwrap(), unknown);
    }
}

#[test]
fn object_schema_ddl_with_nul_tail_is_not_a_supported_prefix() {
    let (_fixture, objects) = object_fixture();
    for (root, scope, leaf) in objects {
        let path = root.join(leaf);
        let unknown = rewrite(
            &path,
            &[
                "PRAGMA writable_schema=ON",
                "UPDATE sqlite_schema SET sql=sql||char(0)||'unknown tail' WHERE name='records'",
                "PRAGMA writable_schema=OFF",
            ],
        );
        assert!(
            inspect_object_database_material(fs::File::open(&root).unwrap(), root, &scope).is_err()
        );
        assert_eq!(fs::read(path).unwrap(), unknown);
    }
}

#[test]
fn object_record_keys_are_not_truncated_before_local_identity_validation() {
    let (_fixture, objects) = object_fixture();
    for (root, scope, leaf) in objects {
        let path = root.join(leaf);
        let original = fs::read(&path).unwrap();
        for sql in [
            "UPDATE records SET id=id||char(0)||'tail' WHERE (kind,id)=(SELECT kind,id FROM records ORDER BY kind,id LIMIT 1)",
            "UPDATE records SET id=CAST(zeroblob(131072) AS TEXT) WHERE (kind,id)=(SELECT kind,id FROM records ORDER BY kind,id LIMIT 1)",
        ] {
            fs::write(&path, &original).unwrap();
            let unknown = rewrite(&path, &[sql]);
            assert!(
                inspect_object_database_material(
                    fs::File::open(&root).unwrap(),
                    root.clone(),
                    &scope,
                )
                .is_err()
            );
            assert_eq!(fs::read(&path).unwrap(), unknown);
        }
        fs::write(path, original).unwrap();
    }
}

#[test]
fn disk_headers_reject_nul_format_and_digest_without_touching_other_selected_family() {
    let mut fixture = Fixture::new();
    fixture.save("file.txt", b"retained");
    fixture.engine.rebuild_disk_catalog("P01").unwrap();
    fixture.engine.rebuild_disk_history("P01").unwrap();
    let identity = fixture
        .engine
        .registered_disks()
        .find(|disk| disk.slot_id == "P01")
        .unwrap()
        .identity();
    for (leaf, selected, other) in [
        (
            ".zxs-disk.sqlite",
            DiskDatabaseReadScope::Current,
            DiskDatabaseReadScope::History,
        ),
        (
            ".zxs-disk-history.sqlite",
            DiskDatabaseReadScope::History,
            DiskDatabaseReadScope::Current,
        ),
    ] {
        let path = fixture.primary.join(leaf);
        let original = fs::read(&path).unwrap();
        for sql in [
            "UPDATE identity SET format=format||char(0)||'tail'",
            "UPDATE identity SET records_hash=records_hash||char(0)||'tail'",
            "UPDATE identity SET format=CAST(zeroblob(131072) AS TEXT)",
        ] {
            fs::write(&path, &original).unwrap();
            let unknown = rewrite(&path, &[sql]);
            for scope in [selected, DiskDatabaseReadScope::Both] {
                assert!(
                    inspect_disk_database_material(
                        fs::File::open(&fixture.primary).unwrap(),
                        fixture.primary.clone(),
                        &identity,
                        scope,
                    )
                    .is_err()
                );
            }
            inspect_disk_database_material(
                fs::File::open(&fixture.primary).unwrap(),
                fixture.primary.clone(),
                &identity,
                other,
            )
            .unwrap();
            assert_eq!(fs::read(&path).unwrap(), unknown);
        }
        fs::write(path, original).unwrap();
    }
}
