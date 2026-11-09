//! A retained refusal marker is not permanent after its exact native switch.
//! Only an already admitted serving Engine may interpret it as that operation's
//! completed installation. No selector bytes construct authority or a writer.
use super::*;

fn digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid(
            "completed root database selection has a malformed digest",
        ));
    }
    Ok(())
}

impl Manifest {
    fn require_original_bindings(&self, engine: &Engine) -> Result<()> {
        let generation = engine
            .config
            .generation()
            .ok_or_else(|| invalid("completed root databases have no native Backup generation"))?;
        let targets = generation
            .backup_layout_targets()?
            .ok_or_else(|| invalid("completed root databases belong to a non-Backup generation"))?;
        let original: Vec<_> = engine
            .config
            .disks
            .iter()
            .filter(|disk| !targets.contains_key(&disk.slot_id))
            .collect();
        if self.original_roots.len() != original.len()
            || self.original_databases.len() != original.len()
        {
            return Err(invalid("completed root databases omit an original binding"));
        }
        digest(&self.aliases_hash)?;
        for ((binding, database), disk) in self
            .original_roots
            .iter()
            .zip(&self.original_databases)
            .zip(original)
        {
            let identity = DiskRegistrationIdentity {
                pool_id: engine.config.pool_id.clone(),
                disk_id: disk.disk_id.clone(),
                slot_id: disk.slot_id.clone(),
                binding_generation: disk.binding_generation,
            };
            if binding.identity != identity
                || database.identity != identity
                || database.bytes == 0
                || database.bytes > 2 * MAX_DATABASE as u64
            {
                return Err(invalid(
                    "completed root databases changed their original exact scope",
                ));
            }
            digest(&database.current_hash)?;
            digest(&database.history_hash)?;
            let root = &engine.disk_resources.get(disk)?.root;
            engine._locks.verify_disk(disk, root)?;
            require_name(root, &binding.physical)?;
        }
        Ok(())
    }
}

impl Engine {
    pub(in crate::engine) fn require_backup_catalog_admission(
        &self,
        root: &DiskRoot,
    ) -> Result<()> {
        let serving = self.backup_serving.as_ref().ok_or_else(|| {
            invalid("selected root databases have no completed native serving admission")
        })?;
        serving.barrier.require_current(self)?;
        if self.state.operations.get(&serving.result.operation_id) != Some(&serving.result) {
            return Err(invalid(
                "selected root databases lost their original native operation",
            ));
        }
        let selected = crate::disk::HeldRegular::capture(root, BACKUP_SELECTION, MAX_DESCRIPTOR)?
            .ok_or_else(|| invalid("completed root database selection is missing"))?;
        let bytes = selected.bytes();
        let manifest: Manifest = storage_contract::decode_strict(bytes)?;
        if canonical_json(&manifest)? != bytes
            || manifest.format != "zxs-backup-ordinary-root-databases"
            || manifest.source_fence_hash != serving.result.request_digest
            || manifest.publication_hash != serving.barrier.publication
            || manifest.sequence != serving.barrier.first_sequence
        {
            return Err(invalid(
                "selected root databases belong to another or incomplete Backup operation",
            ));
        }
        manifest.require_original_bindings(self)?;
        let system = DiskRoot::open(&self.root)?;
        let writer = system.open_read_regular(&system.path().join("writer.lock"))?;
        if platform_io::fs_safety::linux::FileIdentity::from_metadata(&writer.metadata()?)
            != self._locks.system_identity()?
        {
            return Err(invalid(
                "root database selection lost the admitted SYSTEM writer",
            ));
        }
        for name in [INTENT, BACKUP_SELECTION, COMPLETE] {
            if system.read_bytes(&system.path().join(name), MAX_DESCRIPTOR)? != bytes {
                return Err(invalid(
                    "root databases differ from their complete original SYSTEM selection",
                ));
            }
        }
        if root.read_bytes(&root.path().join(COMPLETE), MAX_DESCRIPTOR)? != bytes {
            return Err(invalid(
                "selected root databases lack their exact completion",
            ));
        }
        // The initial SQLite hashes are historical commitments, not today's
        // mutable cache contents. Active reads/rebuilds still derive and verify
        // their own complete scope from authenticated State/publication history.
        let after = crate::disk::HeldRegular::capture(root, BACKUP_SELECTION, MAX_DESCRIPTOR)?
            .ok_or_else(|| invalid("completed root database selection disappeared"))?;
        if after.stamp() != selected.stamp() {
            return Err(invalid(
                "completed root database selection changed during admission",
            ));
        }
        selected.check_running()?;
        writer.check_running()
    }
}
