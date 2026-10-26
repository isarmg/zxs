use super::*;
use crate::schema_identity::SchemaRow;
use crate::sqlite::MAX_SCHEMA_OBJECTS;

#[test]
fn fingerprints_canonical_schema_inside_and_outside_tokio() -> Result<(), Box<dyn std::error::Error>>
{
    let ddl = [
        "CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT NOT NULL)",
        "CREATE INDEX items_value ON items(value)",
        crate::schema_identity::PRODUCT_METADATA_DDL,
    ];
    let expected = schema_fingerprint(&[
        SchemaRow::new(
            "index",
            "items_value",
            "items",
            "CREATE INDEX items_value ON items(value)",
        ),
        SchemaRow::new("table", "items", "items", ddl[0]),
    ])?;
    assert_eq!(fingerprint_trusted_ddl(&ddl)?, expected);
    tokio::runtime::Builder::new_current_thread()
        .build()?
        .block_on(async {
            assert_eq!(fingerprint_trusted_ddl(&ddl).unwrap(), expected);
        });
    Ok(())
}

#[test]
fn rejects_input_and_schema_object_excess_and_sql_errors() {
    assert!(matches!(
        fingerprint_trusted_ddl(&vec!["SELECT 1"; MAX_FRAGMENTS + 1]),
        Err(DdlFingerprintError::InputLimit)
    ));
    assert!(matches!(
        fingerprint_trusted_ddl(&[&"x".repeat(MAX_DDL_BYTES + 1)]),
        Err(DdlFingerprintError::InputLimit)
    ));
    assert!(matches!(
        fingerprint_trusted_ddl(&["CREATE not valid"]),
        Err(DdlFingerprintError::Sqlite(_))
    ));
    let ddl = (0..=MAX_SCHEMA_OBJECTS)
        .map(|index| format!("CREATE TABLE item_{index}(id INTEGER);"))
        .collect::<String>();
    assert!(matches!(
        fingerprint_trusted_ddl(&[&ddl]),
        Err(DdlFingerprintError::ResultLimit)
    ));
    // A failed evaluation is closed; a fresh evaluation remains usable.
    assert!(fingerprint_trusted_ddl(&["CREATE TABLE after_failure(id INTEGER)"]).is_ok());
}

#[test]
fn native_progress_interrupts_unbounded_trusted_evaluation() {
    let start = Instant::now();
    assert!(matches!(
        fingerprint_trusted_ddl(&[
            "WITH RECURSIVE values_(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM values_) SELECT sum(n) FROM values_"
        ]),
        Err(DdlFingerprintError::Deadline)
    ));
    assert!(start.elapsed() < Duration::from_secs(10));
}

#[test]
fn evaluation_and_close_failure_retains_both_sources() {
    let error = DdlFingerprintError::EvaluationAndClose {
        operation: Box::new(DdlFingerprintError::Sqlite(sqlite_access::Error::Io(
            std::io::Error::from_raw_os_error(libc::ENOSPC),
        ))),
        close: sqlite_access::Error::Io(std::io::Error::from_raw_os_error(libc::EIO)),
    };
    assert!(
        std::error::Error::source(&error)
            .unwrap()
            .downcast_ref::<sqlite_access::Error>()
            .is_some()
    );
    let DdlFingerprintError::EvaluationAndClose { operation, close } = error else {
        panic!("expected retained evaluation and close failures")
    };
    assert!(
        matches!(*operation, DdlFingerprintError::Sqlite(sqlite_access::Error::Io(ref source)) if source.raw_os_error() == Some(libc::ENOSPC))
    );
    assert!(
        matches!(close, sqlite_access::Error::Io(ref source) if source.raw_os_error() == Some(libc::EIO))
    );
}
