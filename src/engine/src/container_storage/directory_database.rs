//! A bounded repository-local SQLite materialization of admitted directory state.
//! It carries local compositions, not external mount/path history or authority to
//! elect a publication. SQLite never opens an attacker-replaceable disk pathname.
pub(super) use super::local_database::MAX_BYTES;
use super::local_database::{Prepared, Record};
use crate::{Error, ErrorCode, Result, State, disk::DiskRoot};
use data_serialization::Serialize;
use sqlite_access::{Row, SqliteConnection};
use std::path::Path;

mod material;
mod projection;
pub(super) use material::{read_material, reconstruct_material};

pub(crate) const NAME: &str = ".zxs-directory.sqlite";
const FORMAT: &str = "directory-local-compositions";
pub(super) const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const META: &str = "CREATE TABLE identity (singleton INTEGER PRIMARY KEY CHECK(singleton=1), format TEXT NOT NULL, repository_id TEXT NOT NULL, directory_id TEXT NOT NULL, source_sequence TEXT NOT NULL, records_hash TEXT NOT NULL) STRICT";
const RECORDS: &str = "CREATE TABLE records (kind TEXT NOT NULL CHECK(kind IN ('repository','branch','tree','commit')), id TEXT NOT NULL, body TEXT NOT NULL, PRIMARY KEY(kind,id)) STRICT";

pub(super) const SCHEMA: super::local_database::Schema = super::local_database::Schema {
    name: "directory",
    format: FORMAT,
    identity: META,
    records: RECORDS,
    kinds: &["repository", "branch", "tree", "commit"],
};

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::ContainerLayoutInvalid, message)
}

pub(super) fn prepare(state: &State, directory: &str) -> Result<Prepared> {
    let records = projection::records(state, directory)?;
    let hash = storage_contract::object_hash("directory-local-records", &records)?;
    let git = &state.directory_git_repositories[directory];
    SCHEMA.encode(
        records,
        [&git.repository_id, directory],
        state.sequence,
        &hash,
    )
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
        return Err(SCHEMA.budget());
    }
    crate::sqlite_connection::read_image(bytes, MAX_BYTES as u32, verify_connection)
}

async fn verify_connection(connection: &mut SqliteConnection) -> Result<StoredIdentity> {
    SCHEMA.verify_schema(connection).await?;
    // Bound returned values in bytes, preserving NUL tails for exact rejection.
    let identities = sqlite_access::query(
        "SELECT singleton,CAST(substr(CAST(format AS BLOB),1,65) AS TEXT) AS format,CAST(substr(CAST(repository_id AS BLOB),1,257) AS TEXT) AS repository_id,CAST(substr(CAST(directory_id AS BLOB),1,257) AS TEXT) AS directory_id,CAST(substr(CAST(source_sequence AS BLOB),1,21) AS TEXT) AS source_sequence,CAST(substr(CAST(records_hash AS BLOB),1,65) AS TEXT) AS records_hash FROM identity LIMIT 2",
    )
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
    let sequence =
        storage_contract::parse_u64_decimal(&owner.try_get::<String, _>("source_sequence")?)?;
    let records = SCHEMA.read_records(connection, |_| Ok(())).await?;
    let records_hash = storage_contract::object_hash("directory-local-records", &records)?;
    if records_hash != owner.try_get::<String, _>("records_hash")? {
        return Err(invalid(
            "directory database records do not match their recorded digest",
        ));
    }
    crate::sqlite_integrity::verify(connection).await?;
    // The same typed closure governs rescue and any ordinary admission of old
    // material. A recomputed self-hash cannot authorize replacing unknown rows.
    material::validate_rows(&records, &repository, directory.as_str(), sequence)?;
    Ok(StoredIdentity {
        records,
        sequence,
        records_hash,
        repository,
        directory,
    })
}
