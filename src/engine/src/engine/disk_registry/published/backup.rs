//! Exact registry graph after the original Backup first publication. No
//! Engine/namespace installation, disk registration I/O or writer capability.
use super::*;
use crate::history_package::AdmittedState;

impl PreparedDiskRegistry {
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
