use super::super::store as inputs;
use super::*;
use crate::history_package::preparation_path_exists as exists;

#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
struct VersionInput<'a> {
    version_id: &'a str,
    version_hash: String,
    original_copy: &'a str,
}
#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
struct Intent<'a> {
    format: &'static str,
    candidate_hash: &'a str,
    choice_digest: &'a str,
    identity: &'a DiskRegistrationIdentity,
    file: &'a str,
    history: &'a str,
    name: &'a str,
    versions: Vec<VersionInput<'a>>,
}
#[derive(Deserialize, Serialize, PartialEq, Eq)]
#[serialization(deny_unknown_fields)]
struct Container {
    format: String,
    candidate_hash: String,
    stable: Inode,
    named: Inode,
    versions: Inode,
}

pub(super) struct Target {
    pub(super) owner: Arc<DiskHandle>,
    config: ValidatedConfig,
    pub(super) disk: DiskConfig,
    identity: DiskRegistrationIdentity,
    owner_inode: Inode,
    pub(super) stage: Option<DiskRoot>,
    manifest: Vec<u8>,
    pub(super) candidate: String,
    pub(super) copies: BTreeMap<String, String>,
    pub(super) versions: Vec<FileVersion>,
    pub(super) stable: PathBuf,
    pub(super) named: PathBuf,
    history: PathBuf,
    file: String,
}
impl Target {
    pub(super) fn capture(
        engine: &Engine,
        projection: &FileHistoryMoveProjection,
        materials: &Materials,
        frozen: &FrozenCandidate,
        slot: &str,
    ) -> Result<Self> {
        let participant = materials
            .record
            .disks
            .iter()
            .find(|disk| disk.identity.slot_id == slot)
            .ok_or_else(|| invalid("ordinary target is not an original move participant"))?;
        let disk = engine.disk(slot)?.clone();
        let identity = participant.identity.clone();
        let copies = materials
            .record
            .target_copy_identities
            .get(slot)
            .ok_or_else(|| invalid("ordinary target lacks its original staged copy identities"))?
            .clone();
        let versions = frozen
            .versions()
            .iter()
            .map(|evidence| {
                if !evidence
                    .placement
                    .route_slot_ids
                    .iter()
                    .any(|item| item == slot)
                {
                    return Err(invalid(
                        "ordinary target is absent from the frozen candidate route",
                    ));
                }
                Ok(evidence.version.clone())
            })
            .collect::<Result<Vec<_>>>()?;
        if versions.is_empty()
            || versions
                .iter()
                .any(|version| !copies.contains_key(&version.version_id))
            || versions.len() != copies.len()
        {
            return Err(invalid(
                "ordinary target version set differs from the original selected copies",
            ));
        }
        let file = projection.target_file_id().as_str().to_owned();
        let target =
            &frozen.state().repositories[&projection.capture().namespace().target_repository_id];
        let name = &projection.capture().namespace().target_name;
        let manifest = encode_metadata(
            &Intent {
                format: "file-history-move-ordinary-inputs",
                candidate_hash: frozen.hash(),
                choice_digest: frozen.choice_digest(),
                identity: &identity,
                file: &file,
                history: &target.history_tree_id,
                name,
                versions: versions
                    .iter()
                    .map(|version| {
                        Ok(VersionInput {
                            version_id: &version.version_id,
                            version_hash: object_hash("version", version)?,
                            original_copy: &copies[&version.version_id],
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
            },
            MANIFEST_BYTES,
        )?;
        let owner = Arc::clone(engine.disk_resources.get(&disk)?);
        let owner_inode = Inode::directory(&owner.root, owner.root.path())?;
        let path = owner.root.path().join(DIRECTORY);
        let stage = if owner.root.optional_metadata(&path)?.is_some() {
            Some(owner.root.private_child(&path)?)
        } else {
            None
        };
        let history = disk.root.join("histories").join(&target.history_tree_id);
        let stable = history.join("file-containers").join(&file);
        let named = stable.join(name);
        let target = Self {
            owner,
            config: engine.config.clone(),
            disk,
            identity,
            owner_inode,
            stage,
            manifest,
            candidate: frozen.hash().into(),
            copies,
            versions,
            stable,
            named,
            history,
            file,
        };
        target.check_owner()?;
        Ok(target)
    }
    pub(super) fn slot(&self) -> &str {
        &self.disk.slot_id
    }
    pub(super) fn stage(&self) -> Result<&DiskRoot> {
        self.stage
            .as_ref()
            .ok_or_else(|| invalid("ordinary move stage is absent"))
    }
    pub(super) fn check_owner(&self) -> Result<()> {
        let root = &self.owner.root;
        let named = DiskRoot::open(root.path())?;
        if Inode::directory(&named, named.path())? != self.owner_inode
            || self.disk.disk_id != self.identity.disk_id
            || self.disk.binding_generation != self.identity.binding_generation
        {
            return Err(invalid("ordinary move target root or registration changed"));
        }
        let lease = root.admit_io()?;
        let seal =
            validate_seal_observed(&self.config, &self.disk, &mut |_| lease.check_running())?;
        validate_stored_seal(&self.owner.seal, &seal)?;
        if let Some(stage) = &self.stage
            && Inode::directory(root, stage.path())? != Inode::directory(stage, stage.path())?
        {
            return Err(invalid("ordinary move stage directory was replaced"));
        }
        lease.check_running()
    }
    pub(super) fn verify_manifest(&self) -> Result<()> {
        self.check_owner()?;
        let stage = self.stage()?;
        if stage.read_bytes(&stage.path().join(MANIFEST), MANIFEST_BYTES)? != self.manifest {
            return Err(invalid(
                "ordinary target manifest differs from its original candidate",
            ));
        }
        self.check_owner()
    }
    pub(super) fn preflight(
        &self,
        engine: &Engine,
        materials: &Materials,
        native: &crate::business::NativeMoveDraft,
        scopes: &scopes::Scopes,
        frozen: &FrozenCandidate,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        self.check_owner()?;
        if self.stage.is_some() {
            self.verify_manifest()?;
        } else {
            if exists(&self.owner.root, &self.stable)? {
                return Err(invalid(
                    "unclaimed ordinary move file container is retained without adoption",
                ));
            }
            for version in &self.versions {
                if exists(&self.owner.root, &self.version_directory(version))? {
                    return Err(invalid(
                        "ordinary move version exists without its original manifest",
                    ));
                }
            }
        }
        let mut total = self.manifest.len() as u64 + 32 * 1024;
        let mut missing = if self.stage.is_none() {
            total
        } else {
            32 * 1024
        };
        for version in &self.versions {
            checkpoint(cancellation)?;
            let bytes = version
                .logical_size
                .checked_add(checksum_manifest_length(version)? as u64)
                .and_then(|bytes| bytes.checked_add(64 * 1024))
                .ok_or_else(limit)?;
            total = total.checked_add(bytes).ok_or_else(limit)?;
            if self.require_original_or_absent(version, cancellation)? {
                missing = missing.checked_add(bytes).ok_or_else(limit)?;
            }
        }
        let participant = materials
            .record
            .disks
            .iter()
            .find(|item| item.identity == self.identity)
            .expect("invariant: captured target retains its original participant");
        let required = inputs::scoped_storage_bytes(materials, participant, native, scopes)?
            .checked_add(
                engine
                    .history_move_choice
                    .as_ref()
                    .expect("invariant: selected move")
                    .storage_bytes(),
            )
            .and_then(|bytes| bytes.checked_add(frozen.retained_bytes(participant.carries_state())))
            .and_then(|bytes| bytes.checked_add(total))
            .ok_or_else(limit)?;
        if let Some(capacity) = self.disk.capacity_bytes
            && engine
                .used_bytes(self.slot())
                .checked_add(required)
                .ok_or_else(limit)?
                > capacity
        {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "ordinary copies plus all selected move material exceed configured capacity",
            ));
        }
        if missing
            .checked_add(self.config.metadata_reserve_bytes)
            .ok_or_else(limit)?
            > self.owner.root.available()?
        {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "ordinary move copies would consume metadata reserve",
            ));
        }
        Ok(())
    }
    pub(super) fn install_manifest(&mut self) -> Result<()> {
        if self.stage.is_none() {
            let path = self.owner.root.path().join(DIRECTORY);
            let inode = self.owner.root.create_private_directory_synced(&path)?;
            let stage = self.owner.root.private_child_matching(&path, inode)?;
            stage.write_atomic(&stage.path().join(MANIFEST), &self.manifest, false)?;
            self.stage = Some(stage);
        }
        self.verify_manifest()?;
        let stage = self.stage()?;
        stage
            .open_read_regular(&stage.path().join(MANIFEST))?
            .sync_all()?;
        stage.sync_dir(stage.path())?;
        self.owner.root.sync_dir(self.owner.root.path())?;
        Ok(())
    }
    fn container_record(&self) -> Result<Container> {
        let root = &self.owner.root;
        Ok(Container {
            format: "file-history-move-ordinary-container".into(),
            candidate_hash: self.candidate.clone(),
            stable: Inode::directory(root, &self.stable)?,
            named: Inode::directory(root, &self.named)?,
            versions: Inode::directory(root, &self.named.join("versions"))?,
        })
    }
    pub(super) fn verify_container(&self) -> Result<()> {
        self.verify_manifest()?;
        let stage = self.stage()?;
        let path = stage.path().join("container-origin.json");
        let original: Container =
            storage_contract::decode_strict(&stage.read_bytes(&path, MAX_RECORD)?)?;
        if original != self.container_record()? {
            return Err(invalid(
                "ordinary move container no longer has its original inodes",
            ));
        }
        if exists(
            &self.owner.root,
            &self
                .history
                .join(".zxs-layout/files")
                .join(format!("{}.json", self.file)),
        )? {
            return Err(invalid(
                "unpublished ordinary move container has an unexpected namespace locator",
            ));
        }
        self.check_owner()
    }
    pub(super) fn prepare_container(&self) -> Result<()> {
        let stage = self.stage()?;
        let origin = stage.path().join("container-origin.json");
        if stage.open_optional_read_regular(&origin)?.is_none() {
            if exists(&self.owner.root, &self.stable)? {
                return Err(invalid(
                    "ordinary move container lacks its original inode record; retain it",
                ));
            }
            let root = &self.owner.root;
            root.private_dir(
                self.stable
                    .parent()
                    .expect("invariant: stable container has a parent"),
            )?;
            root.create_private_directory_synced(&self.stable)?;
            root.create_private_directory_synced(&self.named)?;
            root.create_private_directory_synced(&self.named.join("versions"))?;
            stage.write_atomic(
                &origin,
                &encode_metadata(&self.container_record()?, MAX_RECORD)?,
                false,
            )?;
        }
        self.verify_container()?;
        stage.open_read_regular(&origin)?.sync_all()?;
        stage.sync_dir(stage.path())?;
        let root = &self.owner.root;
        root.sync_dir(self.stable.parent().expect("invariant: stable parent"))?;
        root.sync_dir(&self.history)?;
        root.sync_dir(
            self.history
                .parent()
                .expect("invariant: history collection parent"),
        )?;
        root.sync_dir(root.path())?;
        Ok(())
    }
    pub(super) fn version_directory(&self, version: &FileVersion) -> PathBuf {
        version_dir(&self.disk, &version.history_tree_id, &version.version_id)
    }
    pub(super) fn staged_version(&self, version: &FileVersion) -> PathBuf {
        self.disk
            .root
            .join(DIRECTORY)
            .join("versions")
            .join(&version.version_id)
    }
    pub(super) fn origin_path(&self, version: &FileVersion) -> PathBuf {
        self.disk
            .root
            .join(DIRECTORY)
            .join("origins")
            .join(format!("{}.json", version.version_id))
    }
    pub(super) fn retained_bytes(&self) -> Result<u64> {
        self.versions
            .iter()
            .try_fold(self.manifest.len() as u64 + 32 * 1024, |bytes, version| {
                let checksums = checksum_manifest_length(version)? as u64;
                bytes
                    .checked_add(version.logical_size)
                    .and_then(|bytes| bytes.checked_add(checksums))
                    .and_then(|bytes| bytes.checked_add(64 * 1024))
                    .ok_or_else(limit)
            })
    }
    pub(super) fn max_file_bytes(&self) -> u64 {
        self.config.max_file_bytes
    }
}
