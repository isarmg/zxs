//! Exact registry graph after the original Backup first publication. No
//! Engine/namespace installation, disk registration I/O or writer capability.
use super::*;
use crate::history_package::AdmittedState;

impl PreparedDiskRegistry {
    /// Read both canonical registry generations from the same target. The
    /// caller's reader has a cumulative budget; no in-memory object is a
    /// substitute for a missing parent or successor page.
    pub(in crate::engine) fn verify_backup_with(
        &self,
        mut read: impl FnMut(&'static str, &str) -> Result<Vec<u8>>,
    ) -> Result<VerifiedDiskRegistry> {
        let parent =
            self.expected.state.predecessor.as_ref().ok_or_else(|| {
                invalid("Backup registry lacks its exact original registry parent")
            })?;
        load_registry(parent, &mut read)?;
        self.verify_with(read)
    }

    /// The caller first proves the exact original Backup recipe and its actual
    /// Publication. Both full States are native admitted snapshots, not caller
    /// JSON. This constructor retains all original registrations and voters;
    /// the replaced data terminal is not silently disabled or deregistered.
    pub(in crate::engine) fn for_backup_publication(
        original: &ValidatedConfig,
        source: &AdmittedState,
        source_snapshot: &str,
        selected: &ValidatedConfig,
        published: &AdmittedState,
        published_snapshot: &str,
        authority: &str,
    ) -> Result<Self> {
        if original.generation().is_some()
            || !original.same_metadata_membership(selected)
            || source.state().sequence >= published.state().sequence
            || source.state().topology_generation.checked_add(1)
                != Some(published.state().topology_generation)
            || crate::journal::prepare_snapshot(source.state(), MAX_METADATA)?.hash
                != source_snapshot
            || crate::journal::prepare_snapshot(published.state(), MAX_METADATA)?.hash
                != published_snapshot
        {
            return Err(invalid(
                "Backup registry is not bound to its fixed source and first published snapshot",
            ));
        }
        digest(authority)?;
        let base = encode_registry(base_state(original, source.state(), Some(source_snapshot)))?;
        let mut next = base.expected.prepare_continuation(
            original,
            selected,
            continuation::RegistryBoundary {
                authority,
                snapshot: published_snapshot,
                sequence: published.state().sequence,
                topology_generation: published.state().topology_generation,
            },
        )?;
        // Carry the actual parent graph too; a root digest alone is not the
        // old registry, and a later installer must not rebuild a missing page.
        let mut objects = BTreeMap::new();
        let mut bytes = 0usize;
        for object in base.objects.into_iter().chain(next.objects) {
            let key = (object.kind(), object.hash.clone());
            if let Some(old) = objects.get(&key) {
                let old: &PreparedMetadataObject = old;
                if old.bytes() != object.bytes() {
                    return Err(invalid("Backup registry objects collide"));
                }
                continue;
            }
            bytes = bytes.checked_add(object.bytes().len()).ok_or_else(budget)?;
            if objects.len() == 4096 || bytes > 64 * 1024 * 1024 {
                return Err(budget());
            }
            objects.insert(key, object);
        }
        next.objects = objects.into_values().collect();
        Ok(next)
    }
}
fn budget() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "Backup registry exceeds 4096 objects or 64 MiB",
    )
}

impl VerifiedDiskRegistry {
    /// Private replay baseline, not an admitted Current or a persisted graph.
    /// The actual source publications and final registry must still be consumed.
    pub(in crate::engine) fn backup_replay_base(original: &ValidatedConfig) -> Result<Self> {
        if original.generation().is_some() {
            return Err(invalid("Backup namespace replay requires fixed Genesis"));
        }
        Ok(encode_registry(base_state(original, &State::default(), None))?.expected)
    }

    /// Bind a read graph to the prescribed, already published Backup State.
    /// The caller additionally consumes actual Publication and target-local
    /// graph evidence; this check alone grants no namespace or writer lease.
    pub(in crate::engine) fn require_backup_namespace(
        &self,
        candidate: &crate::engine::membership_transition::PreparedBackupFirstCandidate,
        published: &AdmittedState,
    ) -> Result<()> {
        let state = published.state();
        if candidate.original_config().generation().is_some()
            || candidate.candidate().registry().is_some()
            || self.state.source.pool_id != candidate.candidate().pool_id
            || self.state.source.membership_authority.as_deref() != Some(candidate.authority_hash())
            || self.state.source.snapshot_hash.as_deref() != Some(candidate.snapshot_hash())
            || self.state.source.sequence != candidate.sequence()
            || state.sequence != candidate.sequence()
            || self.state.source.topology_generation != state.topology_generation
            || self.state.predecessor.is_none()
            || self.state.pending_exit.is_some()
            || !self.state.detached.is_empty()
            || self
                .state
                .registrations
                .values()
                .any(|entry| entry.mode != RegistrationMode::Enabled)
        {
            return Err(invalid(
                "Backup Current read graph differs from the fixed-source published generation",
            ));
        }
        Ok(())
    }
}
