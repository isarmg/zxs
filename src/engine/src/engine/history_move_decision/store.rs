//! Immutable per-owner staging. No adoption without the exact original manifest,
//! no overwrite of damaged inputs, and no online container/catalog installation.
use super::*;
use super::materials::{DiskScope, Role};
use platform_io::fs_safety::linux::FileIdentity;
use std::sync::Arc;

pub(super) enum Mode { Prepare, Read }

enum Owner {
    System { root: DiskRoot, writer: FileIdentity },
    Disk { config: ValidatedConfig, disk: DiskConfig, handle: Arc<DiskHandle> },
}
impl Owner {
    fn root(&self) -> &DiskRoot {
        match self {
            Self::System { root, .. } => root,
            Self::Disk { handle, .. } => &handle.root,
        }
    }
    fn check(&self, identity: FileIdentity) -> Result<()> {
        let root = self.root();
        let named = DiskRoot::open(root.path())?;
        if named.directory_identity(named.path())? != identity {
            return Err(invalid("joint move owner root changed"));
        }
        match self {
            Self::System { writer, .. } => {
                let named_writer = named.open_regular(&named.path().join("writer.lock"))?;
                if FileIdentity::from_fd(&named_writer)? != *writer {
                    return Err(invalid("joint move system writer changed"));
                }
            }
            Self::Disk { config, disk, handle } => {
                let lease = handle.root.admit_io()?;
                let seal = validate_seal_observed(config, disk, &mut |_| lease.check_running())?;
                validate_stored_seal(&handle.seal, &seal)?;
                lease.check_running()?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Scope { Metadata, Payload }
impl Scope {
    fn artifacts<'a>(self, materials: &'a Materials) -> Vec<Artifact<'a>> {
        let mut artifacts = vec![
            Artifact::Exact { name: LOGICAL, bytes: &materials.logical },
            Artifact::Database { name: SOURCE_DATABASE, prepared: &materials.source },
            Artifact::Database { name: TARGET_DATABASE, prepared: &materials.target },
        ];
        if let Self::Metadata = self {
            artifacts.push(Artifact::Exact { name: STATE, bytes: &materials.state });
        }
        artifacts
    }
}

enum Artifact<'a> {
    Exact { name: &'static str, bytes: &'a [u8] },
    Database { name: &'static str, prepared: &'a crate::container_storage::file_database::PreparedMoveFile },
}
impl Artifact<'_> {
    fn name(&self) -> &'static str {
        match self { Self::Exact { name, .. } | Self::Database { name, .. } => name }
    }
    fn bytes(&self) -> &[u8] {
        match self { Self::Exact { bytes, .. } => bytes, Self::Database { prepared, .. } => prepared.bytes() }
    }
    fn maximum(&self) -> usize {
        match self {
            Self::Exact { .. } => STATE_BYTES,
            Self::Database { .. } => crate::container_storage::file_database::MAX_BYTES,
        }
    }
    fn verify(&self, actual: &[u8]) -> Result<()> {
        match self {
            Self::Exact { bytes, .. } if *bytes != actual => {
                Err(Error::new(ErrorCode::ObjectCorrupt, "joint move logical material differs from its exact proposal"))
            }
            Self::Exact { .. } => Ok(()),
            Self::Database { prepared, .. } => prepared.verify(actual),
        }
    }
}

pub(super) struct Store {
    owner: Owner,
    root: DiskRoot,
    owner_identity: FileIdentity,
    stage_identity: FileIdentity,
    manifest: Vec<u8>,
    scope: Scope,
    reserve: u64,
}
impl Store {
    pub(super) fn system(engine: &Engine, materials: &Materials, mode: Mode) -> Result<Self> {
        let root = DiskRoot::open(&engine.root)?;
        root.private_directory(root.path())?;
        let owner = Owner::System { root, writer: engine._locks.system_identity()? };
        Self::open(owner, Scope::Metadata, engine.config.metadata_reserve_bytes, materials, mode)
    }

    pub(super) fn disk(
        engine: &Engine,
        materials: &Materials,
        participant: &DiskScope,
        mode: Mode,
    ) -> Result<Self> {
        let identity = &participant.identity;
        let current = engine.registered_disks().find(|disk| disk.slot_id == identity.slot_id)
            .filter(|disk| disk.registration_mode == RegistrationMode::Enabled)
            .ok_or_else(|| invalid("joint move participant is no longer enabled"))?;
        let disk = engine.disk(&identity.slot_id)?;
        if current.identity() != *identity
            || disk.disk_id != identity.disk_id
            || disk.binding_generation != identity.binding_generation
            || disk.metadata_member != participant.carries_state()
            || current.metadata_member != participant.carries_state()
        {
            return Err(invalid("joint move participant scope or binding changed"));
        }
        let scope = if participant.carries_state() { Scope::Metadata } else { Scope::Payload };
        if let Mode::Prepare = mode {
            let mut required = storage_bytes(scope, materials)?;
            if participant.roles.contains(&Role::Target) {
                required = required.checked_add(materials.target_preparation_bytes).ok_or_else(limit)?;
            }
            if participant.carries_state() {
                required = required.checked_add(2 * 1024 * 1024).ok_or_else(limit)?;
            }
            if let Some(capacity) = disk.capacity_bytes
                && engine.used_bytes(&identity.slot_id).checked_add(required).ok_or_else(limit)? > capacity
            {
                return Err(Error::new(ErrorCode::NoSpace, "joint move inputs exceed configured participant capacity"));
            }
        }
        let owner = Owner::Disk {
            config: engine.config.clone(), disk: disk.clone(),
            handle: Arc::clone(engine.disk_resources.get(disk)?),
        };
        Self::open(owner, scope, engine.config.metadata_reserve_bytes, materials, mode)
    }

    fn open(owner: Owner, scope: Scope, reserve: u64, materials: &Materials, mode: Mode) -> Result<Self> {
        let outer = owner.root();
        let owner_identity = outer.directory_identity(outer.path())?;
        owner.check(owner_identity)?;
        let path = outer.path().join(DIRECTORY);
        if outer.optional_metadata(&path)?.is_none() {
            match mode {
                Mode::Read => return Err(Error::new(ErrorCode::PlanNotFound, "original joint move inputs are absent")),
                Mode::Prepare => {
                    if outer.available()? < storage_bytes(scope, materials)?.checked_add(reserve).ok_or_else(limit)? {
                        return Err(Error::new(ErrorCode::NoSpace, "joint move inputs would consume metadata reserve"));
                    }
                    outer.create_private_directory_synced(&path)?;
                    outer.write_atomic(&path.join(MANIFEST), &materials.manifest, false)?;
                }
            }
        }
        outer.private_directory(&path)?;
        let stage_identity = outer.directory_identity(&path)?;
        let root = outer.private_child_matching(&path, stage_identity)?;
        let manifest = root.open_optional_read_regular(&path.join(MANIFEST))?.ok_or_else(|| {
            Error::new(ErrorCode::RecoveryRequired, "joint move input directory lacks its original manifest; retain it")
        })?;
        if read_file_bytes(manifest, MANIFEST_BYTES)? != materials.manifest {
            return Err(Error::new(ErrorCode::ObjectCollision, "joint move input slot belongs to different or damaged material"));
        }
        let store = Self {
            owner, root, owner_identity, stage_identity,
            manifest: materials.manifest.clone(), scope, reserve,
        };
        store.check_names()?;
        Ok(store)
    }

    fn check_names(&self) -> Result<()> {
        self.owner.check(self.owner_identity)?;
        if self.owner.root().directory_identity(self.root.path())? != self.stage_identity
            || self.root.read_bytes(&self.root.path().join(MANIFEST), MANIFEST_BYTES)? != self.manifest
        {
            return Err(invalid("joint move input directory or original manifest changed"));
        }
        Ok(())
    }

    pub(super) fn install(&self, materials: &Materials, cancellation: &HistoryWindowCancellation) -> Result<()> {
        self.check_names()?;
        let artifacts = self.scope.artifacts(materials);
        let mut remaining = 0u64;
        for artifact in &artifacts {
            checkpoint(cancellation)?;
            if self.root.open_optional_read_regular(&self.root.path().join(artifact.name()))?.is_none() {
                remaining = remaining.checked_add(artifact.bytes().len() as u64 + 4096).ok_or_else(limit)?;
            }
        }
        if self.root.available()? < remaining.checked_add(self.reserve).ok_or_else(limit)? {
            return Err(Error::new(ErrorCode::NoSpace, "remaining joint move inputs would consume metadata reserve"));
        }
        for artifact in &artifacts {
            checkpoint(cancellation)?;
            self.check_names()?;
            let path = self.root.path().join(artifact.name());
            if self.root.open_optional_read_regular(&path)?.is_none() {
                self.root.write_atomic(&path, artifact.bytes(), false)?;
            }
            self.read_artifact(artifact, true)?;
        }
        self.root.open_regular(&self.root.path().join(MANIFEST))?.sync_all()?;
        self.root.sync_dir(self.root.path())?;
        self.owner.root().sync_dir(self.owner.root().path())?;
        if self.root.available()? < self.reserve {
            return Err(Error::new(ErrorCode::NoSpace, "joint move input metadata reserve changed"));
        }
        self.verify(materials, cancellation)
    }

    pub(super) fn verify(&self, materials: &Materials, cancellation: &HistoryWindowCancellation) -> Result<()> {
        self.check_names()?;
        for artifact in self.scope.artifacts(materials) {
            checkpoint(cancellation)?;
            self.read_artifact(&artifact, false)?;
        }
        self.check_names()
    }

    fn read_artifact(&self, artifact: &Artifact<'_>, synchronize: bool) -> Result<()> {
        let path = self.root.path().join(artifact.name());
        let file = self.root.open_read_regular(&path)?;
        let stamp = crate::file_stamp::FileStamp::from_metadata(&file.metadata()?);
        artifact.verify(&read_file_bytes(file.try_clone()?, artifact.maximum())?)?;
        if synchronize { file.sync_all()?; }
        let named = self.root.open_read_regular(&path)?;
        if crate::file_stamp::FileStamp::from_metadata(&file.metadata()?) != stamp
            || crate::file_stamp::FileStamp::from_metadata(&named.metadata()?) != stamp
        {
            return Err(invalid("joint move input changed during independent verification"));
        }
        file.check_running()?;
        self.check_names()
    }
}

fn storage_bytes(scope: Scope, materials: &Materials) -> Result<u64> {
    let mut bytes = materials.manifest.len() as u64 + 4096;
    for artifact in scope.artifacts(materials) {
        bytes = bytes.checked_add(artifact.bytes().len() as u64 + 4096).ok_or_else(limit)?;
    }
    Ok(bytes)
}
