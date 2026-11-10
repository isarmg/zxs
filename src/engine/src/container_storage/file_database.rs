//! File-local SQLite projection of a captured State, not publication authority.
//! It never opens a disk pathname through SQLite and never authorizes a move.
use crate::{Error, ErrorCode, Result, state_projection::StateProjection};
use data_serialization::Serialize;
use sqlite_access::{
    Connection, Executor, Row, SqliteConnection,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteOwnedBuf},
};

pub(super) mod changes;
pub(super) mod install;
mod material;
mod projection;
mod rows;
pub(super) use material::{read_material, reconstruct_material};

pub(crate) const NAME: &str = ".zxs-file.sqlite";
const FORMAT: &str = "file-local-retained-graph";
pub(super) const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_ROWS: usize = 65_536;
const META: &str = "CREATE TABLE identity (singleton INTEGER PRIMARY KEY CHECK(singleton=1), format TEXT NOT NULL, file_id TEXT NOT NULL, history_tree_id TEXT NOT NULL, source_sequence TEXT NOT NULL, records_hash TEXT NOT NULL) STRICT";
const RECORDS: &str = "CREATE TABLE records (kind TEXT NOT NULL CHECK(kind IN ('file','version','branch','anchor','metadata','deletion','lifecycle','merge','merge_link')), id TEXT NOT NULL, body TEXT NOT NULL, PRIMARY KEY(kind,id)) STRICT";

#[derive(Serialize)]
struct Record {
    kind: &'static str,
    id: String,
    body: String,
}

struct ProjectedRecords {
    records: Vec<Record>,
    events: Vec<crate::file_deletion::retained::LifecycleRow>,
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
        "file database exceeds its bounded record or byte budget",
    )
}
/// Rows originate from the captured State and its tied owner index. The caller
/// must prove publication admission before installation. Neither this projection
/// nor its self digest grants authority or proves a complete historical copy.
fn validated_records(
    view: StateProjection<'_>,
    file: &str,
    history: &str,
) -> Result<ProjectedRecords> {
    validate_owner(view, file, history)?;
    let records = projection::records(view, file)?;
    validate_records(&records.records, file, history, view.state().sequence)?;
    Ok(records)
}

fn validate_owner(view: StateProjection<'_>, file: &str, history: &str) -> Result<()> {
    storage_contract::validate_id(file, "file")?;
    storage_contract::validate_id(history, "history")?;
    if !view
        .owners()
        .file_histories(file)
        .map(String::as_str)
        .eq([history])
    {
        return Err(invalid(
            "file database requires one exact retained history owner",
        ));
    }
    Ok(())
}

fn validate_records(records: &[Record], file: &str, history: &str, sequence: u64) -> Result<()> {
    let mut validation = rows::Validation::new(file, history, sequence);
    for record in records {
        validation.admit(record.kind, &record.id, &record.body)?;
    }
    validation.finish()
}

pub(crate) struct CopyCapture {
    pub(crate) records_hash: String,
    pub(crate) events: Vec<crate::file_deletion::retained::LifecycleRow>,
}

/// A complete bounded logical snapshot, not proof of a physical DB or disk.
/// The copy publication separately proves body sources and installs the exact
/// target database. A move cannot use this alone as its three-scope proof.
pub(crate) fn capture_copy(state: &crate::State, file: &str, history: &str) -> Result<CopyCapture> {
    let projection = crate::state_projection::ProjectionSnapshot::new(state);
    capture_projected_copy(projection.view(), file, history)
}

/// Aggregate moves reuse this bound source view across all retained files.
pub(crate) fn capture_projected_copy(
    view: StateProjection<'_>,
    file: &str,
    history: &str,
) -> Result<CopyCapture> {
    let state = view.state();
    if view.owners().file_rows(file).is_some_and(|rows| {
        rows.history_copies
            .keys()
            .any(|id| state.operations[id].kind == crate::business::history_copy::KIND)
    }) {
        return Err(Error::new(
            ErrorCode::UnsupportedOperation,
            "the older graph-only copy has no retained lifecycle closure; preserve it without upgrading its receipt",
        ));
    }
    let ProjectedRecords { records, events } = validated_records(view, file, history)?;
    let records_hash = storage_contract::object_hash("file-local-records", &records)?;
    Ok(CopyCapture {
        records_hash,
        events,
    })
}

pub(super) fn prepare(view: StateProjection<'_>, file: &str, history: &str) -> Result<Prepared> {
    let ProjectedRecords { records, .. } = validated_records(view, file, history)?;
    let hash = storage_contract::object_hash("file-local-records", &records)?;
    let sequence = view.state().sequence;
    encode_records(records, file, history, sequence, &hash)
}

fn encode_records(
    records: Vec<Record>,
    file: &str,
    history: &str,
    sequence: u64,
    hash: &str,
) -> Result<Prepared> {
    let bytes = crate::sqlite_connection::run(
        SqliteConnectOptions::new()
            .in_memory(true)
            .journal_mode(SqliteJournalMode::Memory),
        async |connection| {
            connection.execute("PRAGMA page_size=4096").await?;
            connection.execute("PRAGMA max_page_count=4096").await?;
            let mut tx = connection.begin().await?;
            tx.execute(META).await?;
            tx.execute(RECORDS).await?;
            sqlite_access::query("INSERT INTO identity VALUES(1,?,?,?,?,?)")
                .bind(FORMAT)
                .bind(file)
                .bind(history)
                .bind(sequence.to_string())
                .bind(hash)
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
        },
    )?;
    Ok(Prepared { bytes })
}

/// Only a logical staging database. Its constructors receive the typed joint
/// projection's audit, not a forged copy/delete operation receipt.
pub(crate) struct PreparedMoveFile {
    bytes: Vec<u8>,
    file: String,
    history: String,
    sequence: u64,
    records_hash: String,
}

impl PreparedMoveFile {
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn records_hash(&self) -> &str {
        &self.records_hash
    }

    pub(crate) fn verify(&self, bytes: &[u8]) -> Result<()> {
        let actual = verify_snapshot(bytes)?;
        if actual.file != self.file
            || actual.history != self.history
            || actual.sequence != self.sequence
            || actual.records_hash != self.records_hash
        {
            return Err(invalid(
                "staged move file database differs from the exact joint projection",
            ));
        }
        Ok(())
    }
}

pub(crate) fn prepare_move(
    state: &crate::State,
    audit: &crate::business::history_copy::retained::CopiedLifecycle,
) -> Result<PreparedMoveFile> {
    let transition = crate::state_projection::ProjectionTransition::new(state, state, None)?;
    let view = transition.previous();
    validate_owner(view, &audit.file_id, &audit.history_tree_id)?;
    let records = projection::move_records(view, &audit.file_id, &audit.events)?;
    validate_records(
        &records,
        &audit.file_id,
        &audit.history_tree_id,
        state.sequence,
    )?;
    let records_hash = storage_contract::object_hash("file-local-records", &records)?;
    let prepared = encode_records(
        records,
        &audit.file_id,
        &audit.history_tree_id,
        state.sequence,
        &records_hash,
    )?;
    Ok(PreparedMoveFile {
        bytes: prepared.bytes,
        file: audit.file_id.clone(),
        history: audit.history_tree_id.clone(),
        sequence: state.sequence,
        records_hash,
    })
}

/// Preserve the complete source audit prefix and append the proposed move's
/// own D. The normal published-State decoder still requires a real receipt;
/// this narrowly scoped encoder does not relax that admission rule.
pub(crate) fn prepare_move_source(
    previous: &crate::State,
    next: &crate::State,
    node: &crate::FileDeletionNode,
) -> Result<PreparedMoveFile> {
    use crate::file_deletion::retained::{DeletionAction, LifecycleRow};
    if previous.file_deletions.contains_key(&node.file_id)
        || previous.operations.contains_key(&node.operation_id)
        || next.operations.contains_key(&node.operation_id)
        || node.sequence != next.sequence
        || node.sequence <= previous.sequence
    {
        return Err(invalid(
            "move source database has no fresh unpublished deletion",
        ));
    }
    let mut events = capture_copy(previous, &node.file_id, &node.history_tree_id)?.events;
    events.push(LifecycleRow {
        operation_id: node.operation_id,
        action: DeletionAction::Delete,
        sequence: node.sequence,
        author: node.author.clone(),
        node: node.into(),
    });
    let transition = crate::state_projection::ProjectionTransition::new(next, next, None)?;
    let view = transition.previous();
    validate_owner(view, &node.file_id, &node.history_tree_id)?;
    let records = projection::move_source_records(view, node, &events)?;
    validate_records(
        &records,
        &node.file_id,
        &node.history_tree_id,
        next.sequence,
    )?;
    let records_hash = storage_contract::object_hash("file-local-records", &records)?;
    let prepared = encode_records(
        records,
        &node.file_id,
        &node.history_tree_id,
        next.sequence,
        &records_hash,
    )?;
    Ok(PreparedMoveFile {
        bytes: prepared.bytes,
        file: node.file_id.clone(),
        history: node.history_tree_id.clone(),
        sequence: next.sequence,
        records_hash,
    })
}

/// A pure byte boundary. Resource identity and installation belong to the
/// common installer; this verifier never reopens a pathname or authorizes I/O.
pub(super) fn verify_scope(bytes: &[u8], file: &str, history: &str) -> Result<()> {
    let identity = verify(bytes).map_err(|error| {
        error.context(
            ErrorCode::ContainerLayoutInvalid,
            "file database failed bounded format verification",
        )
    })?;
    if identity != (file.to_owned(), history.to_owned()) {
        return Err(invalid("file database belongs to another container"));
    }
    Ok(())
}

fn verify(bytes: &[u8]) -> Result<(String, String)> {
    let snapshot = verify_snapshot(bytes)?;
    Ok((snapshot.file, snapshot.history))
}

struct VerifiedSnapshot {
    records: Vec<Record>,
    file: String,
    history: String,
    sequence: u64,
    records_hash: String,
}

fn verify_snapshot(bytes: &[u8]) -> Result<VerifiedSnapshot> {
    if bytes.len() > MAX_BYTES {
        return Err(budget());
    }
    let database = SqliteOwnedBuf::try_from(bytes)?;
    crate::sqlite_connection::run(
        SqliteConnectOptions::new().in_memory(true),
        async |connection| {
            connection.deserialize(None, database, true).await?;
            verify_connection(&mut *connection).await
        },
    )
}
async fn verify_schema(connection: &mut SqliteConnection) -> Result<()> {
    let schemas = sqlite_access::query(
        "SELECT CAST(substr(CAST(name AS BLOB),1,257) AS TEXT) AS name,CAST(substr(CAST(type AS BLOB),1,257) AS TEXT) AS type,CAST(substr(CAST(tbl_name AS BLOB),1,257) AS TEXT) AS tbl_name,CAST(substr(CAST(sql AS BLOB),1,4097) AS TEXT) AS sql FROM sqlite_schema ORDER BY name LIMIT 4",
    )
    .fetch_all(&mut *connection)
    .await?;
    let expected = [
        ("identity", "table", "identity", Some(META)),
        ("records", "table", "records", Some(RECORDS)),
        ("sqlite_autoindex_records_1", "index", "records", None),
    ];
    if schemas.len() != expected.len() {
        return Err(invalid("file database has unexpected schema objects"));
    }
    for (row, (name, kind, table, ddl)) in schemas.into_iter().zip(expected) {
        if row.try_get::<String, _>("name")? != name
            || row.try_get::<String, _>("type")? != kind
            || row.try_get::<String, _>("tbl_name")? != table
            || row.try_get::<Option<String>, _>("sql")?.as_deref() != ddl
        {
            return Err(invalid(
                "file database schema differs from the supported format",
            ));
        }
    }
    Ok(())
}
async fn read_records(
    connection: &mut SqliteConnection,
    file: &str,
    history: &str,
    sequence: u64,
) -> Result<Vec<Record>> {
    let mut records = Vec::<Record>::new();
    let mut bytes = 0;
    let mut validation = rows::Validation::new(file, history, sequence);
    loop {
        // Enumerate one bounded row at a time. Shared overflow payloads in a
        // corrupt DB must not multiply into an unbounded fetch_all allocation.
        let row = match records.last() {
            Some(last) => sqlite_access::query(
                "SELECT CAST(substr(CAST(kind AS BLOB),1,17) AS TEXT) AS kind,CAST(substr(CAST(id AS BLOB),1,257) AS TEXT) AS id,length(CAST(id AS BLOB)) AS id_bytes,length(CAST(body AS BLOB)) AS body_bytes FROM records WHERE (records.kind,records.id)>(?,?) ORDER BY records.kind,records.id LIMIT 1",
            )
            .bind(last.kind)
            .bind(&last.id)
            .fetch_optional(&mut *connection)
            .await?,
            None => {
                sqlite_access::query("SELECT CAST(substr(CAST(kind AS BLOB),1,17) AS TEXT) AS kind,CAST(substr(CAST(id AS BLOB),1,257) AS TEXT) AS id,length(CAST(id AS BLOB)) AS id_bytes,length(CAST(body AS BLOB)) AS body_bytes FROM records ORDER BY records.kind,records.id LIMIT 1")
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
            "file" => "file",
            "version" => "version",
            "branch" => "branch",
            "anchor" => "anchor",
            "metadata" => "metadata",
            "deletion" => "deletion",
            "lifecycle" => "lifecycle",
            "merge" => "merge",
            "merge_link" => "merge_link",
            _ => return Err(invalid("file database record kind is unsupported")),
        };
        let id: String = row.try_get("id")?;
        if id.is_empty() || id.len() > 256 || row.try_get::<i64, _>("id_bytes")? != id.len() as i64
        {
            return Err(invalid(
                "file database record identity is not bounded exact text",
            ));
        }
        let length = usize::try_from(row.try_get::<i64, _>("body_bytes")?)
            .map_err(|_| invalid("file database record length is invalid"))?;
        let remaining = projection::MAX_RECORD_BYTES - bytes;
        if id.len() > remaining || length > remaining - id.len() {
            return Err(budget());
        }
        // Check the aggregate budget before copying an untrusted record body.
        let body: String = sqlite_access::query_scalar(
            "SELECT CAST(substr(CAST(body AS BLOB),1,?) AS TEXT) FROM records WHERE kind=? AND id=?",
        )
        .bind((length + 1) as i64)
        .bind(kind)
        .bind(&id)
        .fetch_one(&mut *connection)
        .await?;
        if body.len() != length {
            return Err(invalid(
                "file database record length differs from its bytes",
            ));
        }
        bytes += id.len() + length;
        validation.admit(kind, &id, &body)?;
        records.push(Record { kind, id, body });
    }
    validation.finish()?;
    Ok(records)
}
async fn verify_connection(connection: &mut SqliteConnection) -> Result<VerifiedSnapshot> {
    platform_io::sqlite::enable_defensive(connection)
        .await
        .map_err(|error| {
            Error::from(std::io::Error::other(error)).context(
                ErrorCode::ContainerLayoutInvalid,
                "cannot secure file SQLite reader",
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
                "cannot bound file SQLite reader",
            )
        })?;
    verify_schema(connection).await?;
    // Bound returned values in bytes, preserving NUL tails for exact rejection.
    let identities = sqlite_access::query(
        "SELECT singleton,CAST(substr(CAST(format AS BLOB),1,65) AS TEXT) AS format,CAST(substr(CAST(file_id AS BLOB),1,257) AS TEXT) AS file_id,CAST(substr(CAST(history_tree_id AS BLOB),1,257) AS TEXT) AS history_tree_id,CAST(substr(CAST(source_sequence AS BLOB),1,21) AS TEXT) AS source_sequence,CAST(substr(CAST(records_hash AS BLOB),1,65) AS TEXT) AS records_hash FROM identity LIMIT 2",
    )
        .fetch_all(&mut *connection)
        .await?;
    if identities.len() != 1 {
        return Err(invalid("file database has no unique owner"));
    }
    let owner = &identities[0];
    if owner.try_get::<i64, _>("singleton")? != 1 || owner.try_get::<String, _>("format")? != FORMAT
    {
        return Err(invalid("file database belongs to another format"));
    }
    let file: String = owner.try_get("file_id")?;
    storage_contract::validate_id(&file, "file")?;
    let history: String = owner.try_get("history_tree_id")?;
    storage_contract::validate_id(&history, "history")?;
    let sequence =
        storage_contract::parse_u64_decimal(&owner.try_get::<String, _>("source_sequence")?)?;
    let records = read_records(connection, &file, &history, sequence).await?;
    let records_hash = storage_contract::object_hash("file-local-records", &records)?;
    if records_hash != owner.try_get::<String, _>("records_hash")? {
        return Err(invalid(
            "file database records differ from their recorded digest",
        ));
    }
    crate::sqlite_integrity::verify(connection).await?;
    Ok(VerifiedSnapshot {
        file,
        history,
        sequence,
        records_hash,
        records,
    })
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/container_storage/file_database_tests.rs"]
mod tests;
