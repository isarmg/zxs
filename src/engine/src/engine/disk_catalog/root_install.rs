//! Preserving installation at the ordinary disk-root database names. This
//! receives published bytes from the Backup owner, never chooses a State from
//! SQLite. SYSTEM/current namespace and writer activation remain separate.
use super::*;
use crate::disk::DiskRoot;
use crate::database_install::DISK_HISTORY_DATABASE_NAME;
use crate::history_package::preparation_checkpoint;
use std::sync::atomic::AtomicBool;

pub(in crate::engine) const BACKUP_SELECTION: &str = "backup-root-databases-selected.json";

pub(super) fn require_unselected(root: &DiskRoot) -> Result<()> {
    // Presence is deliberately only a refusal. Corrupt/foreign selector bytes
    // cannot grant a writer, choose a new State, or cause a cache rebuild.
    if root
        .optional_metadata(&root.path().join(BACKUP_SELECTION))?
        .is_some()
    {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "Backup selected these ordinary disk database names; resume its original task before using an active catalog",
        ));
    }
    Ok(())
}

fn require_no_sidecars(root: &DiskRoot) -> Result<()> {
    for name in [DISK_DATABASE_NAME, DISK_HISTORY_DATABASE_NAME] {
        for suffix in ["-wal", "-shm", "-journal"] {
            if root
                .optional_metadata(&root.path().join(format!("{name}{suffix}")))?
                .is_some()
            {
                return Err(invalid(
                    "disk database has a live or unknown SQLite sidecar; all files retained",
                ));
            }
        }
    }
    Ok(())
}

/// The two file exchanges are not an atomic pair. The owner retains a common
/// pending selector and keeps all ordinary catalog consumers fenced until a
/// future complete native activation; a partial pair is never reported ready.
pub(in crate::engine) struct PreparedRootDatabases {
    current: PreparedInstall,
    history: PreparedInstall,
}
impl PreparedRootDatabases {
    pub(in crate::engine) fn capture(
        root: &DiskRoot,
        identity: &DiskRegistrationIdentity,
        bytes: [Vec<u8>; 2],
        sequence: u64,
        admission: InstallAdmission,
    ) -> Result<Self> {
        require_no_sidecars(root)?;
        let [current_bytes, history_bytes] = bytes;
        let current = database::read(&current_bytes, identity)?;
        let history = history::read_for_root_install(&history_bytes, identity)?;
        if current.source_sequence != sequence
            || history.header.through_sequence != sequence
            || history::fold_prepared(&history, Some(&current))? != current
        {
            return Err(invalid(
                "published root databases do not contain one complete identical disk scope",
            ));
        }
        let entity = root.directory_identity(root.path())?;
        let prepared_current = PreparedInstall::capture(
            root,
            root.path(),
            (root.path(), entity),
            (Scope::disk(identity.clone()), admission),
            current_bytes,
            MAX_DATABASE_BYTES,
            |old| {
                let old = database::read(old, identity)?;
                if old.source_sequence > sequence
                    || (old.source_sequence == sequence && old != current)
                {
                    return Err(invalid("existing root catalog claims another publication"));
                }
                Ok(())
            },
        )?;
        let prepared_history = PreparedInstall::capture(
            root,
            root.path(),
            (root.path(), entity),
            (Scope::disk_history(identity.clone()), admission),
            history_bytes,
            MAX_DATABASE_BYTES,
            |old| {
                let old = history::read_for_root_install(old, identity)?;
                history::fold_prepared(&old, None)?;
                if old.header.through_sequence > sequence
                    || (old.header.through_sequence == sequence && old != history)
                {
                    return Err(invalid("existing root history claims another publication"));
                }
                Ok(())
            },
        )?;
        require_no_sidecars(root)?;
        Ok(Self {
            current: prepared_current,
            history: prepared_history,
        })
    }

    /// Check the pair as one retained-install budget before either exchange.
    /// Decided recovery may reuse its own retained attempts, never erase them.
    pub(in crate::engine) fn require_capacity(
        &self,
        root: &DiskRoot,
        admission: InstallAdmission,
        reserve: u64,
    ) -> Result<()> {
        let current = self.current.reservation();
        let history = self.history.reservation();
        let bytes = current.0.checked_add(history.0).ok_or_else(budget)?;
        let entries = current.1.checked_add(history.1).ok_or_else(budget)?;
        admission.check(root, root.path(), bytes, entries)?;
        if bytes.checked_add(reserve).ok_or_else(budget)? > root.available()? {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "Backup root database pair must retain its metadata recovery reserve",
            ));
        }
        Ok(())
    }

    pub(in crate::engine) fn apply(self, root: &DiskRoot, stop: &AtomicBool) -> Result<()> {
        require_no_sidecars(root)?;
        self.current
            .apply(root, &mut |_| preparation_checkpoint(stop))?;
        preparation_checkpoint(stop)?;
        require_no_sidecars(root)?;
        self.history
            .apply(root, &mut |_| preparation_checkpoint(stop))?;
        require_no_sidecars(root)
    }
}

/// Observe bytes after installation without invoking repair or the installer.
pub(in crate::engine) fn require_published_pair(
    root: &DiskRoot,
    identity: &DiskRegistrationIdentity,
    expected: &[Vec<u8>; 2],
    stop: &AtomicBool,
) -> Result<()> {
    require_no_sidecars(root)?;
    for (name, bytes) in [DISK_DATABASE_NAME, DISK_HISTORY_DATABASE_NAME]
        .into_iter()
        .zip(expected)
    {
        preparation_checkpoint(stop)?;
        let actual = crate::disk::HeldRegular::capture(root, name, MAX_DATABASE_BYTES)?
            .ok_or_else(|| invalid("selected ordinary root database is missing"))?;
        if actual.bytes() != bytes {
            return Err(invalid(
                "ordinary root database differs from the exact publication",
            ));
        }
        actual.check_running()?;
    }
    // The exact new bytes were decoded before installation. This separate
    // observer also parses them so callers cannot treat a digest as a scope.
    let current = database::read(&expected[0], identity)?;
    let history = history::read_for_root_install(&expected[1], identity)?;
    if history::fold_prepared(&history, Some(&current))? != current {
        return Err(invalid(
            "ordinary root database pair does not fold to one scope",
        ));
    }
    require_no_sidecars(root)
}
