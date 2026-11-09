//! One exact private staging directory per target, tied to the original move.
use super::*;
use platform_io::fs_safety::linux::FileIdentity;

const INTENT: &str = "target-intent.json";

pub(super) enum Mode {
    Prepare,
    Read,
}

pub(super) struct Target {
    pub(super) root: DiskRoot,
    owner: Arc<DiskHandle>,
    config: ValidatedConfig,
    disk: DiskConfig,
    root_identity: FileIdentity,
    stage_identity: FileIdentity,
    intent: Vec<u8>,
}

impl Target {
    pub(super) fn open(
        engine: &Engine,
        projection: &FileHistoryMoveProjection,
        reviewed: &FileHistoryMoveRecoveryDigest,
        materials: &Materials,
        slot: &str,
        mode: Mode,
    ) -> Result<Self> {
        let disk = engine.disk(slot)?.clone();
        let identity = engine
            .registered_disks()
            .find(|disk| disk.slot_id == slot)
            .filter(|registered| registered.registration_mode == RegistrationMode::Enabled)
            .map(|registered| registered.identity())
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::DiskUnavailable,
                    "move target is not currently enabled",
                )
            })?;
        if identity.pool_id != engine.config.pool_id
            || identity.disk_id != disk.disk_id
            || identity.binding_generation != disk.binding_generation
            || projection.target_binding_generations().get(slot)
                != Some(&identity.binding_generation)
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "move target binding changed",
            ));
        }
        let owner = Arc::clone(engine.disk_resources.get(&disk)?);
        let lease = owner.root.admit_io()?;
        let seal = validate_seal_observed(&engine.config, &disk, &mut |_| lease.check_running())?;
        validate_stored_seal(&owner.seal, &seal)?;
        let root_identity = owner.root.directory_identity(owner.root.path())?;
        let intent = encode_metadata(
            &Intent {
                format: "file-history-move-target-materials",
                operation_id: projection.operation_id(),
                recovery_digest: reviewed.to_string(),
                projection_hash: materials.projection_hash.clone(),
                identity: &identity,
                target_file_id: projection.target_file_id().as_str(),
                file_database_records_hash: materials.database.records_hash(),
            },
            INTENT_BYTES,
        )?;
        let path = owner.root.path().join(DIRECTORY);
        if owner.root.optional_metadata(&path)?.is_none() {
            match mode {
                Mode::Read => {
                    return Err(Error::new(
                        ErrorCode::PlanNotFound,
                        "original move target materials are absent",
                    ));
                }
                Mode::Prepare => {
                    let required = engine
                        .config
                        .metadata_reserve_bytes
                        .checked_add(intent.len() as u64 + 4096)
                        .ok_or_else(limit)?;
                    if owner.root.available()? < required {
                        return Err(Error::new(
                            ErrorCode::NoSpace,
                            "move target intent would consume metadata reserve",
                        ));
                    }
                    owner.root.create_private_directory_synced(&path)?;
                    owner
                        .root
                        .write_atomic(&path.join(INTENT), &intent, false)?;
                }
            }
        }
        owner.root.private_directory(&path)?;
        let stage_identity = owner.root.directory_identity(&path)?;
        let root = owner.root.private_child_matching(&path, stage_identity)?;
        let stored = root
            .open_optional_read_regular(&path.join(INTENT))?
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "move target directory has no original intent; preserve it",
                )
            })?;
        if read_file_bytes(stored, INTENT_BYTES)? != intent {
            return Err(Error::new(
                ErrorCode::ObjectCollision,
                "move target belongs to different retained preparation",
            ));
        }
        let target = Self {
            root,
            owner,
            config: engine.config.clone(),
            disk,
            root_identity,
            stage_identity,
            intent,
        };
        target.check_names()?;
        Ok(target)
    }

    pub(super) fn check_names(&self) -> Result<()> {
        let lease = self.owner.root.admit_io()?;
        let seal =
            validate_seal_observed(&self.config, &self.disk, &mut |_| lease.check_running())?;
        validate_stored_seal(&self.owner.seal, &seal)?;
        let named = DiskRoot::open(self.owner.root.path())?;
        if named.directory_identity(named.path())? != self.root_identity
            || self.owner.root.directory_identity(self.root.path())? != self.stage_identity
            || self
                .root
                .read_bytes(&self.root.path().join(INTENT), INTENT_BYTES)?
                != self.intent
        {
            return Err(Error::new(
                ErrorCode::PreparationStale,
                "original move target resource or intent changed",
            ));
        }
        lease.check_running()
    }

    pub(super) fn preflight(&self, engine: &Engine, materials: &Materials) -> Result<()> {
        if let Some(capacity) = self.disk.capacity_bytes {
            let required = engine
                .used_bytes(&self.disk.slot_id)
                .checked_add(materials.storage_bytes)
                .ok_or_else(limit)?;
            if required > capacity {
                return Err(Error::new(
                    ErrorCode::NoSpace,
                    "complete move target exceeds the configured capacity",
                ));
            }
        }
        let mut remaining = 0u64;
        let mut account = |path: &Path, bytes: u64| -> Result<()> {
            if !crate::history_package::preparation_path_exists(&self.root, path)? {
                remaining = remaining.checked_add(bytes).ok_or_else(limit)?;
            }
            Ok(())
        };
        account(
            &self.root.path().join(LOGICAL),
            materials.logical.len() as u64,
        )?;
        account(
            &self.root.path().join(DATABASE),
            materials.database.bytes().len() as u64,
        )?;
        for version in materials.versions.values() {
            let directory = self.version_directory(version);
            account(
                &directory.join("content").join(&version.creation_name),
                version.logical_size,
            )?;
            account(&directory.join("version.json"), 4096)?;
            account(
                &directory.join("checksums.bin"),
                checksum_manifest_length(version)? as u64,
            )?;
            account(&directory.join("copy-identity.json"), 4096)?;
        }
        let required = remaining
            .checked_add(self.config.metadata_reserve_bytes)
            .ok_or_else(limit)?;
        if self.root.available()? < required {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "complete move target would consume metadata reserve",
            ));
        }
        self.check_names()
    }

    pub(super) fn version_directory(&self, version: &FileVersion) -> std::path::PathBuf {
        self.root.path().join("versions").join(&version.version_id)
    }

    pub(super) fn install_metadata(&self, materials: &Materials) -> Result<()> {
        crate::history_package::install_preparation_bytes(
            &self.root,
            &self.root.path().join(LOGICAL),
            &materials.logical,
        )?;
        let path = self.root.path().join(DATABASE);
        match self.root.open_optional_read_regular(&path)? {
            Some(file) => {
                let bytes = read_file_bytes(
                    file.try_clone()?,
                    crate::container_storage::file_database::MAX_BYTES,
                )?;
                materials.database.verify(&bytes)?;
                file.sync_all()?;
                self.root.sync_dir(self.root.path())?;
            }
            None => self
                .root
                .write_atomic(&path, materials.database.bytes(), false)?,
        }
        self.verify_metadata(materials)
    }

    pub(super) fn verify_metadata(&self, materials: &Materials) -> Result<()> {
        self.check_names()?;
        if self
            .root
            .read_bytes(&self.root.path().join(LOGICAL), METADATA_BYTES)?
            != materials.logical
        {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "target logical graph/audit differs from the original joint projection",
            ));
        }
        let bytes = self.root.read_bytes(
            &self.root.path().join(DATABASE),
            crate::container_storage::file_database::MAX_BYTES,
        )?;
        materials.database.verify(&bytes)?;
        self.check_names()
    }

    pub(super) fn synchronize(&self) -> Result<()> {
        self.check_names()?;
        self.root
            .open_regular(&self.root.path().join(INTENT))?
            .sync_all()?;
        self.root.sync_dir(self.root.path())?;
        self.owner.root.sync_dir(self.owner.root.path())?;
        if self.root.available()? < self.config.metadata_reserve_bytes {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "move preparation exhausted metadata reserve",
            ));
        }
        self.check_names()
    }
}
