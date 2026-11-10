//! Bounded in-memory SQLite codec. Stored digests check representation only;
//! the caller must still derive the entire prefix from admitted publications.
use super::*;
use crate::sqlite_connection::load_read_only;
use sqlite_access::{
    Connection, Executor, Row, SqliteConnection,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteOwnedBuf},
};

const FORMAT: &str = "disk-local-inventory-changes";
const MAX_HEADER_BYTES: usize = 8192;
const IDENTITY: &str = "CREATE TABLE identity(singleton INTEGER PRIMARY KEY CHECK(singleton=1),format TEXT NOT NULL,body TEXT NOT NULL,records_hash TEXT NOT NULL) STRICT";
const CHANGES: &str =
    "CREATE TABLE changes(ordinal INTEGER PRIMARY KEY CHECK(ordinal>=0),body TEXT NOT NULL) STRICT";

pub(super) fn encode(snapshot: &DiskHistorySnapshot, hash: &str) -> Result<Vec<u8>> {
    let header = crate::disk::encode_metadata(&snapshot.header, MAX_HEADER_BYTES)?;
    crate::sqlite_connection::run(
        SqliteConnectOptions::new()
            .in_memory(true)
            .journal_mode(SqliteJournalMode::Memory),
        async |connection| {
            connection.execute("PRAGMA page_size=4096").await?;
            connection.execute("PRAGMA max_page_count=8192").await?;
            let mut tx = connection.begin().await?;
            tx.execute(IDENTITY).await?;
            tx.execute(CHANGES).await?;
            sqlite_access::query("INSERT INTO identity VALUES(1,?,?,?)")
                .bind(FORMAT)
                .bind(std::str::from_utf8(&header).expect("invariant: metadata is UTF-8"))
                .bind(hash)
                .execute(&mut *tx)
                .await?;
            for (ordinal, row) in snapshot.changes.iter().enumerate() {
                let bytes = crate::disk::encode_metadata(row, MAX_RECORD_BYTES)?;
                sqlite_access::query("INSERT INTO changes VALUES(?,?)")
                    .bind(ordinal as i64)
                    .bind(std::str::from_utf8(&bytes).expect("invariant: metadata is UTF-8"))
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await?;
            let bytes = connection.serialize(None).await?;
            if bytes.len() > MAX_DATABASE_BYTES {
                return Err(budget());
            }
            Ok(bytes.to_vec())
        },
    )
}

pub(super) fn read(
    bytes: &[u8],
    identity: &DiskRegistrationIdentity,
) -> Result<DiskHistorySnapshot> {
    if bytes.len() > MAX_DATABASE_BYTES {
        return Err(budget());
    }
    let database = SqliteOwnedBuf::try_from(bytes)?;
    crate::sqlite_connection::run(
        SqliteConnectOptions::new().in_memory(true),
        async |connection| {
            load_read_only(connection, database, MAX_DATABASE_BYTES as u32).await?;
            read_connection(&mut *connection, identity).await
        },
    )
}

async fn read_connection(
    connection: &mut SqliteConnection,
    identity: &DiskRegistrationIdentity,
) -> Result<DiskHistorySnapshot> {
    let schema = sqlite_access::query(
        "SELECT CAST(substr(CAST(name AS BLOB),1,257) AS TEXT),CAST(substr(CAST(type AS BLOB),1,257) AS TEXT),CAST(substr(CAST(tbl_name AS BLOB),1,257) AS TEXT),CAST(substr(CAST(sql AS BLOB),1,4097) AS TEXT) FROM sqlite_schema ORDER BY name LIMIT 3",
    )
    .fetch_all(&mut *connection)
    .await?;
    if schema.len() != 2 {
        return Err(invalid("disk history has unexpected schema objects"));
    }
    for (row, (name, sql)) in schema
        .into_iter()
        .zip([("changes", CHANGES), ("identity", IDENTITY)])
    {
        if row.try_get::<String, _>(0)? != name
            || row.try_get::<String, _>(1)? != "table"
            || row.try_get::<String, _>(2)? != name
            || row.try_get::<Option<String>, _>(3)?.as_deref() != Some(sql)
        {
            return Err(invalid(
                "disk history schema differs from the supported format",
            ));
        }
    }
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
    let mut changes = Vec::<DiskHistoryRecord>::new();
    let mut total = 0usize;
    loop {
        let row = if changes.is_empty() {
            // No empty/small sentinel: inspect even a malformed first key.
            sqlite_access::query(
                "SELECT ordinal,length(CAST(body AS BLOB)) FROM changes ORDER BY ordinal LIMIT 1",
            )
            .fetch_optional(&mut *connection)
            .await?
        } else {
            sqlite_access::query("SELECT ordinal,length(CAST(body AS BLOB)) FROM changes WHERE ordinal>? ORDER BY ordinal LIMIT 1")
                .bind(changes.len() as i64 - 1).fetch_optional(&mut *connection).await?
        };
        let Some(row) = row else {
            break;
        };
        if changes.len() == MAX_CHANGES {
            return Err(budget());
        }
        if row.try_get::<i64, _>(0)? != changes.len() as i64 {
            return Err(invalid("disk history row ordinals are not contiguous"));
        }
        let length = usize::try_from(row.try_get::<i64, _>(1)?)
            .map_err(|_| invalid("invalid disk history row length"))?;
        if length > MAX_RECORD_BYTES - total {
            return Err(budget());
        }
        let body: Vec<u8> = sqlite_access::query_scalar(
            "SELECT substr(CAST(body AS BLOB),1,?) FROM changes WHERE ordinal=?",
        )
        .bind((length + 1) as i64)
        .bind(changes.len() as i64)
        .fetch_one(&mut *connection)
        .await?;
        let value: DiskHistoryRecord = storage_contract::decode_strict(&body)?;
        if body.len() != length || crate::disk::encode_metadata(&value, MAX_RECORD_BYTES)? != body {
            return Err(invalid("disk history row encoding is not canonical"));
        }
        validate_record(&value, through)?;
        if changes.last().is_some_and(|previous| {
            (previous.sequence, previous.change.key()) >= (value.sequence, value.change.key())
        }) {
            return Err(invalid(
                "disk history changes are not in publication/entity order",
            ));
        }
        total += length;
        changes.push(value);
    }
    Ok(changes)
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
