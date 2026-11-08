//! SQLite is built/read in memory. The disk-facing adapter reads bounded bytes
//! under the retained I/O gate and gives them to the common safe installer.
use super::*;
use platform_io::sqlite::block_on_sqlite_connection;
use sqlite_access::{
    Connection, Executor, Row, SqliteConnection,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteOwnedBuf},
};

mod directories;

const FORMAT: &str = "disk-current-local-catalog";
const IDENTITY: &str = "CREATE TABLE identity(singleton INTEGER PRIMARY KEY CHECK(singleton=1),format TEXT NOT NULL,scope TEXT NOT NULL,source_sequence TEXT NOT NULL,records_hash TEXT NOT NULL) STRICT";
const VERSIONS: &str =
    "CREATE TABLE versions(version_id TEXT PRIMARY KEY COLLATE BINARY,body TEXT NOT NULL) STRICT";

const DIRECTORIES: &str = "CREATE TABLE backup_directories(ordinal INTEGER PRIMARY KEY CHECK(ordinal>=0),body TEXT NOT NULL) STRICT";

async fn finish<T>(result: Result<T>, connection: SqliteConnection) -> Result<T> {
    match (result, connection.close().await) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error.into()),
        (Err(error), Err(close)) => Err(Error::Cleanup {
            operation: Box::new(error),
            cleanup: std::io::Error::other(close),
        }),
    }
}

pub(super) fn encode(snapshot: &DiskCatalogSnapshot, hash: &str) -> Result<Vec<u8>> {
    block_on_sqlite_connection(async {
        let options = SqliteConnectOptions::new()
            .in_memory(true)
            .journal_mode(SqliteJournalMode::Memory);
        let mut connection = SqliteConnection::connect_with(&options).await?;
        let result = async {
            connection.execute("PRAGMA page_size=4096").await?;
            connection.execute("PRAGMA max_page_count=8192").await?;
            let mut tx = connection.begin().await?;
            tx.execute(IDENTITY).await?;
            tx.execute(VERSIONS).await?;
            tx.execute(DIRECTORIES).await?;
            let scope = json_codec::to_string(&snapshot.identity)?;
            sqlite_access::query("INSERT INTO identity VALUES(1,?,?,?,?)")
                .bind(FORMAT)
                .bind(scope)
                .bind(snapshot.source_sequence.to_string())
                .bind(hash)
                .execute(&mut *tx)
                .await?;
            for row in &snapshot.versions {
                let bytes = crate::disk::encode_metadata(row, MAX_RECORD_BYTES)?;
                let body = std::str::from_utf8(&bytes).expect("invariant: metadata is UTF-8");
                sqlite_access::query("INSERT INTO versions VALUES(?,?)")
                    .bind(&row.version.version_id)
                    .bind(body)
                    .execute(&mut *tx)
                    .await?;
            }
            for (ordinal, row) in snapshot.backup_directories.iter().enumerate() {
                let bytes = crate::disk::encode_metadata(row, MAX_RECORD_BYTES)?;
                sqlite_access::query("INSERT INTO backup_directories VALUES(?,?)")
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
        }
        .await;
        finish(result, connection).await
    })
}

pub(super) fn read(
    bytes: &[u8],
    identity: &DiskRegistrationIdentity,
) -> Result<DiskCatalogSnapshot> {
    if bytes.len() > MAX_DATABASE_BYTES {
        return Err(budget());
    }
    let database = SqliteOwnedBuf::try_from(bytes)?;
    block_on_sqlite_connection(async {
        let mut connection =
            SqliteConnection::connect_with(&SqliteConnectOptions::new().in_memory(true)).await?;
        let result = async {
            platform_io::sqlite::apply_connection_limits(
                &mut connection,
                platform_io::sqlite::ConnectionLimits::new(MAX_DATABASE_BYTES as u32)
                    .with_max_sql_bytes(4096)
                    .with_max_vm_operations(100_000)
                    .with_max_bind_parameters(16),
            )
            .await
            .map_err(|error| Error::from(std::io::Error::other(error)))?;
            platform_io::sqlite::enable_defensive(&mut connection)
                .await
                .map_err(|error| Error::from(std::io::Error::other(error)))?;
            connection.deserialize(None, database, true).await?;
            read_connection(&mut connection, identity).await
        }
        .await;
        finish(result, connection).await
    })
}

async fn read_connection(
    connection: &mut SqliteConnection,
    identity: &DiskRegistrationIdentity,
) -> Result<DiskCatalogSnapshot> {
    let schema = sqlite_access::query(
        "SELECT name,type,tbl_name,sql FROM sqlite_schema ORDER BY name LIMIT 5",
    )
    .fetch_all(&mut *connection)
    .await?;
    if schema.len() != 4 {
        return Err(invalid("disk catalog has unexpected schema objects"));
    }
    for (row, (name, kind, table, sql)) in schema.into_iter().zip([
        ("backup_directories", "table", "backup_directories", Some(DIRECTORIES)),
        ("identity", "table", "identity", Some(IDENTITY)),
        ("sqlite_autoindex_versions_1", "index", "versions", None),
        ("versions", "table", "versions", Some(VERSIONS)),
    ]) {
        if row.try_get::<String, _>(0)? != name
            || row.try_get::<String, _>(1)? != kind
            || row.try_get::<String, _>(2)? != table
            || row.try_get::<Option<String>, _>(3)?.as_deref() != sql
        {
            return Err(invalid(
                "disk catalog schema differs from the supported format",
            ));
        }
    }
    let headers = sqlite_access::query("SELECT * FROM identity LIMIT 2")
        .fetch_all(&mut *connection)
        .await?;
    if headers.len() != 1 {
        return Err(invalid("disk catalog requires one exact identity"));
    }
    let header = &headers[0];
    let scope: String = header.try_get("scope")?;
    let stored_identity: DiskRegistrationIdentity =
        storage_contract::decode_strict(scope.as_bytes())?;
    if header.try_get::<i64, _>("singleton")? != 1
        || header.try_get::<String, _>("format")? != FORMAT
        || &stored_identity != identity
    {
        return Err(invalid(
            "disk catalog belongs to another disk binding or format",
        ));
    }
    let sequence =
        storage_contract::parse_u64_decimal(&header.try_get::<String, _>("source_sequence")?)?;
    let mut versions = Vec::<DiskCatalogVersion>::new();
    let mut total = 0usize;
    loop {
        let row = match versions.last() {
            None => sqlite_access::query("SELECT substr(version_id,1,257),length(CAST(version_id AS BLOB)),length(CAST(body AS BLOB)) FROM versions ORDER BY version_id LIMIT 1")
                .fetch_optional(&mut *connection).await?,
            Some(last) => sqlite_access::query("SELECT substr(version_id,1,257),length(CAST(version_id AS BLOB)),length(CAST(body AS BLOB)) FROM versions WHERE version_id>? ORDER BY version_id LIMIT 1")
                .bind(&last.version.version_id).fetch_optional(&mut *connection).await?,
        };
        let Some(row) = row else {
            break;
        };
        if versions.len() == MAX_ROWS {
            return Err(budget());
        }
        let id: String = row.try_get(0)?;
        storage_contract::validate_id(&id, "ver")?;
        if row.try_get::<i64, _>(1)? != id.len() as i64 {
            return Err(invalid("disk catalog contains a noncanonical version key"));
        }
        let length = usize::try_from(row.try_get::<i64, _>(2)?)
            .map_err(|_| invalid("disk catalog row has an invalid length"))?;
        if length > MAX_RECORD_BYTES - total {
            return Err(budget());
        }
        let body: Vec<u8> = sqlite_access::query_scalar(
            "SELECT substr(CAST(body AS BLOB),1,?) FROM versions WHERE version_id=?",
        )
        .bind((length + 1) as i64)
        .bind(&id)
        .fetch_one(&mut *connection)
        .await?;
        let value: DiskCatalogVersion = storage_contract::decode_strict(&body)?;
        if body.len() != length
            || value.version.version_id != id
            || crate::disk::encode_metadata(&value, MAX_RECORD_BYTES)? != body
        {
            return Err(invalid(
                "disk catalog version row is noncanonical or mismatched",
            ));
        }
        storage_contract::validate_id(&value.version.file_id, "file")?;
        storage_contract::validate_id(&value.version.history_tree_id, "history")?;
        if let Some(path) = &value.owner_path {
            name_policy::PortablePath::parse(path)?;
        }
        total += length;
        versions.push(value);
    }
    let backup_directories = directories::read(connection, versions.len(), &mut total).await?;
    let snapshot = DiskCatalogSnapshot {
        identity: stored_identity,
        source_sequence: sequence,
        versions,
        backup_directories,
    };
    if storage_contract::object_hash("disk-current-catalog", &snapshot)?
        != header.try_get::<String, _>("records_hash")?
    {
        return Err(invalid(
            "disk catalog rows differ from their recorded digest",
        ));
    }
    Ok(snapshot)
}
