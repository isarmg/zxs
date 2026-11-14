//! File-local SQLite projection of a captured State, not publication authority.
//! It never opens a disk pathname through SQLite and never authorizes a move.
pub(crate) use super::local_database::MAX_BYTES;
use super::local_database::{Prepared, Record};
use crate::{Error, ErrorCode, Result, state_projection::StateProjection};
use sqlite_access::{Row, SqliteConnection};

pub(super) mod changes;
pub(super) mod install;
mod material;
mod projection;
mod rows;
pub(super) use material::{read_material, reconstruct_material};

pub(crate) const NAME: &str = ".zxs-file.sqlite";
const FORMAT: &str = "file-local-container-current-name";
pub(super) const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const META: &str = "CREATE TABLE identity (singleton INTEGER PRIMARY KEY CHECK(singleton=1), format TEXT NOT NULL, file_id TEXT NOT NULL, history_tree_id TEXT NOT NULL, source_sequence TEXT NOT NULL, records_hash TEXT NOT NULL) STRICT";
const RECORDS: &str = "CREATE TABLE records (kind TEXT NOT NULL CHECK(kind IN ('file','version','branch','path_change','metadata','deletion','lifecycle','merge','merge_link')), id TEXT NOT NULL, body TEXT NOT NULL, PRIMARY KEY(kind,id)) STRICT";

pub(super) const SCHEMA: super::local_database::Schema = super::local_database::Schema {
    name: "file",
    format: FORMAT,
    identity: META,
    records: RECORDS,
    kinds: &[
        "file",
        "version",
        "branch",
        "path_change",
        "metadata",
        "deletion",
        "lifecycle",
        "merge",
        "merge_link",
    ],
};

struct ProjectedRecords {
    records: Vec<Record>,
    events: Vec<crate::file_deletion::retained::LifecycleRow>,
}
fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::ContainerLayoutInvalid, message)
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
    let ProjectedRecords { records, events } = validated_records(view, file, history)?;
    let material = CapturedMaterial::from_validated_records(
        records,
        file.to_owned(),
        history.to_owned(),
        view.state().sequence,
    )?;
    Ok(CopyCapture {
        records_hash: material.records_hash,
        events,
    })
}

pub(super) fn prepare(view: StateProjection<'_>, file: &str, history: &str) -> Result<Prepared> {
    let prepared = CapturedMaterial::from_projection(view, file, history)?.encode()?;
    Ok(Prepared {
        bytes: prepared.bytes,
    })
}

/// An encoded logical material, not publication or installation authority.
/// Move staging still requires its typed joint audit instead of forged receipts.
pub(crate) struct PreparedFileMaterial {
    bytes: Vec<u8>,
    file: String,
    history: String,
    sequence: u64,
    records_hash: String,
}

impl PreparedFileMaterial {
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
) -> Result<PreparedFileMaterial> {
    let transition = crate::state_projection::ProjectionTransition::new(state, state, None)?;
    let view = transition.previous();
    validate_owner(view, &audit.file_id, &audit.history_tree_id)?;
    let records = projection::move_records(
        view,
        &audit.file_id,
        &audit.events,
        &audit.path_changes,
    )?;
    CapturedMaterial::capture(records, &audit.file_id, &audit.history_tree_id, state.sequence)?
        .encode()
}

/// Preserve the complete source audit prefix and append the proposed move's
/// own D. The normal published-State decoder still requires a real receipt;
/// this narrowly scoped encoder does not relax that admission rule.
pub(crate) fn prepare_move_source(
    previous: &crate::State,
    next: &crate::State,
    node: &crate::FileDeletionNode,
) -> Result<PreparedFileMaterial> {
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
    CapturedMaterial::capture(records, &node.file_id, &node.history_tree_id, next.sequence)?
        .encode()
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

/// One captured FILE's canonical rows and local identity. This material does
/// not prove a disk owner, a complete published inventory or an installation.
/// Encoding consumes the same rows and digest without another State projection.
struct CapturedMaterial {
    records: Vec<Record>,
    file: String,
    history: String,
    sequence: u64,
    records_hash: String,
}

impl CapturedMaterial {
    fn from_projection(view: StateProjection<'_>, file: &str, history: &str) -> Result<Self> {
        let ProjectedRecords { records, .. } = validated_records(view, file, history)?;
        Self::from_validated_records(
            records,
            file.to_owned(),
            history.to_owned(),
            view.state().sequence,
        )
    }

    fn capture(records: Vec<Record>, file: &str, history: &str, sequence: u64) -> Result<Self> {
        validate_records(&records, file, history, sequence)?;
        Self::from_validated_records(records, file.to_owned(), history.to_owned(), sequence)
    }

    /// Projection admission or streaming SQLite admission has already closed
    /// these exact rows. Neither path repeats that admission while capturing.
    fn from_validated_records(
        records: Vec<Record>,
        file: String,
        history: String,
        sequence: u64,
    ) -> Result<Self> {
        let records_hash = storage_contract::object_hash("file-local-records", &records)?;
        Ok(Self {
            records,
            file,
            history,
            sequence,
            records_hash,
        })
    }

    fn encode(self) -> Result<PreparedFileMaterial> {
        let prepared = SCHEMA.encode(
            self.records,
            [&self.file, &self.history],
            self.sequence,
            &self.records_hash,
        )?;
        Ok(PreparedFileMaterial {
            bytes: prepared.bytes,
            file: self.file,
            history: self.history,
            sequence: self.sequence,
            records_hash: self.records_hash,
        })
    }
}

fn verify_snapshot(bytes: &[u8]) -> Result<CapturedMaterial> {
    if bytes.len() > MAX_BYTES {
        return Err(SCHEMA.budget());
    }
    crate::sqlite_connection::read_image(bytes, MAX_BYTES as u32, verify_connection)
}
async fn verify_connection(connection: &mut SqliteConnection) -> Result<CapturedMaterial> {
    SCHEMA.verify_schema(connection).await?;
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
    let mut validation = rows::Validation::new(&file, &history, sequence);
    let records = SCHEMA
        .read_records(connection, |record| {
            validation.admit(record.kind, &record.id, &record.body)
        })
        .await?;
    validation.finish()?;
    let material = CapturedMaterial::from_validated_records(records, file, history, sequence)?;
    if material.records_hash != owner.try_get::<String, _>("records_hash")? {
        return Err(invalid(
            "file database records differ from their recorded digest",
        ));
    }
    crate::sqlite_integrity::verify(connection).await?;
    Ok(material)
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/container_storage/file_database_tests.rs"]
mod tests;
