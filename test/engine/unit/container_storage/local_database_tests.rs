use super::super::{directory_database, file_database};
use super::*;
use sqlite_access::Connection;
use sqlite_access::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteOwnedBuf};

fn records(schema: &Schema) -> Vec<Record> {
    let mut records = schema.records();
    records.add(schema.kinds[0], "b", &2).unwrap();
    records.add(schema.kinds[0], "a", &1).unwrap();
    records.finish()
}

// Original encoder retained only as a format oracle, outside production code.
fn original_encode(schema: &Schema) -> Vec<u8> {
    crate::sqlite_connection::run(
        SqliteConnectOptions::new()
            .in_memory(true)
            .journal_mode(SqliteJournalMode::Memory),
        async |connection| {
            connection.execute("PRAGMA page_size=4096").await?;
            connection.execute("PRAGMA max_page_count=4096").await?;
            let mut tx = connection.begin().await?;
            tx.execute(schema.identity).await?;
            tx.execute(schema.records).await?;
            sqlite_access::query("INSERT INTO identity VALUES(1,?,?,?,?,?)")
                .bind(schema.format)
                .bind("owner")
                .bind("scope")
                .bind("7")
                .bind("records-digest")
                .execute(&mut *tx)
                .await?;
            for record in records(schema) {
                sqlite_access::query("INSERT INTO records VALUES(?,?,?)")
                    .bind(record.kind)
                    .bind(record.id)
                    .bind(record.body)
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await?;
            Ok(connection.serialize(None).await?.to_vec())
        },
    )
    .unwrap()
}

#[test]
fn both_local_encoders_keep_their_original_sqlite_bytes_and_record_fields() {
    for schema in [&file_database::SCHEMA, &directory_database::SCHEMA] {
        let prepared = schema
            .encode(records(schema), ["owner", "scope"], 7, "records-digest")
            .unwrap();
        assert_eq!(prepared.bytes, original_encode(schema));
        assert_eq!(
            json_codec::to_value(&records(schema)[0]).unwrap(),
            json_codec::json!({
                "kind": schema.kinds[0], "id": "a", "body": "1"
            })
        );
    }
}

#[test]
fn bounded_reader_keeps_sorted_rows_and_stops_at_the_domain_error() {
    for schema in [&file_database::SCHEMA, &directory_database::SCHEMA] {
        let bytes = original_encode(schema);
        crate::sqlite_connection::run(
            SqliteConnectOptions::new().in_memory(true),
            async |connection| {
                connection
                    .deserialize(None, SqliteOwnedBuf::try_from(bytes.as_slice())?, true)
                    .await?;
                schema.verify_schema(connection).await?;
                let values = schema.read_records(connection, |_| Ok(())).await?;
                assert_eq!(
                    values.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
                    ["a", "b"]
                );
                let mut visited = Vec::new();
                let error = schema
                    .read_records(connection, |record| {
                        visited.push(record.id.clone());
                        Err(Error::new(
                            ErrorCode::PublicationInvalid,
                            "original row error",
                        ))
                    })
                    .await
                    .err()
                    .unwrap();
                assert_eq!(visited, ["a"]);
                assert_eq!(error.code(), ErrorCode::PublicationInvalid);
                assert_eq!(error.message(), "original row error");
                Ok(())
            },
        )
        .unwrap();
    }
}

#[test]
fn schema_and_record_rejections_keep_each_original_database_name() {
    for schema in [&file_database::SCHEMA, &directory_database::SCHEMA] {
        let bytes = original_encode(schema);
        crate::sqlite_connection::run(
            SqliteConnectOptions::new().in_memory(true),
            async |connection| {
                connection
                    .deserialize(None, SqliteOwnedBuf::try_from(bytes.as_slice())?, false)
                    .await?;
                connection
                    .execute("CREATE VIEW extra AS SELECT id FROM records")
                    .await?;
                let error = schema.verify_schema(connection).await.unwrap_err();
                assert_eq!(
                    error.message(),
                    format!("{} database has unexpected schema objects", schema.name)
                );
                connection.execute("DROP VIEW extra").await?;
                connection
                    .execute("PRAGMA ignore_check_constraints=ON")
                    .await?;
                connection
                    .execute("UPDATE records SET kind='unsupported'")
                    .await?;
                connection
                    .execute("PRAGMA ignore_check_constraints=OFF")
                    .await?;
                let error = schema
                    .read_records(connection, |_| Ok(()))
                    .await
                    .err()
                    .unwrap();
                assert_eq!(
                    error.message(),
                    format!("{} database record kind is unsupported", schema.name)
                );
                Ok(())
            },
        )
        .unwrap();
    }
}

#[test]
fn projection_budget_includes_record_identity_bytes() {
    for schema in [&file_database::SCHEMA, &directory_database::SCHEMA] {
        let mut records = schema.records();
        records.bytes = MAX_RECORD_BYTES - 1;
        let error = records.add(schema.kinds[0], "id", &0).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceLimit);
        assert_eq!(error.message(), schema.budget().message());
        assert!(records.rows.is_empty());
    }
}
