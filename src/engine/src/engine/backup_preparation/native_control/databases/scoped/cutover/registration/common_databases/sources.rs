//! Original roots admitted by the live frozen Engine, not by target JSON.
use super::*;

struct OriginalRoot {
    root: DiskRoot,
    disk: DiskConfig,
    physical: PhysicalBinding,
    seal: Vec<u8>,
    lease: DiskIoLease,
}
impl OriginalRoot {
    fn recheck(&self) -> Result<()> {
        self.lease.check_running()?;
        if PhysicalBinding::of(&DiskRoot::open(self.root.path())?)? != self.physical
            || self
                .root
                .read_bytes(&self.root.path().join("disk.json"), MAX_METADATA)?
                != self.seal
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "original common-database root no longer has its captured binding and seal",
            ));
        }
        Ok(())
    }
}

/// No constructor from manifests or prepared targets. These leases and the
/// surrounding worker's original writer locks remain held through all I/O.
pub(in super::super) struct Sources {
    configuration_hash: String,
    metadata_reserve_bytes: u64,
    system: PhysicalBinding,
    system_writer: PhysicalBinding,
    disks: Vec<OriginalRoot>,
}
impl Sources {
    pub(in super::super) fn capture(engine: &Engine, system: &DiskRoot) -> Result<Self> {
        if engine.config.generation().is_some() || engine.config.disks.len() > 32 {
            return Err(Error::new(
                ErrorCode::UnsupportedOperation,
                "common Backup databases require the original bounded fixed Genesis pool",
            ));
        }
        let writer = engine._locks.system_identity()?;
        let named = system.open_read_regular(&system.path().join("writer.lock"))?;
        if platform_io::fs_safety::linux::FileIdentity::from_metadata(&named.metadata()?) != writer
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "SYSTEM common-database writer ownership changed",
            ));
        }
        let system_writer = PhysicalBinding {
            device: writer.device,
            inode: writer.inode,
        };
        let mut disks = Vec::with_capacity(engine.config.disks.len());
        for disk in &engine.config.disks {
            let handle = engine.disk_resources.get(disk)?;
            let root = &handle.root;
            let lease = root.io_binding()?.admit()?;
            engine._locks.verify_disk(disk, root)?;
            let physical = PhysicalBinding::of(root)?;
            let seal = root.read_bytes(&root.path().join("disk.json"), MAX_METADATA)?;
            validate_stored_seal(&decode_seal(&seal)?, &handle.seal)?;
            let source = OriginalRoot {
                root: root.private_child(root.path())?,
                disk: disk.clone(),
                physical,
                seal,
                lease,
            };
            source.recheck()?;
            // Common files are below the existing metadata directory. This
            // does not create/repair a missing original metadata root.
            source
                .root
                .private_child(&source.root.path().join("metadata"))?;
            disks.push(source);
        }
        Ok(Self {
            configuration_hash: object_hash("backup-preparation-config", &engine.config)?,
            metadata_reserve_bytes: engine.config.metadata_reserve_bytes,
            system: PhysicalBinding::of(system)?,
            system_writer,
            disks,
        })
    }
    pub(super) fn require_task(&self, prepared: &PreparedBackupRegistry) -> Result<()> {
        if self.configuration_hash != prepared.projection.original_configuration_hash
            || self.disks.len() != prepared.projection.retained_original_disks.len()
            || canonical_json(
                &self
                    .disks
                    .iter()
                    .map(|source| &source.disk)
                    .collect::<Vec<_>>(),
            )? != canonical_json(&prepared.projection.retained_original_disks)?
        {
            return Err(invalid(
                "common database sources are not this exact frozen original pool",
            ));
        }
        Ok(())
    }
    pub(in super::super) fn recheck(&self, system: &DiskRoot) -> Result<()> {
        if PhysicalBinding::of(&DiskRoot::open(system.path())?)? != self.system {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "SYSTEM common-database root changed",
            ));
        }
        let named = system.open_read_regular(&system.path().join("writer.lock"))?;
        let writer = platform_io::fs_safety::linux::FileIdentity::from_metadata(&named.metadata()?);
        if self.system_writer
            != (PhysicalBinding {
                device: writer.device,
                inode: writer.inode,
            })
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "SYSTEM common-database writer name changed",
            ));
        }
        for disk in &self.disks {
            disk.recheck()?;
        }
        Ok(())
    }
    pub(super) fn bindings(&self, pool_id: &str) -> Vec<OriginalBinding> {
        self.disks
            .iter()
            .map(|disk| OriginalBinding {
                identity: identity(pool_id, &disk.disk),
                physical: disk.physical.clone(),
                seal_hash: blake3::hash(&disk.seal).to_hex().to_string(),
            })
            .collect()
    }
    pub(in super::super) fn system_binding(&self) -> &PhysicalBinding {
        &self.system
    }
    pub(in super::super) fn metadata_reserve_bytes(&self) -> u64 {
        self.metadata_reserve_bytes
    }
    pub(in super::super) fn system_writer_binding(&self) -> &PhysicalBinding {
        &self.system_writer
    }
    pub(in super::super) fn for_each(
        &self,
        stop: &AtomicBool,
        mut action: impl FnMut(&DiskRoot, &DiskRegistrationIdentity) -> Result<()>,
        pool_id: &str,
    ) -> Result<()> {
        for source in &self.disks {
            preparation_checkpoint(stop)?;
            source.recheck()?;
            let root = source
                .root
                .private_child(&source.root.path().join("metadata"))?;
            action(&root, &identity(pool_id, &source.disk))?;
            source.recheck()?;
        }
        Ok(())
    }
    /// Original held disk roots, not their metadata children and not target
    /// paths read from a descriptor. The caller cannot change this source set.
    pub(in super::super) fn for_each_disk(
        &self,
        stop: &AtomicBool,
        mut action: impl FnMut(&DiskRoot, &DiskRegistrationIdentity) -> Result<()>,
        pool_id: &str,
    ) -> Result<()> {
        for source in &self.disks {
            preparation_checkpoint(stop)?;
            source.recheck()?;
            action(&source.root, &identity(pool_id, &source.disk))?;
            source.recheck()?;
        }
        Ok(())
    }
    pub(in super::super) fn require_capacity_before_choice(&self, system: &DiskRoot) -> Result<()> {
        self.recheck(system)?;
        // At most one 64 MiB SYSTEM or two 32 MiB local SQLite files plus
        // private manifests, atomic staging and retained completion overhead.
        const REQUIRED: u64 = 80 * 1024 * 1024;
        if system.available()? < REQUIRED {
            return Err(no_space());
        }
        let original_control = REQUIRED
            .checked_add(original_control::PRE_CHOICE_BYTES)
            .and_then(|bytes| bytes.checked_add(self.metadata_reserve_bytes))
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ResourceLimit,
                    "original Backup control capacity overflow",
                )
            })?;
        for source in &self.disks {
            if source.root.available()? < original_control {
                return Err(Error::new(
                    ErrorCode::NoSpace,
                    "new Backup choice requires complete original-root control/archive capacity beside common databases and metadata reserve",
                ));
            }
        }
        Ok(())
    }
}
fn identity(pool_id: &str, disk: &DiskConfig) -> DiskRegistrationIdentity {
    DiskRegistrationIdentity {
        pool_id: pool_id.into(),
        disk_id: disk.disk_id.clone(),
        slot_id: disk.slot_id.clone(),
        binding_generation: disk.binding_generation,
    }
}
fn no_space() -> Error {
    Error::new(
        ErrorCode::NoSpace,
        "SYSTEM or an original disk lacks bounded common-database capacity",
    )
}
