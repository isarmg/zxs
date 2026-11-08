//! A bounded repository-local SQLite materialization of admitted directory state.
//! It carries local compositions, not external mount/path history or authority to
//! elect a publication. SQLite never opens an attacker-replaceable disk pathname.
use crate::{Error, ErrorCode, Result, State, disk::DiskRoot};
use data_serialization::Serialize;
use platform_io::sqlite::block_on_sqlite_connection;
use sqlite_access::{
    Connection, Executor, Row, SqliteConnection,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteOwnedBuf},
};
use std::path::Path;

mod projection;
mod material;
pub(super) use material::{read_material, reconstruct_material};

pub(crate) const NAME: &str = ".zxs-directory.sqlite";
const FORMAT: &str = "directory-local-compositions";
pub(super) const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
pub(super) const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_ROWS: usize = 65_536;
const META: &str = "CREATE TABLE identity (singleton INTEGER PRIMARY KEY CHECK(singleton=1), format TEXT NOT NULL, repository_id TEXT NOT NULL, directory_id TEXT NOT NULL, source_sequence TEXT NOT NULL, records_hash TEXT NOT NULL) STRICT";
const RECORDS: &str = "CREATE TABLE records (kind TEXT NOT NULL CHECK(kind IN ('repository','branch','tree','commit')), id TEXT NOT NULL, body TEXT NOT NULL, PRIMARY KEY(kind,id)) STRICT";

#[derive(Serialize)]
struct Record {
    kind: &'static str,
    id: String,
    body: String,
}

pub(super) struct Prepared {
    pub(super) bytes: Vec<u8>,
}

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::ContainerLayoutInvalid, message)
}
fn budget() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "directory database exceeds its bounded record or byte budget",
    )
}

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

pub(super) fn prepare(state: &State, directory: &str) -> Result<Prepared> {
    let records = projection::records(state, directory)?;
    let hash = storage_contract::object_hash("directory-local-records", &records)?;
    let git = &state.directory_git_repositories[directory];
    encode_records(records, &git.repository_id, directory, state.sequence, &hash)
}

fn encode_records(
    records: Vec<Record>,
    repository: &str,
    directory: &str,
    sequence: u64,
    hash: &str,
) -> Result<Prepared> {
    let bytes = block_on_sqlite_connection(async {
        let options = SqliteConnectOptions::new()
            .in_memory(true)
            .journal_mode(SqliteJournalMode::Memory);
        let mut connection = SqliteConnection::connect_with(&options).await?;
        let result = async {
            connection.execute("PRAGMA page_size=4096").await?;
            connection.execute("PRAGMA max_page_count=4096").await?;
            let mut tx = connection.begin().await?;
            tx.execute(META).await?;
            tx.execute(RECORDS).await?;
            sqlite_access::query("INSERT INTO identity VALUES(1,?,?,?,?,?)")
                .bind(FORMAT)
                .bind(repository)
                .bind(directory)
                .bind(sequence.to_string())
                .bind(&hash)
                .execute(&mut *tx)
                .await?;
            for record in records {
                sqlite_access::query("INSERT INTO records VALUES(?,?,?)")
                    .bind(record.kind)
                    .bind(record.id)
                    .bind(record.body)
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await?;
            let bytes = connection.serialize(None).await?;
            if bytes.len() > MAX_BYTES {
                return Err(budget());
            }
            Ok(bytes.to_vec())
        }
        .await;
        finish(result, connection).await
    })?;
    Ok(Prepared { bytes })
}

/// Admit a stored materialization before preserving/moving or replacing it.
/// Its scope and digest prove format consistency, never publication authority.
pub(super) fn verify_at(
    root: &DiskRoot,
    path: &Path,
    repository: &str,
    directory: &str,
) -> Result<usize> {
    let bytes = root.read_bytes(path, MAX_BYTES)?;
    verify_scope(&bytes, repository, directory)
        .map(|()| bytes.len())
        .map_err(|error| {
            error.context(
                ErrorCode::ContainerLayoutInvalid,
                "directory database failed bounded format and identity verification",
            )
        })
}

pub(super) fn verify_scope(bytes: &[u8], repository: &str, directory: &str) -> Result<()> {
    let identity = verify(bytes).map_err(|error| {
        error.context(
            ErrorCode::ContainerLayoutInvalid,
            "directory database failed bounded format and identity verification",
        )
    })?;
    if identity.repository != repository || identity.directory.as_str() != directory {
        return Err(invalid("directory database belongs to another directory"));
    }
    Ok(())
}

struct StoredIdentity {
    records: Vec<Record>,
    sequence: u64,
    records_hash: String,
    repository: String,
    directory: super::layout_locator::DirectoryId,
}

/// An absence scan cannot adopt a database or authorize writes. It only skips a
/// closed metadata file in its exact recorded directory, never an unindexed
/// file container. The later layout preflight still checks State ownership.
pub(super) fn verify_located_at(
    root: &DiskRoot,
    history: &Path,
    history_id: &str,
    path: &Path,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<usize> {
    let bytes = root.read_bytes_observed(path, MAX_BYTES, observe)?;
    let identity = verify(&bytes)?;
    let anchor = super::layout_locator::HistoryAnchor::new(root, history, history_id)?;
    let intent = super::layout_locator::load_directory(&anchor, &identity.directory, observe)?;
    let located = super::layout_locator::locate_directory(&anchor, &intent, observe)?;
    if path.parent() != Some(located.as_path()) {
        return Err(invalid("directory database is outside its recorded entity"));
    }
    Ok(bytes.len())
}

fn verify(bytes: &[u8]) -> Result<StoredIdentity> {
    if bytes.len() > MAX_BYTES {
        return Err(budget());
    }
    let database = SqliteOwnedBuf::try_from(bytes)?;
    block_on_sqlite_connection(async {
        let mut connection =
            SqliteConnection::connect_with(&SqliteConnectOptions::new().in_memory(true)).await?;
        let result = async {
            connection.deserialize(None, database, true).await?;
            verify_connection(&mut connection).await
        }
        .await;
        finish(result, connection).await
    })
}

async fn verify_schema(connection: &mut SqliteConnection) -> Result<()> {
    let schemas = sqlite_access::query(
        "SELECT name,type,tbl_name,sql FROM sqlite_schema ORDER BY name LIMIT 4",
    )
    .fetch_all(&mut *connection)
    .await?;
    if schemas.len() != 3 {
        return Err(invalid("directory database has unexpected schema objects"));
    }
    for (row, (name, kind, table, ddl)) in schemas.into_iter().zip([
        ("identity", "table", "identity", Some(META)),
        ("records", "table", "records", Some(RECORDS)),
        ("sqlite_autoindex_records_1", "index", "records", None),
    ]) {
        if row.try_get::<String, _>("name")? != name
            || row.try_get::<String, _>("type")? != kind
            || row.try_get::<String, _>("tbl_name")? != table
            || row.try_get::<Option<String>, _>("sql")?.as_deref() != ddl
        {
            return Err(invalid(
                "directory database schema differs from the supported format",
            ));
        }
    }
    Ok(())
}

async fn read_records(connection: &mut SqliteConnection) -> Result<Vec<Record>> {
    let mut records = Vec::<Record>::new();
    let mut bytes = 0;
    loop {
        // One bounded row at a time: a corrupt database must not multiply a
        // shared overflow payload across an unbounded fetch_all allocation.
        // No sentinel can precede every untrusted key: enumerate the first row
        // without a predicate, then continue from an actually observed key.
        let row = match records.last() {
            Some(last) => sqlite_access::query(
                "SELECT kind,id,body FROM records WHERE (kind,id)>(?,?) ORDER BY kind,id LIMIT 1",
            )
            .bind(last.kind)
            .bind(&last.id)
            .fetch_optional(&mut *connection)
            .await?,
            None => {
                sqlite_access::query("SELECT kind,id,body FROM records ORDER BY kind,id LIMIT 1")
                    .fetch_optional(&mut *connection)
                    .await?
            }
        };
        let Some(row) = row else {
            break;
        };
        if records.len() == MAX_ROWS {
            return Err(budget());
        }
        let kind: String = row.try_get("kind")?;
        let kind = match kind.as_str() {
            "repository" => "repository",
            "branch" => "branch",
            "tree" => "tree",
            "commit" => "commit",
            _ => return Err(invalid("directory database record kind is unsupported")),
        };
        let id: String = row.try_get("id")?;
        if id.is_empty() {
            return Err(invalid("directory database record identity is empty"));
        }
        let body: String = row.try_get("body")?;
        bytes += id.len() + body.len();
        if bytes > projection::MAX_RECORD_BYTES {
            return Err(budget());
        }
        records.push(Record { kind, id, body });
    }
    Ok(records)
}

async fn verify_connection(connection: &mut SqliteConnection) -> Result<StoredIdentity> {
    platform_io::sqlite::enable_defensive(connection)
        .await
        .map_err(|error| {
            Error::from(std::io::Error::other(error)).context(
                ErrorCode::ContainerLayoutInvalid,
                "cannot secure directory SQLite reader",
            )
        })?;
    let limits = platform_io::sqlite::ConnectionLimits::new(MAX_BYTES as u32)
        .with_max_sql_bytes(4096)
        .with_max_vm_operations(100_000)
        .with_max_bind_parameters(16);
    platform_io::sqlite::apply_connection_limits(connection, limits)
        .await
        .map_err(|error| {
            Error::from(std::io::Error::other(error)).context(
                ErrorCode::ContainerLayoutInvalid,
                "cannot bound directory SQLite reader",
            )
        })?;
    verify_schema(connection).await?;
    let identities = sqlite_access::query("SELECT * FROM identity LIMIT 2")
        .fetch_all(&mut *connection)
        .await?;
    if identities.len() != 1 {
        return Err(invalid("directory database has no unique owner"));
    }
    let owner = &identities[0];
    if owner.try_get::<i64, _>("singleton")? != 1 || owner.try_get::<String, _>("format")? != FORMAT
    {
        return Err(invalid(
            "directory database belongs to another directory or format",
        ));
    }
    let repository: String = owner.try_get("repository_id")?;
    storage_contract::validate_id(&repository, "repo")?;
    let directory: String = owner.try_get("directory_id")?;
    storage_contract::validate_id(
        &directory,
        if directory.starts_with("repo_") {
            "repo"
        } else {
            "entry"
        },
    )?;
    let directory = super::layout_locator::DirectoryId::parse(&directory)?;
    let sequence = storage_contract::parse_u64_decimal(&owner.try_get::<String, _>("source_sequence")?)?;
    let records = read_records(connection).await?;
    let records_hash = storage_contract::object_hash("directory-local-records", &records)?;
    if records_hash != owner.try_get::<String, _>("records_hash")?
    {
        return Err(invalid(
            "directory database records do not match their recorded digest",
        ));
    }
    Ok(StoredIdentity {
        records,
        sequence,
        records_hash,
        repository,
        directory,
    })
}
