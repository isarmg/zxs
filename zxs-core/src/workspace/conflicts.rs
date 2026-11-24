//! Explicitly release the local slot of a conflicted original, without replacing its evidence.
use super::{Basis, Original, Resolution, Workspace, checked_schema, db_basis, held_lease_at};
use crate::{
    Result, invalid,
    r3::{Id, Intent, Lookup},
};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use std::sync::Arc;

pub(super) const ARCHIVED: &str = "archived_conflict";
pub(super) const ARCHIVE_VERSION: i64 = 2;

// Introduced only when the user explicitly archives the first conflict. Keep
// v1 readable without upgrading a normal checkout, a scan or a receipt lookup.
const SCHEMA: &str = "
CREATE TABLE archived_original(generation INTEGER PRIMARY KEY CHECK(generation>0),
  op TEXT NOT NULL UNIQUE, container TEXT NOT NULL, path TEXT NOT NULL, record TEXT NOT NULL, receipt TEXT) STRICT;
CREATE INDEX archived_container ON archived_original(container, generation);
CREATE INDEX archived_path ON archived_original(path);
CREATE TABLE recovered_checkout(path TEXT PRIMARY KEY, container TEXT NOT NULL,
  generation INTEGER NOT NULL CHECK(generation>0)) STRICT, WITHOUT ROWID;
-- An older process may already be inside prepare when the version changes.
-- Fence the durable row, even if it missed the new application gate.
CREATE TRIGGER guard_recovered_original BEFORE INSERT ON original
WHEN EXISTS(SELECT 1 FROM archived_original AS old WHERE old.path=NEW.path OR old.op=NEW.op)
  OR EXISTS(
    SELECT 1 FROM archived_original AS old WHERE old.container=NEW.container AND NOT EXISTS(
      SELECT 1 FROM recovered_checkout AS selected
      WHERE selected.path=NEW.path AND selected.container=NEW.container
        AND selected.generation=(
          SELECT MAX(latest.generation) FROM archived_original AS latest WHERE latest.container=NEW.container
        )
    )
  )
BEGIN SELECT RAISE(ABORT, 'an archived conflict requires a new verified checkout'); END;
";

/// Exact, workspace-bound evidence shown before releasing a local conflict.
pub struct ConflictArchiveChoice {
    owner: Arc<()>,
    original: Original,
}

impl ConflictArchiveChoice {
    #[must_use]
    pub fn intent(&self) -> &Intent {
        &self.original.intent
    }

    #[must_use]
    pub fn working_path(&self) -> &str {
        &self.original.path
    }
}

impl Workspace {
    /// Select but do not change one recorded conflict. The sealed bytes need
    /// not still be available to retain the operation identity.
    ///
    /// # Errors
    /// Refuses an unverified workspace or anything but an unresolved saved conflict.
    pub fn review_conflict_archive(&self, op: &Id) -> Result<ConflictArchiveChoice> {
        self.validate()?;
        let (original, phase, receipt) = self.original(op)?;
        if phase != "conflict" || receipt.is_some() {
            return Err(invalid("only a saved unresolved conflict may be archived"));
        }
        Ok(ConflictArchiveChoice {
            owner: Arc::clone(&self.read_session_owner),
            original,
        })
    }

    /// Free only the exact selected path/container slot, retaining the old
    /// receipt identity, frozen-body evidence, working file and old basis.
    /// Neither contacts the server nor attaches working bytes to a new head.
    ///
    /// # Errors
    /// Refuses a canceled, changed or no-longer-conflicted selection or unverified storage.
    pub fn archive_conflict(
        &mut self,
        choice: ConflictArchiveChoice,
        confirmed: bool,
    ) -> Result<()> {
        if !confirmed || !Arc::ptr_eq(&self.read_session_owner, &choice.owner) {
            return Err(invalid(
                "confirm this exact workspace conflict before archiving",
            ));
        }
        self.validate()?;
        let original = choice.original;
        let op = &original.intent.operation_id;
        let lease = self.hold_container(&original.intent.container_id)?;
        let (now, phase, receipt) = self.original(op)?;
        if now != original || phase != "conflict" || receipt.is_some() {
            return Err(invalid(
                "the selected unresolved conflict changed; check its receipt",
            ));
        }
        let root = &self.root;
        let db_id = self.db_id;
        let guard = || held_lease_at(root, db_id, &lease);
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        guard()?;
        if db_basis(&tx, &original.path)? != original.prior {
            return Err(invalid(
                "working-file mapping changed; keep the conflict intact",
            ));
        }
        if checked_schema(&tx)? != ARCHIVE_VERSION {
            tx.execute_batch(SCHEMA)?;
            tx.pragma_update(None, "user_version", ARCHIVE_VERSION)?;
        }
        let raw = serde_json::to_string(&original)?;
        let values = params![
            op.as_str(),
            original.intent.container_id.as_str(),
            &original.path,
            raw
        ];
        if tx.execute(
            "INSERT INTO archived_original(op,container,path,record)
             SELECT op,container,path,record FROM original
             WHERE op=?1 AND container=?2 AND path=?3 AND record=?4 AND phase='conflict' AND receipt IS NULL AND cleaned=0",
            values,
        )? != 1 {
            return Err(invalid("the original no longer owns the selected conflict"));
        }
        guard()?;
        if tx.execute(
            "DELETE FROM original WHERE op=?1 AND container=?2 AND path=?3 AND record=?4
             AND phase='conflict' AND receipt IS NULL AND cleaned=0",
            values,
        )? != 1
        {
            return Err(invalid("the exact conflict was not released"));
        }
        guard()?;
        tx.commit()?;
        root.sync().and_then(|()| guard()).map_err(|error| format!(
            "archive for {op} may already be selected; reopen the original workspace and check archived-conflicts: {error}",
        ))?;
        Ok(())
    }

    /// List original operation IDs retained for explicit historical receipt checks.
    ///
    /// # Errors
    /// Refuses damaged or changed workspace evidence.
    pub fn archived_conflicts(&self) -> Result<Vec<Id>> {
        self.validate()?;
        if !has_archive(&self.db)? {
            return Ok(Vec::new());
        }
        let mut sql = self
            .db
            .prepare("SELECT op FROM archived_original ORDER BY generation")?;
        let rows = sql.query_map([], |r| r.get::<_, String>(0))?;
        let result = rows.map(|value| Id::parse(&value?)).collect();
        self.validate()?;
        result
    }

    pub(super) fn settle_archived(
        &self,
        original: &Original,
        status: Lookup,
        prior: Option<u64>,
        guard: impl Fn() -> Result<()>,
    ) -> Result<Resolution> {
        let op = &original.intent.operation_id;
        guard()?;
        if let Lookup::Confirmed { sequence } = status
            && prior.is_none()
        {
            if self.db.execute(
                "UPDATE archived_original SET receipt=?2 WHERE op=?1 AND record=?3 AND receipt IS NULL",
                params![op.as_str(), sequence.to_string(), serde_json::to_string(original)?],
            )? != 1 {
                return Err(invalid("the historical receipt was not recorded on the exact archive"));
            }
            self.root.sync()?;
        }
        // An old receipt is history only, even if no new path exists or
        // the old working mapping happens to still equal the former basis.
        guard()?;
        Ok(Resolution {
            operation: op.clone(),
            status,
            local_cleanup_pending: false,
        })
    }
}

pub(super) fn has_archive(db: &Connection) -> Result<bool> {
    Ok(checked_schema(db)? == ARCHIVE_VERSION)
}

pub(super) fn archived_id(db: &Connection, op: &Id) -> Result<bool> {
    if !has_archive(db)? {
        return Ok(false);
    }
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM archived_original WHERE op=?1)",
        [op.as_str()],
        |r| r.get(0),
    )?)
}

pub(super) fn archived_path(db: &Connection, path: &str) -> Result<bool> {
    if !has_archive(db)? {
        return Ok(false);
    }
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM archived_original WHERE path=?1)",
        [path],
        |r| r.get(0),
    )?)
}

pub(super) fn latest_generation(db: &Connection, container: &Id) -> Result<i64> {
    if !has_archive(db)? {
        return Ok(0);
    }
    Ok(db.query_row(
        "SELECT generation FROM archived_original WHERE container=?1 ORDER BY generation DESC LIMIT 1",
        [container.as_str()], |r| r.get(0),
    ).optional()?.unwrap_or(0))
}

pub(super) fn require_fresh_path(db: &Connection, path: &str, basis: Option<&Basis>) -> Result<()> {
    if archived_path(db, path)? {
        return Err(invalid(
            "archived conflict keeps this working file as evidence; check out the current head to a new path",
        ));
    }
    if let Some(basis) = basis {
        let generation = latest_generation(db, &basis.container)?;
        if generation != 0 && !db.query_row(
            "SELECT EXISTS(SELECT 1 FROM recovered_checkout WHERE path=?1 AND container=?2 AND generation=?3)",
            params![path, basis.container.as_str(), generation], |r| r.get::<_, bool>(0),
        )? {
            return Err(invalid("this checkout predates the archived conflict; check out the current head to a new path"));
        }
    }
    Ok(())
}

pub(super) fn require_generation(db: &Connection, container: &Id, generation: i64) -> Result<()> {
    if latest_generation(db, container)? != generation {
        return Err(invalid(
            "the conflict archive changed after checkout selection; select the current head again",
        ));
    }
    Ok(())
}

pub(super) fn stamp_checkout(
    db: &Connection,
    path: &str,
    container: &Id,
    generation: i64,
) -> Result<()> {
    require_generation(db, container, generation)?;
    if generation != 0 {
        db.execute(
            "INSERT INTO recovered_checkout(path,container,generation) VALUES(?1,?2,?3)",
            params![path, container.as_str(), generation],
        )?;
    }
    Ok(())
}
