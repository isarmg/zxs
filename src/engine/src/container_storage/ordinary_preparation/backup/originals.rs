//! Original FILE stores in the actual published Backup switch. The original
//! frozen route and its permanent namespace select each held occurrence. Target
//! replicas, current Config guesses and legacy locators cannot supply it.
use super::*;
use crate::container_storage::{backup_databases, file_database};
use crate::database_install::{InstallAdmission, PreparedInstall, Scope};
use crate::disk::{FileStamp, HeldRegular};
use crate::engine::fresh_layout::directory_error;
use crate::engine::membership_transition::{PublishedBackupOriginal, PublishedBackupPayload};
use crate::state_projection::ProjectionSnapshot;
use std::sync::Arc;

const MAX_COMPLETE: usize = 4 * 1024 * 1024;
const COMPLETION_RESERVE: u64 = 2 * MAX_COMPLETE as u64 + 1024 * 1024;
const MAX_CONTAINER_ENTRIES: usize = 65_536;

fn require_database_names(root: &DiskRoot, stop: &AtomicBool) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;

    for (count, entry) in root.entries(root.path())?.enumerate() {
        preparation_checkpoint(stop)?;
        if count == MAX_CONTAINER_ENTRIES {
            return Err(Failure::ContainerBudget.into());
        }
        let name = entry.map_err(crate::disk::fs_error)?;
        let bytes = name.as_os_str().as_bytes();
        // SQLite companions may contain uncheckpointed state. The preserving
        // installer owns only NAME; it must neither ignore nor clean siblings.
        // Its private retention/stage is outside this held FILE container.
        if bytes.starts_with(b".zxs-")
            && bytes.windows(b".sqlite".len()).any(|part| part == b".sqlite")
            && bytes != file_database::NAME.as_bytes()
        {
            return Err(Failure::BackupLayoutEvidence.into());
        }
    }
    Ok(())
}

struct File {
    file: FileId,
    root: DiskRoot,
    current: DiskRoot,
    history: std::path::PathBuf,
    common: DiskRoot,
    prefixes: Vec<(DiskRoot, areas::HeldAreas, EntryName, ContainerEntity)>,
    directories: Vec<(DiskRoot, EntryName, ContainerEntity)>,
    name: EntryName,
    entity: ContainerEntity,
}
impl File {
    fn capture(source: &PublishedBackupOriginal<'_>, file: FileId) -> Result<Self> {
        Self::capture_source(source.config(), source.original().state(), source.root(),
            source.disk(), file)
    }

    fn capture_source(
        config: &crate::ValidatedConfig, state: &State, source_root: &DiskRoot,
        selected: &crate::DiskConfig, file: FileId,
    ) -> Result<Self> {
        let disk = selected.disk_id.parse::<DiskId>()?;
        crate::engine::fresh_layout::containers::require_backup_original_geometry(
            config, state, file, disk,
        )?;
        let logical = state.file_containers.get(&file).ok_or(Failure::BackupLayoutEvidence)?;
        let record = logical.replicas.get(&disk).ok_or(Failure::BackupLayoutEvidence)?;
        let configured = crate::disk_names::configuration::ConfiguredDiskNames::new(config)
            .map_err(crate::Error::ConfiguredDiskName)?;
        let chain = configured.source_path(disk).map_err(crate::Error::ConfiguredDiskName)?;
        let mut prefixes: Vec<(DiskRoot, areas::HeldAreas, EntryName, ContainerEntity)> =
            Vec::new();
        for (prefix, name) in record.prefixes.iter()
            .zip(layout::container::source_components(&chain))
        {
            let parent = prefixes.last().map(|value| &value.0).unwrap_or(source_root);
            let root = parent.private_entry_matching(&name, identity(prefix.entity))
                .map_err(directory_error)?;
            let areas = areas::HeldAreas::observe(&root, prefix.areas)?;
            prefixes.push((root, areas, name, prefix.entity));
        }
        let binding = &config.storage_roots[&disk];
        let (common, current) = match prefixes.last() {
            Some((_, areas, _, _)) => (
                areas.common().retained(areas.common().path().to_owned())?,
                areas.current_zxs().retained(areas.current_zxs().path().to_owned())?,
            ),
            None => {
                let roots = &binding.roots;
                let to_identity = |value: storage_contract::working_tree::LocalEntityIdentity| {
                    FileIdentity { device: value.device(), inode: value.inode() }
                };
                let common = source_root.private_entry_matching(
                    &layout::COMMON_DATA_ROOT, to_identity(roots.common_data.entity),
                ).map_err(directory_error)?;
                let management = source_root.private_entry_matching(
                    &layout::ZXS_DATA_ROOT, to_identity(roots.zxs_data.entity),
                ).map_err(directory_error)?;
                let current = management.private_entry_matching(
                    &layout::COMMON_DATA_ROOT,
                    to_identity(roots.zxs_lifecycles.common_data.entity),
                ).map_err(directory_error)?;
                (common, current)
            }
        };
        let parent = logical.namespace.parent.ordinary().ok_or(Failure::BackupLayoutEvidence)?;
        let chain = match parent {
            Parent::Root => Vec::new(),
            Parent::Directory(entry) => {
                crate::engine::fresh_layout::directories::chain(state, entry)?
            }
        };
        let mut directories: Vec<(DiskRoot, EntryName, ContainerEntity)> = Vec::new();
        for logical in chain {
            let occurrence = logical.replicas.get(&disk).ok_or(Failure::BackupLayoutEvidence)?;
            if occurrence.root_id != record.root_id
                || occurrence.source_chain != binding.source_chain
                || occurrence.source_prefixes != record.prefixes
            {
                return Err(Failure::BackupLayoutEvidence.into());
            }
            let parent = directories.last().map(|value| &value.0).unwrap_or(&common);
            if entity(parent.directory_identity(parent.path())?) != occurrence.parent_entity {
                return Err(Failure::ContainerIdentity.into());
            }
            let name = layout::namespace::directory(&logical.name);
            let root = parent.private_entry_matching(&name, identity(occurrence.entity))
                .map_err(directory_error)?;
            directories.push((root, name, occurrence.entity));
        }
        let parent = directories.last().map(|value| &value.0).unwrap_or(&common);
        let name = layout::namespace::directory(&logical.namespace.name);
        let root = parent.private_entry_matching(&name, identity(record.entity))
            .map_err(directory_error)?;
        let history = current.path().join(logical.history_tree_id.to_string());
        current.private_child(&history)?;
        Ok(Self {
            file, root, current, history, common, prefixes, directories, name,
            entity: record.entity,
        })
    }

    fn verify(&self, source_root: &DiskRoot) -> Result<()> {
        let mut parent = source_root;
        for (root, areas, name, expected) in &self.prefixes {
            let observed = parent.private_entry_matching(name, identity(*expected))
                .map_err(directory_error)?;
            if root.directory_identity(root.path())?
                != observed.directory_identity(observed.path())?
            {
                return Err(Failure::ContainerIdentity.into());
            }
            areas.verify(root)?;
            parent = root;
        }
        // The source rechecks all five global roots; each prefix rechecks its
        // own five children. Retain these exact lifecycle FDs through the DB tail.
        let mut parent = &self.common;
        for (root, name, expected) in &self.directories {
            let observed = parent.private_entry_matching(name, identity(*expected))
                .map_err(directory_error)?;
            if root.directory_identity(root.path())?
                != observed.directory_identity(observed.path())?
            {
                return Err(Failure::ContainerIdentity.into());
            }
            parent = root;
        }
        let observed = parent.private_entry_matching(&self.name, identity(self.entity))
            .map_err(directory_error)?;
        if self.root.directory_identity(self.root.path())?
            != observed.directory_identity(observed.path())?
        {
            return Err(Failure::ContainerIdentity.into());
        }
        self.current.private_child(&self.history)?;
        Ok(())
    }
}

struct Object {
    file: File,
    bytes: u64,
    digest: String,
}

pub(crate) struct BackupOriginalCompletion(Vec<u8>);
impl BackupOriginalCompletion {
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.0
    }
}

pub(crate) struct BackupOriginalImage<'a> {
    source: &'a PublishedBackupOriginal<'a>,
    projection: ProjectionSnapshot<'a>,
    objects: Vec<Object>,
    installs: Vec<PreparedInstall>,
    recipe_hash: String,
    _reservation: crate::engine::PayloadProofReservation,
}
impl<'a> BackupOriginalImage<'a> {
    pub(crate) fn capture(source: &'a PublishedBackupOriginal<'a>, stop: &AtomicBool)
        -> Result<Self>
    {
        source.require(stop)?;
        let state = source.original().state();
        if !state.directory_git_repositories.is_empty() || !state.file_deletions.is_empty() {
            return Err(Failure::BackupLayoutUnsupported.into());
        }
        let mut selected = BTreeSet::new();
        for version in state.versions.values() {
            preparation_checkpoint(stop)?;
            if state.placements[&version.version_id].route_slot_ids
                .contains(&source.disk().slot_id)
            {
                selected.insert(version.file_id.parse::<FileId>()?);
            }
        }
        let mut descriptors = 0usize;
        for file in &selected {
            let record = &state.file_containers[file];
            let disk = source.disk().disk_id.parse::<DiskId>()?;
            let replica = record.replicas.get(&disk).ok_or(Failure::BackupLayoutEvidence)?;
            let parent = record.namespace.parent.ordinary().ok_or(Failure::BackupLayoutEvidence)?;
            let depth = match parent {
                Parent::Root => 0,
                Parent::Directory(entry) => {
                    crate::engine::fresh_layout::directories::chain(state, entry)?.len()
                }
            };
            descriptors = descriptors.checked_add(replica.prefixes.len() * 6 + depth + 4)
                .ok_or(Failure::ContainerBudget)?;
        }
        let reservation = reserve(descriptors)?;
        let projection = ProjectionSnapshot::new(state);
        let mut objects = Vec::with_capacity(selected.len());
        let mut installs = Vec::with_capacity(selected.len());
        for database in backup_databases::PreparedDatabases::capture(source.published(), stop)?
            .into_databases()
        {
            preparation_checkpoint(stop)?;
            let (scope, bytes) = database.into_parts();
            let backup_databases::Scope::File { file, history } = scope else {
                return Err(Failure::BackupLayoutUnsupported.into());
            };
            let id = file.parse::<FileId>()?;
            if !selected.remove(&id) {
                continue;
            }
            let held = File::capture(source, id)?;
            file_database::install::verify_held_container(&held.root, &file, projection.view())?;
            require_database_names(&held.root, stop)?;
            let length = bytes.len() as u64;
            let digest = blake3::hash(&bytes).to_hex().to_string();
            let install = PreparedInstall::capture_held(
                &held.current, &held.history, &held.root,
                (Scope::file(&history, &file)?, InstallAdmission::DecidedRecovery),
                Arc::new(bytes), file_database::MAX_BYTES,
                |old| file_database::verify_scope(old, &file, &history),
            )?;
            objects.push(Object {
                file: held, bytes: length, digest,
            });
            installs.push(install);
        }
        if !selected.is_empty() {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        #[derive(Serialize)]
        struct ObjectDescriptor<'b> {
            file: FileId,
            binding: &'b storage_contract::file_container::FileContainerRecord,
            bytes: u64,
            digest: &'b str,
        }
        #[derive(Serialize)]
        struct Recipe<'b> {
            root: &'b storage_contract::working_tree::RootBinding,
            original_sequence: u64,
            published_sequence: u64,
            objects: Vec<ObjectDescriptor<'b>>,
        }
        let disk = source.disk().disk_id.parse::<DiskId>()?;
        let recipe = crate::canonical_json(&Recipe {
            root: &source.config().storage_roots[&disk],
            original_sequence: state.sequence,
            published_sequence: source.published().state().sequence,
            objects: objects.iter().map(|object| ObjectDescriptor {
                file: object.file.file, binding: &state.file_containers[&object.file.file],
                bytes: object.bytes, digest: &object.digest,
            }).collect(),
        })?;
        if recipe.len() > MAX_COMPLETE {
            return Err(Failure::ContainerBudget.into());
        }
        source.require(stop)?;
        Ok(Self {
            source, projection, objects, installs,
            recipe_hash: blake3::hash(&recipe).to_hex().to_string(),
            _reservation: reservation,
        })
    }

    pub(crate) fn recipe_hash(&self) -> &str {
        &self.recipe_hash
    }

    pub(crate) fn require_capacity(
        &self, admission: InstallAdmission, reserve: u64, stop: &AtomicBool,
    ) -> Result<()> {
        self.source.require(stop)?;
        let mut total = COMPLETION_RESERVE.checked_add(reserve).ok_or(Failure::ContainerBudget)?;
        let mut histories = BTreeMap::new();
        for (object, install) in self.objects.iter().zip(&self.installs) {
            preparation_checkpoint(stop)?;
            object.file.verify(self.source.root())?;
            let (bytes, entries) = install.reservation();
            let entry = histories.entry(&object.file.history)
                .or_insert((&object.file.current, 0u64, 0usize));
            entry.1 = entry.1.checked_add(bytes).ok_or(Failure::ContainerBudget)?;
            entry.2 = entry.2.checked_add(entries).ok_or(Failure::ContainerBudget)?;
            total = total.checked_add(bytes).ok_or(Failure::ContainerBudget)?;
        }
        for (history, (root, bytes, entries)) in histories {
            admission.check(root, history, bytes, entries)?;
        }
        if self.source.root().available()? < total {
            return Err(Failure::BackupRootSpace.into());
        }
        Ok(())
    }

    fn read(&self, object: &Object, stop: &AtomicBool) -> Result<FileStamp> {
        self.source.require(stop)?;
        object.file.verify(self.source.root())?;
        file_database::install::verify_held_container(
            &object.file.root, &object.file.file.to_string(), self.projection.view(),
        )?;
        require_database_names(&object.file.root, stop)?;
        let root = &object.file.root;
        let path = root.path().join(file_database::NAME);
        let before = crate::file_stamp::FileStamp::from_metadata(
            &root.optional_metadata(&path)?.ok_or(Failure::BackupLayoutEvidence)?,
        );
        let mut actual = HeldRegular::capture(root, file_database::NAME, file_database::MAX_BYTES)?
            .ok_or(Failure::BackupLayoutEvidence)?;
        if actual.stamp().bytes != object.bytes || actual.stamp().digest != object.digest {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        let history = self.source.original().state().file_containers[&object.file.file]
            .history_tree_id.to_string();
        file_database::verify_scope(actual.bytes(), &object.file.file.to_string(), &history)?;
        actual.check_running()?;
        let named = root.open_read_regular(&path)?;
        let metadata = named.metadata()?;
        let identity = FileIdentity::from_metadata(&metadata);
        if crate::file_stamp::FileStamp::from_metadata(&metadata) != before
            || actual.stamp().device != identity.device
            || actual.stamp().inode != identity.inode
        {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        object.file.verify(self.source.root())?;
        let stamp = actual.stamp().clone();
        actual.release_bytes();
        self.source.require(stop)?;
        Ok(stamp)
    }

    pub(crate) fn apply_preserving(mut self, stop: &AtomicBool)
        -> Result<BackupOriginalCompletion>
    {
        for (object, install) in self.objects.iter().zip(std::mem::take(&mut self.installs)) {
            self.source.require(stop)?;
            object.file.verify(self.source.root())?;
            require_database_names(&object.file.root, stop)?;
            install.apply(&object.file.current, &mut |_| preparation_checkpoint(stop))?;
            self.read(object, stop)?;
        }
        self.verify(stop)
    }

    pub(crate) fn verify(&self, stop: &AtomicBool) -> Result<BackupOriginalCompletion> {
        let mut entries = Vec::with_capacity(self.objects.len());
        for object in &self.objects {
            entries.push(self.read(object, stop)?);
        }
        for (object, expected) in self.objects.iter().zip(&entries) {
            if self.read(object, stop)? != *expected {
                return Err(Failure::BackupLayoutEvidence.into());
            }
        }
        #[derive(Serialize)]
        struct Completion<'b> {
            recipe_hash: &'b str,
            entries: Vec<FileStamp>,
        }
        let bytes = crate::canonical_json(&Completion { recipe_hash: &self.recipe_hash, entries })?;
        if bytes.len() > MAX_COMPLETE {
            return Err(Failure::ContainerBudget.into());
        }
        self.source.require(stop)?;
        Ok(BackupOriginalCompletion(bytes))
    }
}

/// Long-lived held read of the exact native first payload. Construction is
/// impossible from a path, target report, or Config without its Publication.
pub(crate) struct BackupPayloadVersion<'a> {
    source: &'a PublishedBackupPayload<'a>,
    version: &'a crate::FileVersion,
    held: File,
    _reservation: crate::engine::PayloadProofReservation,
}
impl<'a> BackupPayloadVersion<'a> {
    pub(crate) fn observe(
        source: &'a PublishedBackupPayload<'a>, version: &'a crate::FileVersion,
        stop: &AtomicBool,
    ) -> Result<Self> {
        source.require(stop)?;
        if source.state().versions.get(&version.version_id) != Some(version) {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        let placement = source.state().placements.get(&version.version_id)
            .ok_or(Failure::BackupLayoutEvidence)?;
        if !placement.route_slot_ids.contains(&source.disk().slot_id)
            || placement.binding_generations.get(&source.disk().slot_id)
                != Some(&source.disk().binding_generation)
        {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        let file = version.file_id.parse::<FileId>()?;
        let record = source.state().file_containers.get(&file)
            .ok_or(Failure::BackupLayoutEvidence)?;
        if record.history_tree_id.to_string() != version.history_tree_id {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        let replica = record.replicas.get(&source.disk().disk_id.parse::<DiskId>()?)
            .ok_or(Failure::BackupLayoutEvidence)?;
        let parent = record.namespace.parent.ordinary().ok_or(Failure::BackupLayoutEvidence)?;
            let depth = match parent {
            Parent::Root => 0,
            Parent::Directory(entry) => {
                crate::engine::fresh_layout::directories::chain(source.state(), entry)?.len()
            }
        };
        let reservation = reserve(replica.prefixes.len() * 6 + depth + 4)?;
        let held = File::capture_source(source.config(), source.state(), source.root(),
            source.disk(), file)?;
        let result = Self { source, version, held, _reservation: reservation };
        result.verify(stop)?;
        Ok(result)
    }
    pub(crate) fn root(&self) -> &DiskRoot {
        &self.held.current
    }
    pub(crate) fn directory(&self) -> Result<std::path::PathBuf> {
        Ok(self.root().path().join(layout::source_version(
            self.version.history_tree_id.parse()?, self.version.version_id.parse()?,
        )))
    }
    pub(crate) fn container(&self) -> &DiskRoot {
        &self.held.root
    }
    pub(crate) fn body_path(&self) -> Result<std::path::PathBuf> {
        crate::container_storage::installed_version_path(self.container().path(), self.version)
    }
    pub(crate) fn verify(&self, stop: &AtomicBool) -> Result<()> {
        self.source.require(stop)?;
        self.held.verify(self.source.root())
    }
}
