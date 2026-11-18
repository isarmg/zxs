//! The ordinary destination of an actually Published Backup. All authoritative
//! directories were created and recorded before the common decision. This stage
//! fills only those entities, then installs their recorded names without replace.
use super::*;
use crate::container_storage::backup_databases::{PreparedDatabases, Scope};
use crate::engine::membership_transition::PublishedBackupLayout;
use crate::history_package::{install_preparation_bytes, preparation_path_exists};
use crate::{ContentState, FileVersion};
use std::{
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
use storage_contract::{HistoryTreeId, VersionId};

const MAX_METADATA: usize = 96 * 1024 * 1024;
const MAX_MANIFEST: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serialization(rename_all = "kebab-case")]
enum VersionFile {
    Descriptor,
    Checksums,
    Commit,
    Publication,
}
impl VersionFile {
    fn name(self) -> &'static EntryName {
        match self {
            Self::Descriptor => &layout::VERSION_DESCRIPTOR,
            Self::Checksums => &layout::VERSION_CHECKSUMS,
            Self::Commit => &layout::VERSION_COMMIT_PROOF,
            Self::Publication => &layout::VERSION_PUBLICATION_PROOF,
        }
    }
}
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serialization(tag = "kind", rename_all = "kebab-case")]
enum Key {
    Database {
        file: FileId,
    },
    Owner {
        file: FileId,
    },
    Body {
        file: FileId,
        version: VersionId,
    },
    Metadata {
        file: FileId,
        history: HistoryTreeId,
        version: VersionId,
        material: VersionFile,
    },
}
impl Key {
    fn file(&self) -> FileId {
        match self {
            Self::Database { file }
            | Self::Owner { file }
            | Self::Body { file, .. }
            | Self::Metadata { file, .. } => *file,
        }
    }
}
enum Data<'a> {
    Bytes(Vec<u8>),
    Body(&'a FileVersion),
}
impl Data<'_> {
    fn length(&self) -> u64 {
        match self {
            Self::Bytes(bytes) => bytes.len() as u64,
            Self::Body(version) => version.logical_size,
        }
    }
    fn hash(&self) -> storage_contract::ContentHash {
        match self {
            Self::Bytes(bytes) => storage_contract::ContentHash::from_digest(blake3::hash(bytes)),
            Self::Body(version) => version.content_hash,
        }
    }
}

pub(crate) struct BackupServingImage<'a> {
    source: &'a PublishedBackupLayout<'a>,
    stop: &'a AtomicBool,
    held: Observed,
    files: BTreeMap<Key, Data<'a>>,
    manifest: Vec<u8>,
    metadata_bytes: usize,
    pub(crate) bodies: u64,
    pub(crate) body_bytes: u64,
    pub(crate) directory_repositories: usize,
}

/// This source exists only inside a complete published target image. It keeps
/// global membership separate from the actual source-lifecycle metadata FD.
pub(crate) struct BackupServingVersion<'a> {
    image: &'a BackupServingImage<'a>,
    version: &'a FileVersion,
}
impl BackupServingVersion<'_> {
    pub(crate) fn version(&self) -> &FileVersion {
        self.version
    }
    pub(crate) fn global(&self) -> Result<&DiskRoot> {
        self.image.source.source().root()
    }
    pub(crate) fn root(&self) -> Result<&DiskRoot> {
        self.image.current_zxs()
    }
    pub(crate) fn directory(&self) -> Result<PathBuf> {
        Ok(self.root()?.path().join(layout::source_version(
            self.version.history_tree_id.parse()?, self.version.version_id.parse()?,
        )))
    }
    pub(crate) fn verify(&self) -> Result<()> {
        self.image.require_occurrence(self.version.file_id.parse()?, self.image.stop)
    }
}

impl<'a> BackupServingImage<'a> {
    pub(crate) fn capture(source: &'a PublishedBackupLayout<'a>, stop: &'a AtomicBool)
        -> Result<Self>
    {
        source.require(stop)?;
        let held = source.layout().hold(source.source(), stop)?;
        let mut image = Self { source, stop, held, files: BTreeMap::new(), manifest: Vec::new(),
            metadata_bytes: 0, bodies: 0, body_bytes: 0, directory_repositories: 0 };
        for database in PreparedDatabases::capture(source.published(), stop)?.into_databases() {
            let (scope, bytes) = database.into_parts();
            match scope {
                Scope::File { file, history } => {
                    let file = file.parse::<FileId>()?;
                    if source.published().state().file_containers.get(&file)
                        .is_none_or(|record| record.history_tree_id.to_string() != history)
                    {
                        return Err(Failure::BackupLayoutEvidence.into());
                    }
                    image.add(Key::Database { file }, Data::Bytes(bytes))?;
                    image.add(Key::Owner { file }, Data::Bytes(crate::canonical_json(
                        &file.to_string(),
                    )?))?;
                }
                Scope::Directory { .. } => return Err(Failure::BackupLayoutUnsupported.into()),
            }
        }
        let state = source.published().state();
        for (id, version) in state.versions.iter() {
            preparation_checkpoint(stop)?;
            if state.content_states.get(id).is_some_and(|state| *state != ContentState::Available) {
                return Err(Failure::BackupLayoutUnsupported.into());
            }
            let file = version.file_id.parse::<FileId>()?;
            let history = version.history_tree_id.parse::<HistoryTreeId>()?;
            let version_id = version.version_id.parse::<VersionId>()?;
            let record = state.file_containers.get(&file).ok_or(Failure::BackupLayoutEvidence)?;
            if record.history_tree_id != history || !image.held._files.contains_key(&file) {
                return Err(Failure::BackupLayoutEvidence.into());
            }
            let package = source.source().root()?;
            let directory = package.path()
                .join(layout::backup_package_version(history, version_id));
            let proof = crate::journal::local_proof_directory_at(
                package, &directory, source.candidate_hash(),
            )?;
            for material in [VersionFile::Descriptor, VersionFile::Checksums,
                VersionFile::Commit, VersionFile::Publication]
            {
                let maximum = if material == VersionFile::Checksums {
                    crate::disk::checksum_manifest_length(version)?
                } else {
                    crate::disk::MAX_METADATA
                };
                let parent = match material {
                    VersionFile::Commit | VersionFile::Publication => &proof,
                    VersionFile::Descriptor | VersionFile::Checksums => &directory,
                };
                if maximum > MAX_METADATA.saturating_sub(image.metadata_bytes) {
                    return Err(Failure::ContainerBudget.into());
                }
                let bytes = package.read_bytes(&parent.join(material.name().as_path()), maximum)?;
                match material {
                    VersionFile::Descriptor => {
                        let saved: FileVersion = crate::journal::decode_proof(&bytes)?;
                        if &saved != version {
                            return Err(Failure::BackupLayoutEvidence.into());
                        }
                    }
                    VersionFile::Checksums => {
                        if blake3::hash(&bytes).to_hex().as_str() != version.checksum_manifest_hash
                        {
                            return Err(Failure::BackupLayoutEvidence.into());
                        }
                        crate::disk::decode_authenticated_checksums(&bytes, version)?;
                    }
                    VersionFile::Commit => {
                        let proof: crate::journal::CommitProof =
                            crate::journal::decode_proof(&bytes)?;
                        if proof.candidate_hash != source.candidate_hash() {
                            return Err(Failure::BackupLayoutEvidence.into());
                        }
                    }
                    VersionFile::Publication => {
                        let proof: crate::journal::LocalPublication =
                            crate::journal::decode_proof(&bytes)?;
                        if crate::object_hash("candidate", &proof.candidate)?
                                != source.candidate_hash()
                            || crate::object_hash("publication", &proof.publication)?
                                != source.publication_hash()
                        {
                            return Err(Failure::BackupLayoutEvidence.into());
                        }
                    }
                }
                image.add(Key::Metadata { file, history, version: version_id, material },
                    Data::Bytes(bytes))?;
            }
            image.add(Key::Body { file, version: version_id }, Data::Body(version))?;
            image.bodies = image.bodies.checked_add(1).ok_or(Failure::ContainerBudget)?;
            image.body_bytes = image.body_bytes.checked_add(version.logical_size)
                .ok_or(Failure::ContainerBudget)?;
        }
        #[derive(Serialize)]
        struct Descriptor<'b> {
            key: &'b Key,
            bytes: u64,
            hash: storage_contract::ContentHash,
        }
        #[derive(Serialize)]
        struct Manifest<'b> {
            publication: &'b str,
            layout: &'b BackupTargetLayout,
            files: Vec<Descriptor<'b>>,
        }
        image.manifest = crate::canonical_json(&Manifest {
            publication: source.publication_hash(), layout: source.layout(),
            files: image.files.iter().map(|(key, value)| Descriptor {
                key, bytes: value.length(), hash: value.hash(),
            }).collect(),
        })?;
        if image.manifest.len() > MAX_MANIFEST {
            return Err(Failure::ContainerBudget.into());
        }
        image.inventory(stop)?;
        source.require(stop)?;
        Ok(image)
    }

    fn add(&mut self, key: Key, value: Data<'a>) -> Result<()> {
        if let Data::Bytes(bytes) = &value {
            self.metadata_bytes = self.metadata_bytes.checked_add(bytes.len())
                .ok_or(Failure::ContainerBudget)?;
        }
        if self.metadata_bytes > MAX_METADATA || self.files.len() >= MAX_ENTITIES
            || self.files.insert(key, value).is_some()
        {
            return Err(Failure::ContainerBudget.into());
        }
        Ok(())
    }
    pub(crate) fn hash(&self) -> String {
        blake3::hash(&self.manifest).to_hex().to_string()
    }
    pub(crate) fn whole_files(&self) -> usize {
        self.held._files.len()
    }

    fn require_occurrence(&self, file: FileId, stop: &AtomicBool) -> Result<()> {
        self.source.require(stop)?;
        // The borrowed full shape cannot change. Only re-open physical facts
        // which can change externally; do not rehash/re-scan every other FILE.
        let _temporary = reserve(8)?;
        let source = self.source.source();
        let management = management(source)?;
        let configured = crate::disk_names::configuration::ConfiguredDiskNames::new(source.config())
            .map_err(crate::Error::ConfiguredDiskName)?;
        let path = configured.source_path(source.disk()).map_err(crate::Error::ConfiguredDiskName)?;
        let mut installed = true;
        for (index, component) in layout::container::source_components(&path).enumerate() {
            preparation_checkpoint(stop)?;
            let value = &self.source.layout().prefixes[index];
            let parent = if index == 0 {
                source.root()?
            } else {
                &self.held._prefixes[index - 1].0.root
            };
            let node = locate(
                &management, &layout::container::prefix_preparation(value.preparation),
                parent, &component, installed, value.entity)?;
            if node.root.directory_identity(node.root.path())?
                != self.held._prefixes[index].0.root.directory_identity(
                    self.held._prefixes[index].0.root.path())?
            {
                return Err(Failure::ContainerIdentity.into());
            }
            self.held._prefixes[index].1.verify(&node.root)?;
            installed = node.installed;
        }
        let logical = source.state().file_containers.get(&file)
            .ok_or(Failure::BackupLayoutEvidence)?;
        let mut ancestors = Vec::new();
        let mut cursor = logical.namespace.parent.ordinary().ok_or(Failure::BackupLayoutEvidence)?;
        while let Parent::Directory(entry) = cursor {
            let record = source.state().namespace_directories.get(&entry.to_string())
                .ok_or(Failure::BackupLayoutEvidence)?;
            ancestors.push(entry);
            cursor = record.parent;
        }
        let mut parent = self.held._prefixes.last()
            .ok_or(Failure::BackupLayoutEvidence)?.1.common();
        for entry in ancestors.into_iter().rev() {
            preparation_checkpoint(stop)?;
            let logical = &source.state().namespace_directories[&entry.to_string()];
            let value = &self.source.layout().directories[&entry];
            if entity(parent.directory_identity(parent.path())?) != value.parent_entity {
                return Err(Failure::ContainerIdentity.into());
            }
            let node = locate(&management,
                &layout::container::namespace_preparation(value.preparation), parent,
                &layout::namespace::directory(&logical.name), installed, value.entity)?;
            parent = &self.held._directories[&entry].root;
            if parent.directory_identity(parent.path())?
                != node.root.directory_identity(node.root.path())?
            {
                return Err(Failure::ContainerIdentity.into());
            }
            installed = node.installed;
        }
        let value = &self.source.layout().files[&file];
        let node = locate(&management, &layout::container::preparation(value.preparation),
            parent, &layout::namespace::directory(&logical.namespace.name),
            installed, value.entity)?;
        let held = &self.held._files[&file].root;
        if held.directory_identity(held.path())? != node.root.directory_identity(node.root.path())?
        {
            return Err(Failure::ContainerIdentity.into());
        }
        Ok(())
    }

    fn current_zxs(&self) -> Result<&DiskRoot> {
        self.held._prefixes.last().map(|(_, areas)| areas.current_zxs().as_ref())
            .ok_or_else(|| Failure::BackupLayoutEvidence.into())
    }
    fn root(&self, key: &Key) -> Result<&DiskRoot> {
        match key {
            Key::Metadata { .. } => self.current_zxs(),
            Key::Database { file } | Key::Owner { file } | Key::Body { file, .. } => {
                self.held._files.get(file).map(|node| &node.root)
                    .ok_or_else(|| Failure::BackupLayoutEvidence.into())
            }
        }
    }
    fn relative(&self, key: &Key) -> Result<PathBuf> {
        match key {
            Key::Database { .. } => {
                Ok(PathBuf::from(crate::container_storage::file_database::NAME))
            }
            Key::Owner { .. } => Ok(PathBuf::from(crate::container_storage::OWNER_MARKER)),
            Key::Body { file, version } => {
                let state = self.source.published().state();
                let value = state.versions.get(&version.to_string())
                    .ok_or(Failure::BackupLayoutEvidence)?;
                if value.file_id.parse::<FileId>()? != *file
                    || !state.file_containers.contains_key(file)
                {
                    return Err(Failure::BackupLayoutEvidence.into());
                }
                let root = self.root(key)?;
                let path = crate::container_storage::installed_version_path(root.path(), value)?;
                path.strip_prefix(root.path()).map(Path::to_owned)
                    .map_err(|_| Failure::BackupLayoutEvidence.into())
            }
            Key::Metadata { history, version, material, .. } => {
                Ok(layout::source_version(*history, *version).join(material.name().as_path()))
            }
        }
    }
    fn require_root(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<()> {
        self.source.require(stop)?;
        if root.directory_identity(root.path())?
            != self.source.source().root()?.directory_identity(self.source.source().root()?.path())?
        {
            return Err(Failure::BackupRootEvidence.into());
        }
        self.source.layout().observe(self.source.source(), stop)
    }

    fn inventory(&self, stop: &AtomicBool) -> Result<()> {
        let mut management = BTreeSet::new();
        let mut files: BTreeMap<FileId, BTreeSet<PathBuf>> = self.held._files.keys()
            .map(|file| (*file, BTreeSet::from([layout::VERSIONS.as_path().to_owned()])))
            .collect();
        for key in self.files.keys() {
            let paths = match key {
                Key::Metadata { .. } => &mut management,
                _ => files.get_mut(&key.file()).ok_or(Failure::BackupLayoutEvidence)?,
            };
            let relative = self.relative(key)?;
            paths.insert(relative.clone());
            for parent in relative.ancestors().skip(1).filter(|p| !p.as_os_str().is_empty()) {
                paths.insert(parent.to_owned());
            }
        }
        let mut count = 0usize;
        let mut temporaries = 0usize;
        inventory(self.current_zxs()?, &management, stop, &mut count, &mut temporaries)?;
        for (file, paths) in files {
            inventory(&self.held._files[&file].root, &paths, stop, &mut count, &mut temporaries)?;
        }
        Ok(())
    }

    pub(crate) fn require_absent(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<()> {
        self.require_root(root, stop)?;
        self.inventory(stop)?;
        if self.held._prefixes.iter().any(|(node, _)| node.installed)
            || self.held._directories.values().any(|node| node.installed)
            || self.held._files.values().any(|node| node.installed)
        {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        for key in self.files.keys() {
            let held = self.root(key)?;
            if preparation_path_exists(held, &held.path().join(self.relative(key)?))? {
                return Err(Failure::BackupRootUnrecorded.into());
            }
        }
        Ok(())
    }

    pub(crate) fn require_capacity(
        &self, root: &DiskRoot, reserve: u64, stop: &AtomicBool,
    ) -> Result<()> {
        self.require_root(root, stop)?;
        self.inventory(stop)?;
        let mut missing = 0u64;
        let mut largest = 0u64;
        for (key, data) in &self.files {
            preparation_checkpoint(stop)?;
            let held = self.root(key)?;
            let path = held.path().join(self.relative(key)?);
            if preparation_path_exists(held, &path)? {
                self.verify_file(key, data, stop)?;
            } else {
                missing = missing.checked_add(data.length()).ok_or(Failure::ContainerBudget)?;
                largest = largest.max(data.length());
            }
        }
        let needed = missing.checked_add(largest).and_then(|v| v.checked_add(reserve))
            .and_then(|v| v.checked_add(MAX_MANIFEST as u64 + 1024 * 1024))
            .and_then(|v| v.checked_add(self.files.len() as u64 * 8192))
            .ok_or(Failure::ContainerBudget)?;
        if root.available()? < needed {
            return Err(Failure::BackupRootSpace.into());
        }
        Ok(())
    }

    fn verify_file(&self, key: &Key, data: &Data<'_>, stop: &AtomicBool) -> Result<FileIdentity> {
        preparation_checkpoint(stop)?;
        let root = self.root(key)?;
        let path = root.path().join(self.relative(key)?);
        let mut file = root.open_read_regular(&path)?;
        let metadata = file.metadata()?;
        let identity = FileIdentity::from_metadata(&metadata);
        let stamp = crate::file_stamp::FileStamp::from_metadata(&metadata);
        if metadata.nlink() != 1 || metadata.len() != data.length() {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        match data {
            Data::Bytes(bytes) => {
                let mut actual = vec![0; bytes.len()];
                file.read_exact(&mut actual)?;
                if actual != *bytes || file.read(&mut [0; 1])? != 0 {
                    return Err(Failure::BackupLayoutEvidence.into());
                }
            }
            Data::Body(version) => {
                let checks = self.files.get(&Key::Metadata { file: key.file(),
                    history: version.history_tree_id.parse()?, version: version.version_id.parse()?,
                    material: VersionFile::Checksums }).ok_or(Failure::BackupLayoutEvidence)?;
                let Data::Bytes(checks) = checks else {
                    return Err(Failure::BackupLayoutEvidence.into());
                };
                let manifest = crate::disk::decode_authenticated_checksums(checks, version)?;
                file = crate::disk::verify_open_payload_observed(file, manifest, version,
                    &mut |_| preparation_checkpoint(stop))?;
            }
        }
        if crate::file_stamp::FileStamp::from_metadata(&file.metadata()?) != stamp
            || FileIdentity::from_metadata(&root.open_read_regular(&path)?.metadata()?) != identity
        {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        file.check_running()?;
        Ok(identity)
    }

    pub(crate) fn install(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<()> {
        self.require_root(root, stop)?;
        self.inventory(stop)?;
        for (key, data) in &self.files {
            preparation_checkpoint(stop)?;
            let held = self.root(key)?;
            let destination = held.path().join(self.relative(key)?);
            if preparation_path_exists(held, &destination)? {
                self.verify_file(key, data, stop)?;
                continue;
            }
            held.private_dir(destination.parent().ok_or(Failure::BackupLayoutEvidence)?)?;
            match data {
                Data::Bytes(bytes) => install_preparation_bytes(held, &destination, bytes)?,
                Data::Body(version) => {
                    let source = root.path().join(layout::backup_package_version(
                        version.history_tree_id.parse()?, version.version_id.parse()?,
                    )).join(layout::PREPARED_CONTENT.as_path()).join(&version.creation_name);
                    let mut input = Input { file: root.open_read_regular(&source)?, stop };
                    held.copy_verified_bytes_synced(&destination, &mut input,
                        platform_io::payload::Expectation { bytes: version.logical_size,
                            blake3: Some(version.content_hash.digest()) })?;
                }
            }
            self.verify_file(key, data, stop)?;
        }
        // Complete content, local proof and database validation precedes the
        // first public name. A later failure leaves the same decided stages.
        self.verify_materials(true, stop)?;
        self.install_names(stop)?;
        self.require_root(root, stop)?;
        Ok(())
    }

    fn verify_materials(&self, durable: bool, stop: &AtomicBool) -> Result<()> {
        self.inventory(stop)?;
        for (key, data) in &self.files {
            self.verify_file(key, data, stop)?;
            if let Data::Body(version) = data {
                let material = BackupServingVersion { image: self, version };
                crate::journal::verify_backup_serving_publication(&material,
                    &mut |_| preparation_checkpoint(stop))?;
            }
            let held = self.root(key)?;
            let path = held.path().join(self.relative(key)?);
            if durable {
                held.open_read_regular(&path)?.sync_all()?;
                held.sync_dir(path.parent().ok_or(Failure::BackupLayoutEvidence)?)?;
            }
        }
        Ok(())
    }

    fn install_names(&self, stop: &AtomicBool) -> Result<()> {
        let source = self.source.source();
        let record = self.source.layout();
        let management = management(source)?;
        let configured = crate::disk_names::configuration::ConfiguredDiskNames::new(source.config())
            .map_err(crate::Error::ConfiguredDiskName)?;
        let path = configured.source_path(source.disk()).map_err(crate::Error::ConfiguredDiskName)?;
        let components = layout::container::source_components(&path).collect::<Vec<_>>();
        for (index, ((node, _), value)) in
            self.held._prefixes.iter().zip(&record.prefixes).enumerate()
        {
            preparation_checkpoint(stop)?;
            let parent = if index == 0 {
                source.root()?
            } else {
                &self.held._prefixes[index - 1].0.root
            };
            install_name(&management, &layout::container::prefix_preparation(value.preparation),
                parent, &components[index], node, stop)?;
        }
        let common = self.held._prefixes.last().ok_or(Failure::BackupLayoutEvidence)?.1.common();
        for entry in ordered_directories(source.state())? {
            let logical = &source.state().namespace_directories[&entry.to_string()];
            let parent = parent(common, &self.held._directories, logical.parent)?;
            let value = &record.directories[&entry];
            install_name(&management, &layout::container::namespace_preparation(value.preparation),
                parent, &layout::namespace::directory(&logical.name),
                &self.held._directories[&entry], stop)?;
        }
        for (file, value) in &record.files {
            let logical = &source.state().file_containers[file];
            let parent = parent(common, &self.held._directories,
                logical.namespace.parent.ordinary().ok_or(Failure::BackupLayoutEvidence)?)?;
            install_name(&management, &layout::container::preparation(value.preparation),
                parent, &layout::namespace::directory(&logical.namespace.name),
                &self.held._files[file], stop)?;
        }
        record.observe(source, stop)
    }

    pub(crate) fn verify(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<Vec<u8>> {
        self.require_root(root, stop)?;
        let observed = self.source.layout().hold(self.source.source(), stop)?;
        if observed._prefixes.iter().any(|(node, _)| !node.installed)
            || observed._directories.values().any(|node| !node.installed)
            || observed._files.values().any(|node| !node.installed)
        {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        self.verify_materials(false, stop)?;
        #[derive(Serialize)]
        struct Entry<'b> {
            key: &'b Key,
            entity: ContainerEntity,
        }
        #[derive(Serialize)]
        struct Completion<'b> {
            recipe_hash: String,
            layout: &'b BackupTargetLayout,
            entries: Vec<Entry<'b>>,
        }
        let mut entries = Vec::with_capacity(self.files.len());
        for (key, data) in &self.files {
            entries.push(Entry { key, entity: entity(self.verify_file(key, data, stop)?) });
        }
        let bytes = crate::canonical_json(&Completion {
            recipe_hash: self.hash(), layout: self.source.layout(), entries,
        })?;
        if bytes.len() > MAX_MANIFEST {
            return Err(Failure::ContainerBudget.into());
        }
        self.require_root(root, stop)?;
        Ok(bytes)
    }
}

struct Input<'a> {
    file: crate::engine::ReadFile,
    stop: &'a AtomicBool,
}
impl Read for Input<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        preparation_checkpoint(self.stop).map_err(std::io::Error::other)?;
        self.file.read(bytes)
    }
}
fn install_name(
    source: &DiskRoot, stage: &EntryName, parent: &DiskRoot, name: &EntryName,
    observed: &HeldNode, stop: &AtomicBool,
) -> Result<()> {
    preparation_checkpoint(stop)?;
    let expected = observed.root.directory_identity(observed.root.path())?;
    let from = source.path().join(stage.as_path());
    let to = parent.path().join(name.as_path());
    if observed.installed {
        parent.private_child_matching(&to, expected)?;
        parent.sync_dir(parent.path())?;
        source.sync_dir(source.path())?;
    } else {
        source.private_child_matching(&from, expected)?;
        if parent.optional_metadata(&to)?.is_some() {
            return Err(Failure::ContainerIdentity.into());
        }
        source.rename_to_held_synced(&from, parent, &to)?;
        parent.private_child_matching(&to, expected)?;
    }
    Ok(())
}
fn inventory(
    root: &DiskRoot, expected: &BTreeSet<PathBuf>, stop: &AtomicBool,
    count: &mut usize, temporaries: &mut usize,
) -> Result<()> {
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        preparation_checkpoint(stop)?;
        let parent = root.private_child(&root.path().join(&relative))?;
        for item in parent.entries(parent.path())? {
            preparation_checkpoint(stop)?;
            let name = item.map_err(crate::disk::fs_error)?;
            *count = count.checked_add(1).ok_or(Failure::ContainerBudget)?;
            if *count > MAX_ENTITIES {
                return Err(Failure::ContainerBudget.into());
            }
            let child = relative.join(name.as_path());
            let metadata = parent.optional_metadata(&parent.path().join(name.as_path()))?
                .ok_or(Failure::BackupLayoutEvidence)?;
            if expected.contains(&child) {
                if metadata.is_dir() {
                    pending.push(child);
                } else if !metadata.is_file() || metadata.nlink() != 1 {
                    return Err(Failure::BackupLayoutEvidence.into());
                }
            } else if platform_io::fs_safety::AtomicFile::is_temporary_name(&name)
                && metadata.is_file() && metadata.nlink() == 1
            {
                *temporaries = temporaries.checked_add(1).ok_or(Failure::ContainerBudget)?;
                if *temporaries > 1024 {
                    return Err(Failure::ContainerBudget.into());
                }
            } else {
                return Err(Failure::BackupLayoutEvidence.into());
            }
        }
    }
    Ok(())
}
