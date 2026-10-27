//! Rebuildable SQLite projection; never an authority for publication.
//!
//! Startup rebuilds from recovered durable evidence, replacing disposable
//! full-snapshot and generic entity projections without reading their data. Only file-history candidate
//! fields and the publication sequence are stored. Shared, untouched version
//! maps need no comparison; exact transaction write sets compare only changed
//! keys. Cold rebuilds and unknown lineage retain the complete ordered merge.
//! Business objects come from State.
use super::Engine;
use crate::*;
use sqlite_access::{Connection, Executor, Row};
use std::{collections::BTreeMap, path::Path, sync::Arc};
use storage_contract::{MapChanges, TrackedRecords};

const HISTORY_INDEX_MIN_VERSIONS: usize = 300_000;
const HISTORY_INDEX_MAX_CANDIDATES: usize = 100;

const CURRENT_STATE_TABLE: &str = "CREATE TABLE current_state(singleton INTEGER PRIMARY KEY CHECK(singleton=1),sequence TEXT NOT NULL)";
const HISTORY_TABLE: &str = "CREATE TABLE file_history_versions(version_id TEXT PRIMARY KEY COLLATE BINARY,file_id TEXT NOT NULL,history_tree_id TEXT NOT NULL,version_sequence_key BLOB NOT NULL CHECK(typeof(version_sequence_key)='blob' AND length(version_sequence_key)=8))";
const HISTORY_INDEX: &str = "CREATE INDEX file_history_lookup ON file_history_versions(file_id,version_sequence_key,version_id)";

/// Trusted completeness evidence, never loaded from the disposable database.
/// Ready is established by a complete rebuild and maintained under exclusive
/// publication: its sequence matches State, or the previous State during update.
#[derive(Default)]
pub(super) struct QueryProof {
    sequence: u64,
    counts: BTreeMap<String, usize>,
    availability: IndexAvailability,
    #[cfg(test)]
    indexed_reads: usize,
}

/// Availability of the optional history accelerator, independent of publication.
#[derive(Clone, Debug)]
pub enum IndexHealth {
    Unbuilt,
    Ready { sequence: u64 },
    Unavailable(Arc<IndexFailure>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexFailureStage {
    Read,
    Update,
}

/// The first failure retained until a complete rebuild succeeds.
#[derive(Debug, error_definition::Error)]
#[error("history index {stage:?} failed at sequence {sequence}: {source}")]
pub struct IndexFailure {
    stage: IndexFailureStage,
    sequence: u64,
    #[source]
    source: Error,
}
impl IndexFailure {
    pub fn stage(&self) -> IndexFailureStage {
        self.stage
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn cause(&self) -> &Error {
        &self.source
    }
}

#[derive(Default)]
enum IndexAvailability {
    #[default]
    Unbuilt,
    Ready(IndexConnection),
    Unavailable(Arc<IndexFailure>),
}

impl QueryProof {
    fn disable(&mut self, source: Error, stage: IndexFailureStage, sequence: u64) {
        let source = match std::mem::take(&mut self.availability) {
            IndexAvailability::Ready(mut writer) => match writer.close() {
                Ok(()) => source,
                Err(close) => preserve_close_failure(source, close.source),
            },
            IndexAvailability::Unbuilt => source,
            IndexAvailability::Unavailable(_) => {
                unreachable!("invariant: an unavailable index is never accessed")
            }
        };
        let failure = Arc::new(IndexFailure {
            stage,
            sequence,
            source,
        });
        // Stop using the optional projection and retain the concrete cause;
        // application health reporting is separate from mutation receipts.
        self.availability = IndexAvailability::Unavailable(failure);
    }
}

fn preserve_close_failure(operation: Error, close: Error) -> Error {
    Error::Cleanup {
        operation: Box::new(operation),
        cleanup: std::io::Error::other(close),
    }
}

/// Only fields consumed by the candidate query participate in this delta.
/// IDs borrow the frozen states; no entity JSON or second State is retained.
#[derive(Debug, Default, PartialEq)]
struct HistoryDelta<'a> {
    upserts: Vec<&'a str>,
    deletes: Vec<&'a str>,
}

#[cfg(test)]
thread_local! {
    static HISTORY_DELTA_AFTER_ROWS_COMPARED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl<'a> HistoryDelta<'a> {
    fn between(
        before: Option<&'a TrackedRecords<FileVersion>>,
        after: &'a TrackedRecords<FileVersion>,
    ) -> Self {
        let mut delta = Self::default();
        let Some(before) = before else {
            delta.upserts.extend(after.keys().map(String::as_str));
            return delta;
        };
        // FileVersion has no interior mutability. Shared backing therefore
        // proves these records are unchanged; distinct backing proves nothing.
        // Only comparison is skipped: the caller still validates and advances
        // the projection marker and trusted query proof.
        if before.shares_storage_with(after) {
            return delta;
        }
        if let MapChanges::Keys(keys) = after.changes_since(before) {
            for key in keys.iter() {
                if let Some((id, value)) = after.get_key_value(key) {
                    #[cfg(test)]
                    HISTORY_DELTA_AFTER_ROWS_COMPARED.with(|rows| rows.set(rows.get() + 1));
                    if before.get(key).is_none_or(|old| {
                        old.file_id != value.file_id
                            || old.history_tree_id != value.history_tree_id
                            || old.version_sequence != value.version_sequence
                    }) {
                        delta.upserts.push(id);
                    }
                } else if let Some((id, _)) = before.get_key_value(key) {
                    delta.deletes.push(id);
                }
            }
            return delta;
        }
        // Merge the ordered maps once, preserving each output vector's key
        // order without two tree searches per unchanged row.
        let mut before = before.iter().peekable();
        for (id, value) in after.iter() {
            #[cfg(test)]
            HISTORY_DELTA_AFTER_ROWS_COMPARED.with(|rows| rows.set(rows.get() + 1));
            while let Some((old_id, _)) = before.next_if(|(old_id, _)| *old_id < id) {
                delta.deletes.push(old_id);
            }
            let previous = before
                .next_if(|(old_id, _)| *old_id == id)
                .map(|(_, value)| value);
            if previous.is_none_or(|old| {
                old.file_id != value.file_id
                    || old.history_tree_id != value.history_tree_id
                    || old.version_sequence != value.version_sequence
            }) {
                delta.upserts.push(id);
            }
        }
        delta.deletes.extend(before.map(|(id, _)| id.as_str()));
        delta
    }
}

#[cfg(test)]
fn project(path: &Path, before: Option<&State>, after: &State) -> Result<()> {
    let delta = HistoryDelta::between(before.map(|state| &state.versions), &after.versions);
    project_delta(path, before, after, &delta)
}

#[derive(Debug)]
struct ProjectionFailure {
    source: Error,
    corrupt: bool,
}
impl ProjectionFailure {
    fn after_close(self, closed: std::result::Result<(), Self>) -> Self {
        match closed {
            Ok(()) => self,
            Err(close) => Self {
                source: preserve_close_failure(self.source, close.source),
                corrupt: false,
            },
        }
    }
}

impl From<sqlite_access::Error> for ProjectionFailure {
    fn from(error: sqlite_access::Error) -> Self {
        // Extended SQLite codes retain their primary error in the low byte.
        // Never turn permission, I/O, busy, or schema errors into file replacement.
        let corrupt = error
            .as_database_error()
            .and_then(|error| error.code())
            .and_then(|code| code.parse::<i32>().ok())
            .is_some_and(|code| matches!(code & 0xff, 11 | 26));
        Self {
            source: error.into(),
            corrupt,
        }
    }
}
impl From<platform_io::sqlite::Error> for ProjectionFailure {
    fn from(error: platform_io::sqlite::Error) -> Self {
        match error {
            platform_io::sqlite::Error::Sqlx(error) => error.into(),
            error => Self {
                source: error.into(),
                corrupt: false,
            },
        }
    }
}
impl From<&str> for ProjectionFailure {
    fn from(message: &str) -> Self {
        Self {
            source: Error::new(ErrorCode::IndexRebuildFailed, message),
            corrupt: false,
        }
    }
}
impl From<ProjectionFailure> for Error {
    fn from(error: ProjectionFailure) -> Self {
        error.source
    }
}

/// The engine owns the worker until rebuild, failure, or drop. FULL commit
/// confirms this projection transaction, not authoritative publication.
struct IndexConnection {
    connection: Option<sqlite_access::SqliteConnection>,
    file: std::fs::File,
}

impl IndexConnection {
    fn open(path: &Path, create: bool) -> std::result::Result<Self, ProjectionFailure> {
        let mut connection = platform_io::sqlite::block_on_sqlite_connection(
            platform_io::sqlite::open_index_connection(path, create, None),
        )?;
        if let Err(error) = platform_io::sqlite::block_on_sqlite_connection(
            platform_io::sqlite::disable_checkpoint_on_close(&mut connection),
        ) {
            let closed = platform_io::sqlite::block_on_sqlite_connection(connection.close())
                .map_err(ProjectionFailure::from);
            return Err(ProjectionFailure {
                source: sqlite_access::Error::Io(std::io::Error::other(error)).into(),
                corrupt: false,
            }
            .after_close(closed));
        }
        let file = match crate::disk::open_regular(path) {
            Ok(file) => file,
            Err(error) => {
                let closed = platform_io::sqlite::block_on_sqlite_connection(connection.close())
                    .map_err(ProjectionFailure::from);
                return Err(ProjectionFailure {
                    source: error,
                    corrupt: false,
                }
                .after_close(closed));
            }
        };
        Ok(Self {
            connection: Some(connection),
            file,
        })
    }

    fn check_identity(&self, path: &Path) -> Result<()> {
        use platform_io::fs_safety::linux::FileIdentity;
        let current = crate::disk::open_regular(path)?;
        if FileIdentity::from_fd(&self.file)? != FileIdentity::from_fd(&current)? {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "index identity changed while connected",
            ));
        }
        Ok(())
    }

    fn close(&mut self) -> std::result::Result<(), ProjectionFailure> {
        if let Some(connection) = self.connection.take() {
            // A close error must never authorize quarantine.
            platform_io::sqlite::block_on_sqlite_connection(connection.close()).map_err(
                |error| ProjectionFailure {
                    source: error.into(),
                    corrupt: false,
                },
            )?;
        }
        Ok(())
    }

    fn project(
        &mut self,
        before: Option<&State>,
        after: &State,
        delta: &HistoryDelta<'_>,
    ) -> std::result::Result<(), ProjectionFailure> {
        let c = self
            .connection
            .as_mut()
            .expect("invariant: admitted index worker is open");
        let result = platform_io::sqlite::block_on_sqlite_connection(async {
            // Preserve the former reopen boundary for disk reads. Cached b-tree
            // pages must not hide in-place damage behind a valid schema cookie.
            // This releases page memory, not the worker or prepared statements.
            c.execute("PRAGMA shrink_memory")
                .await
                .map_err(ProjectionFailure::from)?;
            let mut tx = c.begin().await.map_err(ProjectionFailure::from)?;
            if let Some(before) = before {
                check_projection_marker(&mut tx, before.sequence).await?;
            } else {
                recreate_projection(&mut tx).await?;
            }
            for id in &delta.deletes {
                sqlite_access::query("DELETE FROM file_history_versions WHERE version_id=?")
                    .bind(*id)
                    .execute(&mut *tx)
                    .await
                    .map_err(ProjectionFailure::from)?;
            }
            for id in &delta.upserts {
                let v = &after.versions[*id];
                sqlite_access::query("INSERT INTO file_history_versions VALUES(?,?,?,?) ON CONFLICT(version_id) DO UPDATE SET file_id=excluded.file_id,history_tree_id=excluded.history_tree_id,version_sequence_key=excluded.version_sequence_key")
                .bind(*id).bind(&v.file_id).bind(&v.history_tree_id).bind(v.version_sequence.to_be_bytes().as_slice())
                .execute(&mut *tx).await.map_err(ProjectionFailure::from)?;
            }
            // Metadata-only publications still advance the marker in this transaction.
            sqlite_access::query("INSERT INTO current_state VALUES(1,?) ON CONFLICT(singleton) DO UPDATE SET sequence=excluded.sequence")
            .bind(after.sequence.to_string()).execute(&mut *tx).await.map_err(ProjectionFailure::from)?;
            tx.commit().await.map_err(ProjectionFailure::from)?;
            Ok::<(), ProjectionFailure>(())
        });
        result.map_err(|error| error.after_close(self.close()))
    }
}

async fn check_projection_marker(
    connection: &mut sqlite_access::SqliteConnection,
    expected_sequence: u64,
) -> std::result::Result<(), ProjectionFailure> {
    // This runs after durable publication. Never evaluate a cache-supplied
    // view while checking the marker. Failure disables this projection, not
    // the independently completed authoritative publication.
    let marker =
        sqlite_access::query("SELECT type,sql FROM sqlite_schema WHERE name='current_state'")
            .fetch_one(&mut *connection)
            .await
            .map_err(ProjectionFailure::from)?;
    if marker
        .try_get::<String, _>(0)
        .map_err(ProjectionFailure::from)?
        != "table"
        || marker
            .try_get::<String, _>(1)
            .map_err(ProjectionFailure::from)?
            != CURRENT_STATE_TABLE
    {
        return Err(
            "projection marker schema mismatch; reopen to rebuild from durable evidence".into(),
        );
    }
    let sequence: Option<String> =
        sqlite_access::query_scalar("SELECT sequence FROM current_state WHERE singleton=1")
            .fetch_optional(&mut *connection)
            .await
            .map_err(ProjectionFailure::from)?;
    if sequence.as_deref() != Some(expected_sequence.to_string().as_str()) {
        return Err("projection sequence mismatch; reopen to rebuild from durable evidence".into());
    }
    Ok(())
}

async fn recreate_projection(
    connection: &mut sqlite_access::SqliteConnection,
) -> std::result::Result<(), ProjectionFailure> {
    // Transactional replacement removes both obsolete full-State layouts.
    // A damaged cache may have substituted views for our tables.
    for (name, drop_view) in [
        ("current_state", "DROP VIEW current_state"),
        ("entities", "DROP VIEW entities"),
        ("file_history_versions", "DROP VIEW file_history_versions"),
    ] {
        let kind: Option<String> =
            sqlite_access::query_scalar("SELECT type FROM sqlite_schema WHERE name=?")
                .bind(name)
                .fetch_optional(&mut *connection)
                .await
                .map_err(ProjectionFailure::from)?;
        if kind.as_deref() == Some("view") {
            connection
                .execute(drop_view)
                .await
                .map_err(ProjectionFailure::from)?;
        }
    }
    connection.execute("DROP TABLE IF EXISTS current_state;DROP TABLE IF EXISTS entities;DROP TABLE IF EXISTS file_history_versions;")
                .await
                .map_err(ProjectionFailure::from)?;
    connection
        .execute(CURRENT_STATE_TABLE)
        .await
        .map_err(ProjectionFailure::from)?;
    connection
        .execute(HISTORY_TABLE)
        .await
        .map_err(ProjectionFailure::from)?;
    connection
        .execute(HISTORY_INDEX)
        .await
        .map_err(ProjectionFailure::from)?;
    Ok(())
}

impl Drop for IndexConnection {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            tracing::warn!(error = %error.source, "index worker shutdown failed");
        }
    }
}

#[cfg(test)]
fn project_delta(
    path: &Path,
    before: Option<&State>,
    after: &State,
    delta: &HistoryDelta<'_>,
) -> Result<()> {
    let mut writer = IndexConnection::open(path, before.is_none()).map_err(Error::from)?;
    let result = writer.project(before, after, delta);
    let closed = writer.close();
    match result {
        Ok(()) => closed.map_err(Error::from),
        Err(error) => Err(error.after_close(closed).into()),
    }
}

// Move every journal before the main database. A partial quarantine must never
// leave old WAL bytes next to a missing database that a later open could create.
const INDEX_FILES: [&str; 4] = [
    "index.sqlite-wal",
    "index.sqlite-shm",
    "index.sqlite-journal",
    "index.sqlite",
];

fn checked_index_files(root: &Path) -> Result<Vec<(&'static str, std::fs::File)>> {
    let mut files = Vec::new();
    for name in INDEX_FILES {
        match std::fs::symlink_metadata(root.join(name)) {
            Ok(_) => files.push((name, crate::disk::open_regular(&root.join(name))?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(files)
}

/// Preserve the corrupt cache and any journals remaining after SQLite closes.
/// This is recoverable diagnostic material, not a forensic pre-open snapshot:
/// SQLite may already have recovered, checkpointed or removed sidecars during
/// cleanup. These files are never recovery authority; durable evidence is untouched.
/// The engine holds its exclusive root lock and no connection may remain open.
/// Reject special files/links instead of following them or overwriting targets.
fn quarantine_corrupt_index(root: &Path) -> Result<()> {
    use platform_io::fs_safety::linux::{FileIdentity, MountPolicy, OpenAt2Root};
    let files = checked_index_files(root)?;
    let source = OpenAt2Root::open(root, MountPolicy::SameMount).map_err(|error| {
        let message = error.to_string();
        Error::from(error).context(storage_contract::ErrorCode::UnsafePath, message)
    })?;
    // Keep from the outset: even a partial move must never be auto-deleted.
    let quarantine = tempfile::Builder::new()
        .prefix("index-corrupt-")
        .tempdir_in(root)?
        .keep();
    let destination = OpenAt2Root::open(&quarantine, MountPolicy::SameMount).map_err(|error| {
        let message = error.to_string();
        Error::from(error).context(storage_contract::ErrorCode::UnsafePath, message)
    })?;
    for (name, file) in files {
        let current = crate::disk::open_regular(&root.join(name))?;
        if FileIdentity::from_fd(&file)? != FileIdentity::from_fd(&current)? {
            return Err(Error::new(
                storage_contract::ErrorCode::UnsafePath,
                "index identity changed before quarantine",
            ));
        }
        rustix::fs::renameat_with(
            source.directory(),
            name,
            destination.directory(),
            name,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(std::io::Error::from)?;
        let moved = crate::disk::open_regular(&quarantine.join(name))?;
        if FileIdentity::from_fd(&file)? != FileIdentity::from_fd(&moved)? {
            return Err(Error::new(
                storage_contract::ErrorCode::UnsafePath,
                "index identity changed during quarantine",
            ));
        }
    }
    destination.directory().sync_all()?;
    source.directory().sync_all()?;
    Ok(())
}

impl Engine {
    /// A cheap snapshot; neither availability nor sequence grants write admission.
    pub fn index_health(&self) -> IndexHealth {
        let proof = self
            .history_query_proof
            .lock()
            .expect("invariant: history query proof lock poisoned");
        match &proof.availability {
            IndexAvailability::Unbuilt => IndexHealth::Unbuilt,
            IndexAvailability::Ready(_) => IndexHealth::Ready {
                sequence: proof.sequence,
            },
            IndexAvailability::Unavailable(failure) => {
                IndexHealth::Unavailable(Arc::clone(failure))
            }
        }
    }

    pub(crate) fn rebuild_index(&self) -> Result<()> {
        let mut proof = self
            .history_query_proof
            .lock()
            .expect("invariant: history query proof lock poisoned");
        match std::mem::take(&mut proof.availability) {
            IndexAvailability::Ready(mut writer) => writer.close().map_err(Error::from)?,
            unavailable @ IndexAvailability::Unavailable(_) => proof.availability = unavailable,
            IndexAvailability::Unbuilt => (),
        }
        let delta = HistoryDelta::between(None, &self.state.versions);
        checked_index_files(&self.root)?;
        let path = self.root.join("index.sqlite");
        let rebuild = || {
            let mut writer = IndexConnection::open(&path, true)?;
            writer.project(None, &self.state, &delta)?;
            Ok::<_, ProjectionFailure>(writer)
        };
        let writer = match rebuild() {
            Ok(writer) => writer,
            Err(error) => {
                if !error.corrupt {
                    return Err(error.into());
                }
                quarantine_corrupt_index(&self.root)?;
                // Only recovered durable State supplies the replacement projection.
                rebuild().map_err(Error::from)?
            }
        };
        proof.counts.clear();
        for v in self.state.versions.values() {
            *proof.counts.entry(v.file_id.clone()).or_default() += 1;
        }
        proof.sequence = self.state.sequence;
        proof.availability = IndexAvailability::Ready(writer);
        Ok(())
    }

    /// Maintain an optional projection after authoritative publication. Its failure
    /// is retained in index_health, never returned as failure of the published write.
    pub(crate) fn update_index(&self, previous: &State) {
        let mut proof = self
            .history_query_proof
            .lock()
            .expect("invariant: history query proof lock poisoned");
        match &proof.availability {
            IndexAvailability::Unavailable(_) => return,
            IndexAvailability::Unbuilt => {
                unreachable!("invariant: engine admission builds its history index")
            }
            IndexAvailability::Ready(_) => (),
        }
        let delta = HistoryDelta::between(Some(&previous.versions), &self.state.versions);
        let result = (|| {
            checked_index_files(&self.root)?;
            let IndexAvailability::Ready(writer) = &mut proof.availability else {
                unreachable!("invariant: index availability is held by this mutex")
            };
            writer.check_identity(&self.root.join("index.sqlite"))?;
            writer
                .project(Some(previous), &self.state, &delta)
                .map_err(Error::from)
        })();
        if let Err(source) = result {
            proof.disable(source, IndexFailureStage::Update, self.state.sequence);
            return;
        }
        // Reuse the changed-version delta, including replacements/moves.
        for id in delta.deletes.iter().chain(&delta.upserts) {
            if let Some(old) = previous.versions.get(*id) {
                let count = proof
                    .counts
                    .get_mut(&old.file_id)
                    .expect("invariant: trusted version count includes the previous version");
                *count -= 1;
                if *count == 0 {
                    proof.counts.remove(&old.file_id);
                }
            }
            if let Some(new) = self.state.versions.get(*id) {
                *proof.counts.entry(new.file_id.clone()).or_default() += 1;
            }
        }
        proof.sequence = self.state.sequence;
    }

    pub(crate) fn history_index_is_selective(&self, file_id: &str) -> bool {
        // Connection/validation costs dominate small maps and large result sets.
        // Conservative measured threshold; this is not an API response limit.
        self.state.versions.len() >= HISTORY_INDEX_MIN_VERSIONS && {
            let proof = self
                .history_query_proof
                .lock()
                .expect("invariant: history query proof lock poisoned");
            matches!(proof.availability, IndexAvailability::Ready(_))
                && proof.counts.get(file_id).copied().unwrap_or(0) <= HISTORY_INDEX_MAX_CANDIDATES
        }
    }

    /// Candidate selection only. Every field comes from State, never SQLite JSON.
    pub(crate) fn indexed_file_history(&self, file_id: &str) -> Option<Vec<FileVersion>> {
        let mut proof = self
            .history_query_proof
            .lock()
            .expect("invariant: history query proof lock poisoned");
        if !matches!(proof.availability, IndexAvailability::Ready(_)) {
            return None;
        }
        if let Err(source) = checked_index_files(&self.root) {
            proof.disable(source, IndexFailureStage::Read, self.state.sequence);
            return None;
        }
        let expected = proof.counts.get(file_id).copied().unwrap_or(0);
        let limit = expected as i64 + 1;
        let result = platform_io::sqlite::block_on_sqlite_connection(async {
            let mut c =
                platform_io::sqlite::open_index_read_connection(&self.root.join("index.sqlite"))
                    .await
                    .map_err(ProjectionFailure::from)?;
            let result = async {
                let mut tx = c.begin().await.map_err(ProjectionFailure::from)?;
                // Attest all queried objects before evaluating their contents: in
                // particular, a cache must not substitute an expensive view.
                for (name, kind, ddl) in [
                    ("current_state", "table", CURRENT_STATE_TABLE),
                    ("file_history_versions", "table", HISTORY_TABLE),
                    ("file_history_lookup", "index", HISTORY_INDEX),
                ] {
                    let actual = sqlite_access::query("SELECT type,sql FROM sqlite_schema WHERE name=?")
                        .bind(name).fetch_one(&mut *tx).await.map_err(ProjectionFailure::from)?;
                    if actual.try_get::<String, _>(0).map_err(ProjectionFailure::from)? != kind
                        || actual.try_get::<String, _>(1).map_err(ProjectionFailure::from)? != ddl {
                        return Err("query schema mismatch".into());
                    }
                }
                let sequence: String = sqlite_access::query_scalar("SELECT sequence FROM current_state WHERE singleton=1")
                    .fetch_one(&mut *tx).await.map_err(ProjectionFailure::from)?;
                if sequence != self.state.sequence.to_string() { return Err("stale sequence".into()); }
                let rows = sqlite_access::query("SELECT version_id,file_id,history_tree_id,version_sequence_key FROM file_history_versions INDEXED BY file_history_lookup WHERE file_id=? ORDER BY version_sequence_key,version_id LIMIT ?")
                    .bind(file_id).bind(limit).fetch_all(&mut *tx).await.map_err(ProjectionFailure::from)?;
                if rows.len() != expected { return Err("incomplete candidate set".into()); }
                let mut versions = Vec::with_capacity(expected);
                let mut prior: Option<(u64, String)> = None;
                for row in rows {
                    let id: String = row.try_get(0).map_err(ProjectionFailure::from)?;
                    let file: String = row.try_get(1).map_err(ProjectionFailure::from)?;
                    let history: String = row.try_get(2).map_err(ProjectionFailure::from)?;
                    let key: Vec<u8> = row.try_get(3).map_err(ProjectionFailure::from)?;
                    let v = self.state.versions.get(&id).ok_or("unknown candidate")?;
                    if file != file_id || v.file_id != file || v.history_tree_id != history || v.version_id != id || key != v.version_sequence.to_be_bytes() {
                        return Err("invalid candidate".into());
                    }
                    let order = (v.version_sequence, id);
                    if prior.as_ref().is_some_and(|p| p >= &order) { return Err("invalid candidate order".into()); }
                    prior = Some(order);
                    versions.push(v.clone());
                }
                tx.commit().await.map_err(ProjectionFailure::from)?;
                Ok::<_, ProjectionFailure>(versions)
            }.await;
            let closed = c.close().await.map_err(ProjectionFailure::from);
            match (result, closed) {
                (Ok(v), Ok(())) => Ok(v),
                (Err(error), closed) => Err(error.after_close(closed)),
                (Ok(_), Err(error)) => Err(error),
            }
        });
        match result {
            Ok(versions) => {
                #[cfg(test)]
                {
                    proof.indexed_reads += 1;
                }
                Some(versions)
            }
            Err(error) => {
                proof.disable(error.into(), IndexFailureStage::Read, self.state.sequence);
                None
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/index/tests.rs"]
mod tests;
