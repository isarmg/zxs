//! Nonvoting additions hold their own identities and durable readiness. They
//! never acquire control archives, voting journals, or a place in either quorum.
use super::*;

const READY_KIND: &str = "membership-data-ready";
const READY_FILE: &str = "data-ready.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum ReadyFormat {
    #[serialization(rename = "membership-data-ready")]
    MembershipDataReady,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct DataDependency {
    format: ReadyFormat,
    pub(super) seal: PendingDiskSeal,
}
impl DataDependency {
    pub(super) fn new(seal: PendingDiskSeal) -> Self {
        Self {
            format: ReadyFormat::MembershipDataReady,
            seal,
        }
    }
    pub(super) fn verify(
        &self,
        disk: &DiskConfig,
        intent: &TransitionIntent,
        archive: &str,
    ) -> Result<()> {
        let identity = &self.seal.identity;
        if disk.metadata_member
            || intent
                .origin
                .config()
                .disks
                .iter()
                .any(|old| old.disk_id == disk.disk_id)
            || identity.pool_id != intent.target.pool_id
            || identity.disk_id != disk.disk_id
            || identity.slot_id != disk.slot_id
            || identity.binding_generation != disk.binding_generation
            || identity.simulation != (intent.target.mode == Mode::Development)
            || intent.added_roots.get(&disk.disk_id) != Some(identity)
            || self.seal.transition_hash != object_hash(INTENT_KIND, intent)?
            || self.seal.target_config_hash != object_hash("membership-target", &intent.target)?
            || self.seal.control_archive_root != archive
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "data readiness differs from exact addition",
            ));
        }
        Ok(())
    }
    pub(super) fn verify_active_seal(&self, seal: &DiskSeal) -> Result<()> {
        let identity = &self.seal.identity;
        if identity.pool_id != seal.pool_id
            || identity.disk_id != seal.disk_id
            || identity.slot_id != seal.slot_id
            || identity.binding_generation != seal.binding_generation
            || identity.device != seal.device
            || identity.inode != seal.inode
            || identity.simulation != seal.simulation
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "data root differs from certified physical identity",
            ));
        }
        Ok(())
    }
    pub(super) fn path(&self, root: &Path) -> PathBuf {
        root.join("metadata/membership-pending")
            .join(&self.seal.transition_hash)
            .join(READY_FILE)
    }
    pub(super) fn verify_bytes(&self, bytes: &[u8]) -> Result<()> {
        AuthenticatedMetadataObject::authenticate(
            READY_KIND,
            &object_hash(READY_KIND, self)?,
            decode_strict(bytes)?,
        )?;
        Ok(())
    }
    fn synchronize_local(&self, root: &DiskRoot) -> Result<()> {
        let path = self.path(root.path());
        let directory = path.parent().expect("invariant: readiness has a parent");
        let held = root.private_child(directory)?;
        let file = held.open_read_regular(&path)?;
        self.verify_bytes(&read_file_bytes(file.try_clone()?, MAX_METADATA)?)?;
        file.sync_all()?;
        held.sync_dir(directory)?;
        if FileIdentity::from_fd(root.directory(directory)?)?
            != FileIdentity::from_fd(held.directory(directory)?)?
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "data readiness directory moved before durable observation",
            ));
        }
        self.verify_local(root)
    }
    pub(super) fn verify_local(&self, root: &DiskRoot) -> Result<()> {
        let identity = admission::verify_named_root(root)?;
        if identity.device != self.seal.identity.device
            || identity.inode != self.seal.identity.inode
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "data readiness root moved",
            ));
        }
        self.verify_bytes(&root.read_bytes(&self.path(root.path()), MAX_METADATA)?)
    }
}

pub(super) struct PendingDataDisk {
    pub(super) disk: DiskConfig,
    pub(super) resource: Box<PendingDisk>,
    pub(super) dependency: DataDependency,
}
impl PendingDataDisk {
    pub(super) fn validate(&self) -> Result<()> {
        admission::verify_named_root(&self.resource.root)?;
        reservation::verify_writer(&self.resource.root, &self.resource._lock)?;
        admission::inspect_new_root(
            &self.resource.root,
            &self.resource.seal,
            &self.resource.reservation_name,
        )?;
        Ok(())
    }
    pub(super) fn synchronize(&self) -> Result<()> {
        self.validate()?;
        let root = &self.resource.root;
        let directory = member_directory(&self.disk, &self.resource.seal.transition_hash);
        root.private_dir(&directory)?;
        let held = root.private_child(&directory)?;
        let names = held
            .entries(&directory)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(fs_error)?;
        let path = self.dependency.path(&self.disk.root);
        let expected = encode_metadata(&self.dependency, MAX_METADATA)?;
        if let Some(file) = held.open_optional_read_regular(&path)? {
            self.dependency
                .verify_bytes(&read_file_bytes(file, MAX_METADATA)?)?;
        }
        for name in names {
            if platform_io::fs_safety::AtomicFile::is_temporary_name(&name) {
                let bytes = held.read_bytes(&directory.join(name.as_path()), MAX_METADATA)?;
                // The already-admitted seal binds this private operation namespace.
                // A staging artifact can explain an interrupted exact write, never
                // supply a different readiness or authorize another operation.
                if !expected.starts_with(&bytes) {
                    return Err(Error::new(
                        ErrorCode::ObjectCorrupt,
                        "data readiness staging differs from its exact pending dependency",
                    ));
                }
            } else if name.as_os_str() != READY_FILE {
                return Err(Error::new(
                    ErrorCode::UnsafePath,
                    "nonvoting readiness contains unrelated material",
                ));
            }
        }
        // Preserve staging evidence. Publish a fresh complete immutable file only
        // after every existing artifact is accounted for; never rename a prefix.
        held.write_atomic(&path, &expected, false)?;
        self.verify_ready()
    }
    pub(super) fn verify_ready(&self) -> Result<()> {
        self.validate()?;
        self.dependency.synchronize_local(&self.resource.root)
    }
}
