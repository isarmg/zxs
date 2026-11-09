//! One-way suspension of the old SYSTEM projection. Only the original Backup
//! worker receives the closed-connection capability; no marker grants a writer.
use super::*;
use crate::disk::DiskRoot;
use platform_io::fs_safety::linux::FileIdentity;

pub(in crate::engine) const SELECTION: &str = "backup-system-index-selected.json";

/// Created only while the Engine query mutex excludes every synchronous SQL
/// reader and all escaped snapshot leases have gone away. Keeping this token
/// alive does not reopen the projection; the Engine stays suspended until drop.
pub(in crate::engine) struct QuiescentIndex {
    system: FileIdentity,
}
impl QuiescentIndex {
    pub(in crate::engine) fn require_root(&self, root: &DiskRoot) -> Result<()> {
        if root.directory_identity(root.path())? != self.system
            || DiskRoot::open(root.path())?.directory_identity(root.path())? != self.system
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "quiescent SYSTEM index no longer has its original held root",
            ));
        }
        Ok(())
    }
}

fn suspended(sequence: u64) -> IndexAvailability {
    IndexAvailability::Unavailable(Arc::new(IndexFailure {
        stage: IndexFailureStage::Update,
        sequence,
        source: Error::new(
            ErrorCode::RecoveryRequired,
            "SYSTEM projection is reserved for the original Backup alias installation; active Current/writer are not installed",
        ),
    }))
}

fn close_old(proof: &mut QueryProof, sequence: u64) -> Result<()> {
    if Arc::strong_count(&proof.read_leases) != 1 {
        return Err(Error::new(
            ErrorCode::WorkspaceBusy,
            "SYSTEM alias installation is waiting for all retained history read snapshots to close",
        ));
    }
    if proof.backup_quiesced {
        return Ok(());
    }
    // A prior close failure is not proof of quiescence. Reopen the original
    // Engine through its cold recovery boundary instead of silently retrying.
    if matches!(proof.availability, IndexAvailability::Unavailable(_)) {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "unavailable SYSTEM index cannot attest that every old SQLite worker closed",
        ));
    }
    if let IndexAvailability::Ready(mut writer) =
        std::mem::replace(&mut proof.availability, suspended(sequence))
        && let Err(failure) = writer.close()
    {
        proof.availability = IndexAvailability::Unavailable(Arc::new(IndexFailure {
            stage: IndexFailureStage::Update,
            sequence,
            source: failure.source,
        }));
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "SYSTEM index shutdown failed; no alias-installation capability was issued; inspect index_health",
        ));
    }
    // IndexConnection disables checkpoint-on-close. The exact main/WAL/SHM/
    // journal set is retained by the worker, not checkpointed or unlinked here.
    proof.backup_quiesced = true;
    proof.backup_serving = None;
    Ok(())
}

/// Called before ANY startup or explicit rebuild opens SQLite. Marker presence
/// only denies old-State rebuild, including malformed markers; interpretation
/// and recovery require the original native decision and complete target set.
pub(super) fn preserve_selected(
    root: &Path,
    proof: &mut QueryProof,
    sequence: u64,
) -> Result<bool> {
    if proof.backup_serving.is_some() {
        return Ok(false);
    }
    if !proof.backup_quiesced
        && DiskRoot::open(root)?
            .optional_metadata(&root.join(SELECTION))?
            .is_none()
    {
        return Ok(false);
    }
    close_old(proof, sequence)?;
    Ok(true)
}

impl Engine {
    pub(in crate::engine) fn quiesce_backup_system_index(&self) -> Result<QuiescentIndex> {
        let root = DiskRoot::open(&self.root)?;
        let mut proof = self
            .history_query_proof
            .lock()
            .expect("invariant: history query proof lock poisoned");
        close_old(&mut proof, self.state.sequence)?;
        let token = QuiescentIndex {
            system: root.directory_identity(root.path())?,
        };
        token.require_root(&root)?;
        Ok(token)
    }

    /// Reopen only after the final coordinator installed the actual native
    /// Current and retained every disk writer. Historical selection files stay
    /// on disk; their presence cannot grant this exception on the next restart.
    pub(in crate::engine) fn reopen_backup_system_index(
        &self,
        barrier: &crate::engine::backup_preparation::ServingBarrier,
    ) -> Result<()> {
        barrier.require_current(self)?;
        let root = DiskRoot::open(&self.root)?;
        let mut proof = self
            .history_query_proof
            .lock()
            .expect("invariant: history query proof lock poisoned");
        if Arc::strong_count(&proof.read_leases) != 1 {
            return Err(Error::new(
                ErrorCode::WorkspaceBusy,
                "Backup serving index requires all old query snapshots to remain closed",
            ));
        }
        if proof.backup_serving.as_deref() == Some(barrier.authority()) {
            return match &proof.availability {
                IndexAvailability::Ready(writer) if proof.sequence == self.state.sequence => {
                    writer.check_identity(&self.root.join("index.sqlite"))
                }
                _ => Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "Backup serving index lost its admitted current connection",
                )),
            };
        }
        close_old(&mut proof, self.state.sequence)?;
        if barrier.uses_replayed_current() || self.state.sequence != barrier.first_sequence() {
            // Later native Publications have already been fully replayed. The
            // first immutable SQLite copy no longer describes that Current;
            // the ordinary derived-index rebuild may now use this exact State.
            proof.backup_quiesced = false;
            proof.backup_serving = Some(barrier.authority().into());
            drop(proof);
            return self.rebuild_index();
        }
        checked_index_files(&self.root)?;
        for suffix in ["-wal", "-shm", "-journal"] {
            if root
                .optional_metadata(&root.path().join(format!("index.sqlite{suffix}")))?
                .is_some()
            {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "first Backup index has an unexpected SQLite sidecar; retained without opening",
                ));
            }
        }
        let held =
            crate::disk::HeldRegular::capture(&root, "index.sqlite", preparation::MAX_BYTES)?
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::RecoveryRequired,
                        "installed Backup SYSTEM index is missing",
                    )
                })?;
        if held.stamp().digest != barrier.system_database_hash() {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "installed Backup SYSTEM index differs from its complete first Publication",
            ));
        }
        let mut writer =
            IndexConnection::open(&self.root.join("index.sqlite"), false).map_err(Error::from)?;
        let actual = FileIdentity::from_fd(&writer.file)?;
        if actual.device != held.stamp().device || actual.inode != held.stamp().inode {
            writer.close().map_err(Error::from)?;
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "Backup SYSTEM index name changed while reopening its completed inode",
            ));
        }
        writer.check_identity(&self.root.join("index.sqlite"))?;
        held.check_running()?;
        proof.read_generation = Arc::new(());
        proof.system_order = system_changes::Order::default();
        proof.system_order.update(None, &self.state);
        proof.scope_counts.clear();
        scope::Delta::between(None, &self.state).apply_counts(
            &mut proof.scope_counts,
            None,
            &self.state,
        );
        proof.counts.clear();
        for version in self.state.versions.values() {
            *proof.counts.entry(version.file_id.clone()).or_default() += 1;
        }
        proof.sequence = self.state.sequence;
        proof.backup_quiesced = false;
        proof.backup_serving = Some(barrier.authority().into());
        proof.availability = IndexAvailability::Ready(writer);
        Ok(())
    }
}
