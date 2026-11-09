//! Immutable per-owner staging. No adoption without the exact original manifest,
//! no overwrite of damaged inputs, and no online container/catalog installation.
use super::materials::{DiskScope, Role};
use super::*;
use platform_io::fs_safety::linux::FileIdentity;
use std::sync::Arc;

pub(super) enum Mode {
    Prepare,
    Read,
}

enum Owner {
    System {
        root: DiskRoot,
        writer: FileIdentity,
    },
    Disk {
        config: ValidatedConfig,
        disk: DiskConfig,
        handle: Arc<DiskHandle>,
    },
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
            Self::Disk {
                config,
                disk,
                handle,
            } => {
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
enum Scope {
    Metadata,
    Payload,
}
impl Scope {
    fn artifacts<'a>(self, materials: &'a Materials) -> Vec<Artifact<'a>> {
        let mut artifacts = vec![
            Artifact::Exact {
                name: LOGICAL,
                bytes: &materials.logical,
            },
            Artifact::Database {
                name: SOURCE_DATABASE,
                prepared: &materials.source,
            },
            Artifact::Database {
                name: TARGET_DATABASE,
                prepared: &materials.target,
            },
        ];
        if let Self::Metadata = self {
            artifacts.push(Artifact::Exact {
                name: STATE,
                bytes: &materials.state,
            });
        }
        artifacts
    }

    fn native_artifacts(self, native: &crate::business::NativeMoveDraft) -> Vec<Artifact<'_>> {
        let mut artifacts = vec![Artifact::Exact {
            name: "native-operation.json",
            bytes: native.envelope(),
        }];
        if let Self::Metadata = self {
            artifacts.push(Artifact::Exact {
                name: "native-state.json",
                bytes: native.state(),
            });
        }
        artifacts
    }
}

pub(super) enum Artifact<'a> {
    Exact {
        name: &'a str,
        bytes: &'a [u8],
    },
    Scoped {
        name: &'a str,
        bytes: &'a [u8],
    },
    Database {
        name: &'a str,
        prepared: &'a crate::container_storage::file_database::PreparedMoveFile,
    },
}
impl Artifact<'_> {
    fn name(&self) -> &str {
        match self {
            Self::Exact { name, .. } | Self::Scoped { name, .. } | Self::Database { name, .. } => {
                name
            }
        }
    }
    pub(super) fn bytes(&self) -> &[u8] {
        match self {
            Self::Exact { bytes, .. } | Self::Scoped { bytes, .. } => bytes,
            Self::Database { prepared, .. } => prepared.bytes(),
        }
    }
    fn maximum(&self) -> usize {
        match self {
            Self::Exact { .. } => STATE_BYTES,
            Self::Scoped { .. } => scopes::MAX_DATABASE_BYTES,
            Self::Database { .. } => crate::container_storage::file_database::MAX_BYTES,
        }
    }
    fn verify(&self, actual: &[u8]) -> Result<()> {
        match self {
            Self::Exact { bytes, .. } | Self::Scoped { bytes, .. } if *bytes != actual => {
                Err(Error::new(
                    ErrorCode::ObjectCorrupt,
                    "joint move logical material differs from its exact proposal",
                ))
            }
            Self::Exact { .. } | Self::Scoped { .. } => Ok(()),
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
        let owner = Owner::System {
            root,
            writer: engine._locks.system_identity()?,
        };
        Self::open(
            owner,
            Scope::Metadata,
            engine.config.metadata_reserve_bytes,
            materials,
            mode,
        )
    }

    pub(super) fn disk(
        engine: &Engine,
        materials: &Materials,
        participant: &DiskScope,
        mode: Mode,
    ) -> Result<Self> {
        let identity = &participant.identity;
        let current = engine
            .registered_disks()
            .find(|disk| disk.slot_id == identity.slot_id)
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
        let scope = if participant.carries_state() {
            Scope::Metadata
        } else {
            Scope::Payload
        };
        if let Mode::Prepare = mode {
            let mut required = storage_bytes(scope, materials)?;
            if participant.roles.contains(&Role::Target) {
                required = required
                    .checked_add(materials.target_preparation_bytes)
                    .ok_or_else(limit)?;
            }
            if participant.carries_state() {
                required = required.checked_add(2 * 1024 * 1024).ok_or_else(limit)?;
            }
            if let Some(capacity) = disk.capacity_bytes
                && engine
                    .used_bytes(&identity.slot_id)
                    .checked_add(required)
                    .ok_or_else(limit)?
                    > capacity
            {
                return Err(Error::new(
                    ErrorCode::NoSpace,
                    "joint move inputs exceed configured participant capacity",
                ));
            }
        }
        let owner = Owner::Disk {
            config: engine.config.clone(),
            disk: disk.clone(),
            handle: Arc::clone(engine.disk_resources.get(disk)?),
        };
        Self::open(
            owner,
            scope,
            engine.config.metadata_reserve_bytes,
            materials,
            mode,
        )
    }

    fn open(
        owner: Owner,
        scope: Scope,
        reserve: u64,
        materials: &Materials,
        mode: Mode,
    ) -> Result<Self> {
        let outer = owner.root();
        let owner_identity = outer.directory_identity(outer.path())?;
        owner.check(owner_identity)?;
        let path = outer.path().join(DIRECTORY);
        if outer.optional_metadata(&path)?.is_none() {
            match mode {
                Mode::Read => {
                    return Err(Error::new(
                        ErrorCode::PlanNotFound,
                        "original joint move inputs are absent",
                    ));
                }
                Mode::Prepare => {
                    if outer.available()?
                        < storage_bytes(scope, materials)?
                            .checked_add(reserve)
                            .ok_or_else(limit)?
                    {
                        return Err(Error::new(
                            ErrorCode::NoSpace,
                            "joint move inputs would consume metadata reserve",
                        ));
                    }
                    outer.create_private_directory_synced(&path)?;
                    outer.write_atomic(&path.join(MANIFEST), &materials.manifest, false)?;
                }
            }
        }
        outer.private_directory(&path)?;
        let stage_identity = outer.directory_identity(&path)?;
        let root = outer.private_child_matching(&path, stage_identity)?;
        let manifest = root
            .open_optional_read_regular(&path.join(MANIFEST))?
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "joint move input directory lacks its original manifest; retain it",
                )
            })?;
        if read_file_bytes(manifest, MANIFEST_BYTES)? != materials.manifest {
            return Err(Error::new(
                ErrorCode::ObjectCollision,
                "joint move input slot belongs to different or damaged material",
            ));
        }
        let store = Self {
            owner,
            root,
            owner_identity,
            stage_identity,
            manifest: materials.manifest.clone(),
            scope,
            reserve,
        };
        store.check_names()?;
        Ok(store)
    }

    fn check_names(&self) -> Result<()> {
        self.owner.check(self.owner_identity)?;
        if self.owner.root().directory_identity(self.root.path())? != self.stage_identity
            || self
                .root
                .read_bytes(&self.root.path().join(MANIFEST), MANIFEST_BYTES)?
                != self.manifest
        {
            return Err(invalid(
                "joint move input directory or original manifest changed",
            ));
        }
        Ok(())
    }

    pub(super) fn install(
        &self,
        materials: &Materials,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        let artifacts = self.scope.artifacts(materials);
        self.install_artifacts(&artifacts, cancellation)
    }

    pub(super) fn install_native(
        &self,
        native: &crate::business::NativeMoveDraft,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        self.install_artifacts(&self.scope.native_artifacts(native), cancellation)
    }

    fn scoped_artifacts<'a>(&self, scopes: &'a scopes::Scopes) -> Vec<Artifact<'a>> {
        let payload_disk = match (&self.scope, &self.owner) {
            (Scope::Metadata, _) => None,
            (Scope::Payload, Owner::Disk { disk, .. }) => Some(disk.disk_id.as_str()),
            (Scope::Payload, Owner::System { .. }) => {
                unreachable!("invariant: the SYSTEM store always has metadata scope")
            }
        };
        scopes.artifacts(payload_disk)
    }

    pub(super) fn install_scopes(
        &self,
        scopes: &scopes::Scopes,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        self.check_names()?;
        let artifacts = self.scoped_artifacts(scopes);
        if self
            .root
            .open_optional_read_regular(&self.root.path().join(scopes::SCOPE_MANIFEST))?
            .is_none()
        {
            // A crash before the immutable manifest may leave no scoped files.
            // If any expected image exists, its origin is unknown; do not adopt
            // it by writing a fresh manifest around retained bytes.
            for artifact in artifacts.iter().skip(1) {
                checkpoint(cancellation)?;
                if self
                    .root
                    .open_optional_read_regular(&self.root.path().join(artifact.name()))?
                    .is_some()
                {
                    return Err(Error::new(
                        ErrorCode::RecoveryRequired,
                        "move scoped image lacks its original manifest; retain all files",
                    ));
                }
            }
        }
        self.install_artifacts(&artifacts, cancellation)
    }

    pub(super) fn verify_scopes(
        &self,
        scopes: &scopes::Scopes,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        self.verify_artifacts(&self.scoped_artifacts(scopes), cancellation)
    }

    pub(super) fn require_scoped_capacity(
        engine: &Engine,
        materials: &Materials,
        participant: &DiskScope,
        native: &crate::business::NativeMoveDraft,
        scopes: &scopes::Scopes,
    ) -> Result<()> {
        let disk = engine.disk(&participant.identity.slot_id)?;
        let Some(capacity) = disk.capacity_bytes else {
            return Ok(());
        };
        let required = scoped_storage_bytes(materials, participant, native, scopes)?;
        if engine
            .used_bytes(&participant.identity.slot_id)
            .checked_add(required)
            .ok_or_else(limit)?
            > capacity
        {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "move scoped databases exceed configured participant capacity",
            ));
        }
        Ok(())
    }

    pub(super) fn install_choice(
        &self,
        bytes: &[u8],
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        self.install_artifacts(
            &[Artifact::Exact {
                name: choice::NAME,
                bytes,
            }],
            cancellation,
        )
    }

    pub(super) fn verify_choice(
        &self,
        bytes: &[u8],
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        self.verify_artifacts(
            &[Artifact::Exact {
                name: choice::NAME,
                bytes,
            }],
            cancellation,
        )
    }

    pub(super) fn require_choice_capacity(
        engine: &Engine,
        materials: &Materials,
        participant: &DiskScope,
        native: &crate::business::NativeMoveDraft,
        scopes: &scopes::Scopes,
        choice_bytes: usize,
    ) -> Result<()> {
        let disk = engine.disk(&participant.identity.slot_id)?;
        let Some(capacity) = disk.capacity_bytes else {
            return Ok(());
        };
        let required = scoped_storage_bytes(materials, participant, native, scopes)?
            .checked_add(choice_bytes as u64 + 4096)
            .ok_or_else(limit)?;
        if engine
            .used_bytes(&participant.identity.slot_id)
            .checked_add(required)
            .ok_or_else(limit)?
            > capacity
        {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "move joint choice and all retained inputs exceed configured participant capacity",
            ));
        }
        Ok(())
    }

    pub(super) fn require_candidate_space(
        &self,
        candidate: &candidate::FrozenCandidate,
        ordinary_missing: u64,
    ) -> Result<()> {
        self.check_names()?;
        let metadata = matches!(self.scope, Scope::Metadata);
        let artifacts = candidate.artifacts(metadata);
        let original = self
            .root
            .open_optional_read_regular(&self.root.path().join(candidate::MANIFEST))?;
        if original.is_none() {
            for artifact in artifacts.iter().skip(1) {
                if self
                    .root
                    .open_optional_read_regular(&self.root.path().join(artifact.name()))?
                    .is_some()
                {
                    return Err(Error::new(
                        ErrorCode::RecoveryRequired,
                        "move candidate stage has no original manifest; preserve it",
                    ));
                }
            }
            if let Owner::Disk { disk, handle, .. } = &self.owner
                && metadata
                && !crate::journal::existing_object_paths_at(
                    &handle.root,
                    disk,
                    "candidate",
                    candidate.hash(),
                )?
                .is_empty()
            {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "ordinary move candidate has no original input manifest; preserve it",
                ));
            }
        }
        let mut required = ordinary_missing
            .checked_add(self.reserve)
            .ok_or_else(limit)?;
        for artifact in artifacts {
            if self
                .root
                .open_optional_read_regular(&self.root.path().join(artifact.name()))?
                .is_none()
            {
                required = required
                    .checked_add(artifact.bytes().len() as u64 + 4096)
                    .ok_or_else(limit)?;
            } else {
                self.read_artifact(&artifact, false)?;
            }
        }
        if self.root.available()? < required {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "move candidate dependencies and stage would consume metadata reserve",
            ));
        }
        self.check_names()
    }

    pub(super) fn install_candidate(
        &self,
        candidate: &candidate::FrozenCandidate,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        self.install_artifacts(
            &candidate.artifacts(matches!(self.scope, Scope::Metadata)),
            cancellation,
        )
    }

    pub(super) fn verify_candidate(
        &self,
        candidate: &candidate::FrozenCandidate,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        self.verify_artifacts(
            &candidate.artifacts(matches!(self.scope, Scope::Metadata)),
            cancellation,
        )
    }

    pub(super) fn install_artifacts(
        &self,
        artifacts: &[Artifact<'_>],
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        self.check_names()?;
        let mut remaining = 0u64;
        for artifact in artifacts {
            checkpoint(cancellation)?;
            if self
                .root
                .open_optional_read_regular(&self.root.path().join(artifact.name()))?
                .is_none()
            {
                remaining = remaining
                    .checked_add(artifact.bytes().len() as u64 + 4096)
                    .ok_or_else(limit)?;
            }
        }
        if self.root.available()? < remaining.checked_add(self.reserve).ok_or_else(limit)? {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "remaining joint move inputs would consume metadata reserve",
            ));
        }
        for artifact in artifacts {
            checkpoint(cancellation)?;
            self.check_names()?;
            let path = self.root.path().join(artifact.name());
            if self.root.open_optional_read_regular(&path)?.is_none() {
                self.root.write_atomic(&path, artifact.bytes(), false)?;
            }
            self.read_artifact(artifact, true)?;
        }
        self.root
            .open_regular(&self.root.path().join(MANIFEST))?
            .sync_all()?;
        self.root.sync_dir(self.root.path())?;
        self.owner.root().sync_dir(self.owner.root().path())?;
        if self.root.available()? < self.reserve {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "joint move input metadata reserve changed",
            ));
        }
        self.verify_artifacts(artifacts, cancellation)
    }

    pub(super) fn verify(
        &self,
        materials: &Materials,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        self.verify_artifacts(&self.scope.artifacts(materials), cancellation)
    }

    pub(super) fn verify_native(
        &self,
        native: &crate::business::NativeMoveDraft,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        self.verify_artifacts(&self.scope.native_artifacts(native), cancellation)
    }

    pub(super) fn require_native_capacity(
        engine: &Engine,
        materials: &Materials,
        participant: &DiskScope,
        native: &crate::business::NativeMoveDraft,
    ) -> Result<()> {
        let disk = engine.disk(&participant.identity.slot_id)?;
        let Some(capacity) = disk.capacity_bytes else {
            return Ok(());
        };
        let required = native_storage_bytes(materials, participant, native)?;
        if engine
            .used_bytes(&participant.identity.slot_id)
            .checked_add(required)
            .ok_or_else(limit)?
            > capacity
        {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "native move draft exceeds configured participant capacity",
            ));
        }
        Ok(())
    }

    pub(super) fn verify_artifacts(
        &self,
        artifacts: &[Artifact<'_>],
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        self.check_names()?;
        for artifact in artifacts {
            checkpoint(cancellation)?;
            self.read_artifact(artifact, false)?;
        }
        self.check_names()
    }

    pub(super) fn read_optional_artifact(
        &self,
        name: &str,
        maximum: usize,
    ) -> Result<Option<Vec<u8>>> {
        self.check_names()?;
        let held = HeldRegular::capture(&self.root, name, maximum)?;
        let named = HeldRegular::capture(&self.root, name, maximum)?;
        if held.as_ref().map(HeldRegular::stamp) != named.as_ref().map(HeldRegular::stamp) {
            return Err(invalid(
                "joint move retained artifact changed during observation",
            ));
        }
        self.check_names()?;
        held.map(|held| {
            held.check_running()?;
            Ok(held.bytes().to_vec())
        })
        .transpose()
    }

    fn read_artifact(&self, artifact: &Artifact<'_>, synchronize: bool) -> Result<()> {
        let path = self.root.path().join(artifact.name());
        let file = self.root.open_read_regular(&path)?;
        let stamp = crate::file_stamp::FileStamp::from_metadata(&file.metadata()?);
        artifact.verify(&read_file_bytes(file.try_clone()?, artifact.maximum())?)?;
        if synchronize {
            file.sync_all()?;
        }
        let named = self.root.open_read_regular(&path)?;
        if crate::file_stamp::FileStamp::from_metadata(&file.metadata()?) != stamp
            || crate::file_stamp::FileStamp::from_metadata(&named.metadata()?) != stamp
        {
            return Err(invalid(
                "joint move input changed during independent verification",
            ));
        }
        file.check_running()?;
        self.check_names()
    }
}

fn storage_bytes(scope: Scope, materials: &Materials) -> Result<u64> {
    let mut bytes = materials.manifest.len() as u64 + 4096;
    for artifact in scope.artifacts(materials) {
        bytes = bytes
            .checked_add(artifact.bytes().len() as u64 + 4096)
            .ok_or_else(limit)?;
    }
    Ok(bytes)
}

fn native_storage_bytes(
    materials: &Materials,
    participant: &DiskScope,
    native: &crate::business::NativeMoveDraft,
) -> Result<u64> {
    let scope = if participant.carries_state() {
        Scope::Metadata
    } else {
        Scope::Payload
    };
    let mut required = storage_bytes(scope, materials)?;
    if participant.roles.contains(&Role::Target) {
        required = required
            .checked_add(materials.target_preparation_bytes)
            .ok_or_else(limit)?;
    }
    if participant.carries_state() {
        required = required.checked_add(2 * 1024 * 1024).ok_or_else(limit)?;
    }
    for artifact in scope.native_artifacts(native) {
        required = required
            .checked_add(artifact.bytes().len() as u64 + 4096)
            .ok_or_else(limit)?;
    }
    Ok(required)
}

pub(super) fn scoped_storage_bytes(
    materials: &Materials,
    participant: &DiskScope,
    native: &crate::business::NativeMoveDraft,
    scopes: &scopes::Scopes,
) -> Result<u64> {
    let mut required = native_storage_bytes(materials, participant, native)?;
    let payload_disk =
        (!participant.carries_state()).then_some(participant.identity.disk_id.as_str());
    for artifact in scopes.artifacts(payload_disk) {
        required = required
            .checked_add(artifact.bytes().len() as u64 + 4096)
            .ok_or_else(limit)?;
    }
    Ok(required)
}
