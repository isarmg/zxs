//! SQLite is built/read in memory. The disk-facing adapter reads bounded bytes
//! under the retained I/O gate and gives them to the common safe installer.
use super::codec;
use super::*;
use sqlite_access::{Executor, Row, SqliteConnection};

const FORMAT: &str = "disk-current-local-catalog";
const IDENTITY: &str = "CREATE TABLE identity(singleton INTEGER PRIMARY KEY CHECK(singleton=1),format TEXT NOT NULL,scope TEXT NOT NULL,source_sequence TEXT NOT NULL,records_hash TEXT NOT NULL) STRICT";
const VERSIONS: &str =
    "CREATE TABLE versions(version_id TEXT PRIMARY KEY COLLATE BINARY,body TEXT NOT NULL) STRICT";

const DIRECTORIES: &str = "CREATE TABLE backup_directories(ordinal INTEGER PRIMARY KEY CHECK(ordinal>=0),body TEXT NOT NULL) STRICT";

pub(super) fn encode(snapshot: &DiskCatalogSnapshot, hash: &str) -> Result<Vec<u8>> {
    crate::sqlite_connection::materialize(MAX_DATABASE_BYTES, budget, async |connection| {
        connection.execute(IDENTITY).await?;
        connection.execute(VERSIONS).await?;
        connection.execute(DIRECTORIES).await?;
        let scope = json_codec::to_string(&snapshot.identity)?;
        sqlite_access::query("INSERT INTO identity VALUES(1,?,?,?,?)")
            .bind(FORMAT)
            .bind(scope)
            .bind(snapshot.source_sequence.to_string())
            .bind(hash)
            .execute(&mut *connection)
            .await?;
        for row in &snapshot.versions {
            let bytes = crate::disk::encode_metadata(row, MAX_RECORD_BYTES)?;
            let body = std::str::from_utf8(&bytes).expect("invariant: metadata is UTF-8");
            sqlite_access::query("INSERT INTO versions VALUES(?,?)")
                .bind(&row.version.version_id)
                .bind(body)
                .execute(&mut *connection)
                .await?;
        }
        for (ordinal, row) in snapshot.backup_directories.iter().enumerate() {
            let bytes = crate::disk::encode_metadata(row, MAX_RECORD_BYTES)?;
            sqlite_access::query("INSERT INTO backup_directories VALUES(?,?)")
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
) -> Result<DiskCatalogSnapshot> {
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
) -> Result<DiskCatalogSnapshot> {
    crate::sqlite_connection::require_schema(
        connection,
        &[
            (
                "backup_directories",
                "table",
                "backup_directories",
                Some(DIRECTORIES),
            ),
            ("identity", "table", "identity", Some(IDENTITY)),
            ("sqlite_autoindex_versions_1", "index", "versions", None),
            ("versions", "table", "versions", Some(VERSIONS)),
        ],
        "disk catalog",
    )
    .await?;
    let headers = sqlite_access::query(
        "SELECT singleton,CAST(substr(CAST(format AS BLOB),1,65) AS TEXT) AS format,substr(CAST(scope AS BLOB),1,8193) AS scope,CAST(substr(CAST(source_sequence AS BLOB),1,21) AS TEXT) AS source_sequence,CAST(substr(CAST(records_hash AS BLOB),1,65) AS TEXT) AS records_hash FROM identity LIMIT 2",
    )
        .fetch_all(&mut *connection)
        .await?;
    if headers.len() != 1 {
        return Err(invalid("disk catalog requires one exact identity"));
    }
    let header = &headers[0];
    let scope: Vec<u8> = header.try_get("scope")?;
    if scope.len() > 8192 {
        return Err(invalid("disk catalog scope header exceeds its byte budget"));
    }
    let stored_identity: DiskRegistrationIdentity = storage_contract::decode_strict(&scope)?;
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
            None => sqlite_access::query("SELECT CAST(substr(CAST(version_id AS BLOB),1,257) AS TEXT),length(CAST(version_id AS BLOB)),length(CAST(body AS BLOB)) FROM versions ORDER BY version_id LIMIT 1")
                .fetch_optional(&mut *connection).await?,
            Some(last) => sqlite_access::query("SELECT CAST(substr(CAST(version_id AS BLOB),1,257) AS TEXT),length(CAST(version_id AS BLOB)),length(CAST(body AS BLOB)) FROM versions WHERE version_id>? ORDER BY version_id LIMIT 1")
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
    // The exact local directory row set shares the current catalog's total quota.
    let backup_directories = codec::DIRECTORY_ROWS
        .read(
            connection,
            MAX_ROWS - versions.len(),
            &mut total,
            |previous: Option<&DiskCatalogDirectory>, value| {
                value.validate()?;
                if previous.is_some_and(|previous| previous.key() >= value.key()) {
                    return Err(invalid(
                        "disk directories are not unique and in identity/path order",
                    ));
                }
                Ok(())
            },
        )
        .await?;
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
    crate::sqlite_integrity::verify(connection).await?;
    Ok(snapshot)
}
