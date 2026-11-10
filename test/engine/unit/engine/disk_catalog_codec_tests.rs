use super::*;
use crate::sqlite_connection::run;
use sqlite_access::{Executor, sqlite::SqliteConnectOptions};

fn visit(
    table: &OrdinalTable,
    rows: &[(i64, &str)],
    limit: usize,
    total: &mut usize,
    admit: impl FnMut(Option<&u64>, &u64) -> Result<()>,
) -> Result<Vec<u64>> {
    run(
        SqliteConnectOptions::new().in_memory(true),
        async |connection| {
            connection
                .execute("CREATE TABLE changes(ordinal INTEGER PRIMARY KEY,body TEXT NOT NULL)")
                .await?;
            connection.execute("CREATE TABLE backup_directories(ordinal INTEGER PRIMARY KEY,body TEXT NOT NULL)").await?;
            for &(ordinal, body) in rows {
                for sql in [
                    "INSERT INTO changes VALUES(?,?)",
                    "INSERT INTO backup_directories VALUES(?,?)",
                ] {
                    sqlite_access::query(sql)
                        .bind(ordinal)
                        .bind(body)
                        .execute(&mut *connection)
                        .await?;
                }
            }
            table.read(connection, limit, total, admit).await
        },
    )
}

#[test]
fn both_ordinal_tables_share_ordered_bounded_admission() {
    for table in [&DIRECTORY_ROWS, &HISTORY_ROWS] {
        let mut total = 5;
        let mut observed = Vec::new();
        let values = visit(table, &[(0, "7"), (1, "12")], 2, &mut total, |old, new| {
            observed.push((old.copied(), *new));
            Ok(())
        })
        .unwrap();
        assert_eq!(values, [7, 12]);
        assert_eq!(observed, [(None, 7), (Some(7), 12)]);
        assert_eq!(total, 8);
        assert!(
            visit(table, &[], 0, &mut total, |_, _| Ok(()))
                .unwrap()
                .is_empty()
        );
        assert_eq!(total, 8);
    }
}

#[test]
fn negative_first_keys_gaps_and_noncanonical_json_are_rejected() {
    for table in [&DIRECTORY_ROWS, &HISTORY_ROWS] {
        for rows in [
            vec![(-1, "1")],
            vec![(1, "1")],
            vec![(0, "1"), (2, "2")],
            vec![(0, " 1")],
        ] {
            let error = visit(table, &rows, 2, &mut 0, |_, _| Ok(())).unwrap_err();
            assert_eq!(error.code(), ErrorCode::ContainerLayoutInvalid, "{rows:?}");
        }
    }
}

#[test]
fn row_and_shared_byte_limits_precede_domain_admission() {
    for table in [&DIRECTORY_ROWS, &HISTORY_ROWS] {
        let mut total = MAX_RECORD_BYTES - 1;
        let mut visits = 0;
        let error = visit(table, &[(0, "12")], 1, &mut total, |_, _| {
            visits += 1;
            Ok(())
        })
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceLimit);
        assert_eq!(total, MAX_RECORD_BYTES - 1);
        assert_eq!(visits, 0);
        for (limit, expected_visits) in [(0, 0), (1, 1)] {
            visits = 0;
            let error = visit(table, &[(0, "1"), (1, "2")], limit, &mut 0, |_, _| {
                visits += 1;
                Ok(())
            })
            .unwrap_err();
            assert_eq!(error.code(), ErrorCode::ResourceLimit);
            assert_eq!(visits, expected_visits);
        }
    }
}

#[test]
fn domain_failure_preserves_its_error_and_the_last_admitted_byte_count() {
    for table in [&DIRECTORY_ROWS, &HISTORY_ROWS] {
        let mut total = 4;
        let error = visit(table, &[(0, "1"), (1, "2")], 2, &mut total, |old, _| {
            if old.is_some() {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "domain row failure",
                ));
            }
            Ok(())
        })
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::PublicationInvalid);
        assert_eq!(error.message(), "domain row failure");
        assert_eq!(total, 5);
    }
}
