//! Retained receipt/path rows and global business maps in the system SQLite projection.
//! This is rebuilt from admitted State, never a publication or recovery authority.
use super::*;
use data_serialization::{Serialize, de::DeserializeOwned};
use std::collections::BTreeSet;

mod audit;
mod catalog;
mod prepared;
mod read;
pub use audit::{SystemAuditEvent, SystemOperationPrincipal, SystemStateMetadata};
pub use catalog::{SystemCatalogKind, SystemCatalogRecord, SystemCatalogValue};
pub(super) use prepared::verify_prepared;

pub(super) const TABLE: &str = "CREATE TABLE system_changes(kind INTEGER NOT NULL CHECK(kind BETWEEN 0 AND 38),id TEXT NOT NULL COLLATE BINARY,sequence_key BLOB NOT NULL CHECK(length(sequence_key)=8),payload TEXT NOT NULL,PRIMARY KEY(kind,id)) STRICT";
pub(super) const LOOKUP: &str =
    "CREATE INDEX system_change_order ON system_changes(kind,sequence_key,id)";
const MAX_PAGE_ROWS: usize = 100;
const MAX_PAGE_BYTES: usize = crate::disk::MAX_METADATA;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Operation,
    FilePath,
    Catalog(SystemCatalogKind),
    Audit(audit::Kind),
}
impl Kind {
    fn sql(self) -> i64 {
        match self {
            Self::Operation => 0,
            Self::FilePath => 1,
            Self::Catalog(kind) => kind.sql(),
            Self::Audit(kind) => kind.sql(),
        }
    }
}

/// A continuation bound to one admitted State and one system-wide row family.
#[derive(Clone, Debug)]
pub struct SystemChangeCursor {
    sequence: u64,
    kind: Kind,
    after: (u64, String),
    generation: Arc<()>,
}

/// At most 100 rows and 64 MiB canonical payload bytes, in sequence/ID order.
#[derive(Debug)]
pub struct SystemChangePage<T> {
    pub sequence: u64,
    pub records: Vec<T>,
    pub next: Option<SystemChangeCursor>,
}

/// Invalid/stale requests are distinct from a disabled projection. The latter
/// retains its original cause in both this error and Engine::index_health.
#[derive(Debug, error_definition::Error)]
pub enum SystemChangeReadError {
    #[error(transparent)]
    Request(#[from] Error),
    #[error("system change projection is unavailable: {0}")]
    Unavailable(#[source] Arc<IndexFailure>),
}

#[derive(Default)]
pub(super) struct Order {
    operations: BTreeSet<(u64, String)>,
    paths: BTreeSet<(u64, String)>,
    catalog: catalog::CatalogOrder,
    audit: audit::Order,
}
impl Order {
    fn rows(&self, kind: Kind) -> &BTreeSet<(u64, String)> {
        match kind {
            Kind::Operation => &self.operations,
            Kind::FilePath => &self.paths,
            Kind::Catalog(kind) => self.catalog.rows(kind),
            Kind::Audit(kind) => self.audit.rows(kind),
        }
    }
    pub(super) fn update(&mut self, before: Option<&State>, after: &State) {
        let delta = Delta::between(before, after);
        for id in delta.operations {
            if let Some(old) = before.and_then(|state| state.operations.get(&id)) {
                self.operations.remove(&(old.sequence, id.to_string()));
            }
            if let Some(new) = after.operations.get(&id) {
                self.operations.insert((new.sequence, id.to_string()));
            }
        }
        delta.catalog.update_order(&mut self.catalog, after);
        delta.audit.update_order(&mut self.audit, after);
        for id in delta.paths {
            if let Some(old) = before.and_then(|state| state.file_path_nodes.get(&id)) {
                self.paths.remove(&(old.sequence, id.clone()));
            }
            if let Some(new) = after.file_path_nodes.get(&id) {
                self.paths.insert((new.sequence, id));
            }
        }
    }
}

pub(super) struct Delta {
    operations: Vec<OperationId>,
    paths: Vec<String>,
    catalog: catalog::CatalogDelta,
    audit: audit::Delta,
}
impl Delta {
    pub(super) fn between(before: Option<&State>, after: &State) -> Self {
        // Exact write sets remain incremental. Unknown lineage compares the
        // ordered union; neither permanent receipt retention nor tests imply
        // these maps can only append.
        macro_rules! keys {
            ($field:ident) => {{
                match before {
                    None => after.$field.keys().cloned().collect(),
                    Some(before) => match after.$field.changes_since(&before.$field) {
                        MapChanges::Keys(keys) => keys
                            .iter()
                            .filter(|id| before.$field.get(*id) != after.$field.get(*id))
                            .cloned()
                            .collect(),
                        MapChanges::All => {
                            let mut changed = Vec::new();
                            let mut old = before.$field.iter().peekable();
                            for (id, value) in after.$field.iter() {
                                while let Some((id, _)) = old.next_if(|(old, _)| *old < id) {
                                    changed.push(id.clone());
                                }
                                let previous = old.next_if(|(old, _)| *old == id).map(|(_, v)| v);
                                if previous != Some(value) {
                                    changed.push(id.clone());
                                }
                            }
                            changed.extend(old.map(|(id, _)| id.clone()));
                            changed
                        }
                    },
                }
            }};
        }
        Self {
            operations: keys!(operations),
            paths: keys!(file_path_nodes),
            catalog: catalog::CatalogDelta::between(before, after),
            audit: audit::Delta::new(before, after, keys!(operation_principals)),
        }
    }
    pub(super) async fn project(
        &self,
        connection: &mut sqlite_access::SqliteConnection,
        after: &State,
    ) -> std::result::Result<(), ProjectionFailure> {
        for id in &self.operations {
            store_row(
                connection,
                Kind::Operation,
                &id.to_string(),
                after.operations.get(id).map(|row| (row.sequence, row)),
            )
            .await?;
        }
        for id in &self.paths {
            store_row(
                connection,
                Kind::FilePath,
                id,
                after.file_path_nodes.get(id).map(|row| (row.sequence, row)),
            )
            .await?;
        }
        self.catalog.project(connection, after).await?;
        self.audit.project(connection, after).await?;
        Ok(())
    }
}

async fn store_row<T: Serialize>(
    connection: &mut sqlite_access::SqliteConnection,
    kind: Kind,
    id: &str,
    value: Option<(u64, &T)>,
) -> std::result::Result<(), ProjectionFailure> {
    let payload = value
        .map(|(sequence, value)| encode(value).map(|bytes| (sequence, bytes)))
        .transpose()
        .map_err(|source| ProjectionFailure {
            source,
            corrupt: false,
        })?;
    store_payload(
        connection,
        kind,
        id,
        payload
            .as_ref()
            .map(|(sequence, bytes)| (*sequence, bytes.as_slice())),
    )
    .await
}

async fn store_payload(
    connection: &mut sqlite_access::SqliteConnection,
    kind: Kind,
    id: &str,
    value: Option<(u64, &[u8])>,
) -> std::result::Result<(), ProjectionFailure> {
    if let Some((sequence, payload)) = value {
        let payload =
            std::str::from_utf8(payload).expect("invariant: metadata encoder emits UTF-8");
        sqlite_access::query("INSERT INTO system_changes VALUES(?,?,?,?) ON CONFLICT(kind,id) DO UPDATE SET sequence_key=excluded.sequence_key,payload=excluded.payload")
            .bind(kind.sql()).bind(id).bind(sequence.to_be_bytes().as_slice()).bind(payload)
            .execute(&mut *connection).await?;
    } else {
        sqlite_access::query("DELETE FROM system_changes WHERE kind=? AND id=?")
            .bind(kind.sql())
            .bind(id)
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}

fn encode(value: &impl Serialize) -> Result<Vec<u8>> {
    crate::disk::encode_metadata(value, MAX_PAGE_BYTES)
}

pub(super) async fn check_schema(
    connection: &mut sqlite_access::SqliteConnection,
) -> std::result::Result<(), ProjectionFailure> {
    for (name, kind, ddl) in [
        ("system_changes", "table", TABLE),
        ("system_change_order", "index", LOOKUP),
    ] {
        let row = sqlite_access::query("SELECT type,sql FROM sqlite_schema WHERE name=?")
            .bind(name)
            .fetch_one(&mut *connection)
            .await?;
        if row.try_get::<String, _>(0)? != kind || row.try_get::<String, _>(1)? != ddl {
            return Err("system change projection schema mismatch".into());
        }
    }
    let triggers: i64 = sqlite_access::query_scalar(
        "SELECT count(*) FROM sqlite_schema WHERE type='trigger' AND tbl_name='system_changes'",
    )
    .fetch_one(&mut *connection)
    .await?;
    if triggers != 0 {
        return Err("system change projection has an unexpected trigger".into());
    }
    Ok(())
}

pub(super) async fn recreate(
    connection: &mut sqlite_access::SqliteConnection,
) -> std::result::Result<(), ProjectionFailure> {
    let kind: Option<String> =
        sqlite_access::query_scalar("SELECT type FROM sqlite_schema WHERE name='system_changes'")
            .fetch_optional(&mut *connection)
            .await?;
    if kind.as_deref() == Some("view") {
        connection.execute("DROP VIEW system_changes").await?;
    }
    connection
        .execute("DROP TABLE IF EXISTS system_changes")
        .await?;
    connection.execute(TABLE).await?;
    connection.execute(LOOKUP).await?;
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/index/system_changes_tests.rs"]
mod tests;
