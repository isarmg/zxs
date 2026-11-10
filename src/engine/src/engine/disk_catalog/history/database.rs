//! Bounded in-memory SQLite codec. Stored digests check representation only;
//! the caller must still derive the entire prefix from admitted publications.
use super::super::codec;
use super::*;
use sqlite_access::{Executor, Row, SqliteConnection};

const FORMAT: &str = "disk-local-inventory-changes";
const MAX_HEADER_BYTES: usize = 8192;
const IDENTITY: &str = "CREATE TABLE identity(singleton INTEGER PRIMARY KEY CHECK(singleton=1),format TEXT NOT NULL,body TEXT NOT NULL,records_hash TEXT NOT NULL) STRICT";
const CHANGES: &str =
    "CREATE TABLE changes(ordinal INTEGER PRIMARY KEY CHECK(ordinal>=0),body TEXT NOT NULL) STRICT";

pub(super) fn encode(snapshot: &DiskHistorySnapshot, hash: &str) -> Result<Vec<u8>> {
    let header = crate::disk::encode_metadata(&snapshot.header, MAX_HEADER_BYTES)?;
    crate::sqlite_connection::materialize(MAX_DATABASE_BYTES, budget, async |connection| {
        connection.execute(IDENTITY).await?;
        connection.execute(CHANGES).await?;
        sqlite_access::query("INSERT INTO identity VALUES(1,?,?,?)")
            .bind(FORMAT)
            .bind(std::str::from_utf8(&header).expect("invariant: metadata is UTF-8"))
            .bind(hash)
            .execute(&mut *connection)
            .await?;
        for (ordinal, row) in snapshot.changes.iter().enumerate() {
            let bytes = crate::disk::encode_metadata(row, MAX_RECORD_BYTES)?;
            sqlite_access::query("INSERT INTO changes VALUES(?,?)")
                .bind(ordinal as i64)
                .bind(std::str::from_utf8(&bytes).expect("invariant: metadata is UTF-8"))
                .execute(&mut *connection)
                .await?;
        }
        Ok(())
    })
}

pub(super) fn read(
    bytes: &[u8],
    identity: &DiskRegistrationIdentity,
) -> Result<DiskHistorySnapshot> {
    if bytes.len() > MAX_DATABASE_BYTES {
        return Err(budget());
    }
    crate::sqlite_connection::read_image(bytes, MAX_DATABASE_BYTES as u32, async |connection| {
        read_connection(connection, identity).await
    })
}

async fn read_connection(
    connection: &mut SqliteConnection,
    identity: &DiskRegistrationIdentity,
) -> Result<DiskHistorySnapshot> {
    crate::sqlite_connection::require_schema(
        connection,
        &[
            ("changes", "table", "changes", Some(CHANGES)),
            ("identity", "table", "identity", Some(IDENTITY)),
        ],
        "disk history",
    )
    .await?;
    let headers = sqlite_access::query("SELECT singleton,CAST(substr(CAST(format AS BLOB),1,65) AS TEXT),length(CAST(body AS BLOB)),substr(CAST(body AS BLOB),1,8193),CAST(substr(CAST(records_hash AS BLOB),1,65) AS TEXT) FROM identity LIMIT 2")
        .fetch_all(&mut *connection).await?;
    if headers.len() != 1 {
        return Err(invalid("disk history requires one scope header"));
    }
    let row = &headers[0];
    let length = usize::try_from(row.try_get::<i64, _>(2)?)
        .map_err(|_| invalid("invalid history header length"))?;
    if row.try_get::<i64, _>(0)? != 1
        || row.try_get::<String, _>(1)? != FORMAT
        || length > MAX_HEADER_BYTES
    {
        return Err(invalid("disk history header format or size differs"));
    }
    let bytes: Vec<u8> = row.try_get(3)?;
    let header: DiskHistoryHeader = storage_contract::decode_strict(&bytes)?;
    if bytes.len() != length
        || header.identity != *identity
        || header.publications as usize > MAX_PUBLICATIONS
        || crate::disk::encode_metadata(&header, MAX_HEADER_BYTES)? != bytes
    {
        return Err(invalid("disk history scope or canonical header differs"));
    }
    let changes = read_changes(connection, header.through_sequence).await?;
    let snapshot = DiskHistorySnapshot { header, changes };
    if digest(&snapshot)? != row.try_get::<String, _>(4)? {
        return Err(invalid(
            "disk history rows differ from their recorded digest",
        ));
    }
    crate::sqlite_integrity::verify(connection).await?;
    Ok(snapshot)
}

async fn read_changes(
    connection: &mut SqliteConnection,
    through: u64,
) -> Result<Vec<DiskHistoryRecord>> {
    let mut total = 0;
    codec::HISTORY_ROWS
        .read(
            connection,
            MAX_CHANGES,
            &mut total,
            |previous: Option<&DiskHistoryRecord>, value| {
                validate_record(value, through)?;
                if previous.is_some_and(|previous| {
                    (previous.sequence, previous.change.key())
                        >= (value.sequence, value.change.key())
                }) {
                    return Err(invalid(
                        "disk history changes are not in publication/entity order",
                    ));
                }
                Ok(())
            },
        )
        .await
}

fn validate_record(record: &DiskHistoryRecord, through: u64) -> Result<()> {
    if record.sequence == 0 || record.sequence > through {
        return Err(invalid(
            "disk history change lies outside its declared coverage",
        ));
    }
    let row = |row: &DiskCatalogVersion| -> Result<()> {
        storage_contract::validate_id(&row.version.version_id, "ver")?;
        storage_contract::validate_id(&row.version.file_id, "file")?;
        storage_contract::validate_id(&row.version.history_tree_id, "history")?;
        if let Some(path) = &row.owner_path {
            name_policy::PortablePath::parse(path)?;
        }
        Ok(())
    };
    match &record.change {
        DiskHistoryChange::BackupDirectoryEntered { after } => after.validate()?,
        DiskHistoryChange::BackupDirectoryDeparted { before } => before.validate()?,
        DiskHistoryChange::BackupDirectoryChanged { before, after } => {
            before.validate()?;
            after.validate()?;
            if before.key() != after.key() || before == after {
                return Err(invalid(
                    "disk directory change has no exact same-identity/path transition",
                ));
            }
        }
        DiskHistoryChange::Entered { after } => row(after)?,
        DiskHistoryChange::Departed { before } => row(before)?,
        DiskHistoryChange::Changed { before, after } => {
            row(before)?;
            row(after)?;
            if before.version.version_id != after.version.version_id || before.as_ref() == after {
                return Err(invalid(
                    "disk history changed row has no exact same-version transition",
                ));
            }
        }
    }
    Ok(())
}
