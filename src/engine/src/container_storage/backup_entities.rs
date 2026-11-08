//! Complete target-local entity image below an isolated, non-serving root.
//! A source fence permits this copy, not registry admission or live placement.
//! Original packages remain untouched; no links, deduplication or source moves.
use super::{
    backup_databases::{PreparedDatabases, Scope},
    layout_goals,
    layout_locator::HistoryRelative,
};
use crate::{
    ContentState, Error, ErrorCode, FileVersion, Result, State, canonical_json,
    disk::*,
    history_package::{AdmittedState, preparation_checkpoint},
};
use data_serialization::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

mod placement;
mod recovery;
mod serving;
mod store;
pub(crate) use placement::EntityPlacement;
pub(crate) use recovery::MAX_RECOVERY_RANGE;
pub(crate) use serving::{PublishedImageBinding, capacity_before_choice};
const ROOT: &str = "activation-image";
const MANIFEST: &str = "manifest.json";
const MAX_ENTRIES: usize = 100_000;
const MAX_BYTES: usize = 96 * 1024 * 1024;
const MAX_MANIFEST: usize = 16 * 1024 * 1024;

fn invalid(detail: &str) -> Error {
    Error::new(ErrorCode::ContainerLayoutInvalid, detail)
}
fn budget() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "backup entity image exceeds its complete-unit metadata budget",
    )
}

#[derive(Serialize)]
struct Descriptor {
    #[serialization(with = "storage_contract::u64_string")]
    bytes: u64,
    digest: String,
}
enum Data<'a> {
    Bytes(Vec<u8>),
    Body {
        version: &'a FileVersion,
        source: PathBuf,
    },
}
impl Data<'_> {
    fn descriptor(&self) -> Descriptor {
        match self {
            Self::Bytes(bytes) => Descriptor {
                bytes: bytes.len() as u64,
                digest: blake3::hash(bytes).to_hex().to_string(),
            },
            Self::Body { version, .. } => Descriptor {
                bytes: version.logical_size,
                digest: version.content_hash.to_string(),
            },
        }
    }
}

/// Borrowed from exactly one target's independently admitted snapshot. This is
/// not a live DiskResource or a capability to write outside activation-image.
pub(crate) struct Image<'a> {
    root_name: &'static str,
    directories: BTreeSet<PathBuf>,
    files: BTreeMap<PathBuf, Data<'a>>,
    metadata_bytes: usize,
    manifest: Vec<u8>,
    placement: EntityPlacement,
    pub(crate) file_units: usize,
    pub(crate) directory_repositories: usize,
    pub(crate) bodies: u64,
    pub(crate) body_bytes: u64,
}
impl<'a> Image<'a> {
    pub(crate) fn capture(
        root: &DiskRoot,
        admitted: &'a AdmittedState,
        prefix: &[String],
        fence_hash: &str,
        stop: &AtomicBool,
    ) -> Result<Self> {
        let mut image = Self::build(root, admitted, prefix, ROOT, stop)?;
        #[derive(Serialize)]
        struct Manifest<'a> {
            format: &'static str,
            source_fence_hash: &'a str,
            directories: &'a BTreeSet<PathBuf>,
            files: BTreeMap<&'a PathBuf, Descriptor>,
        }
        image.manifest = canonical_json(&Manifest {
            format: "zxs-backup-target-entity-image",
            source_fence_hash: fence_hash,
            directories: &image.directories,
            files: image
                .files
                .iter()
                .map(|(path, data)| (path, data.descriptor()))
                .collect(),
        })?;
        if image.manifest.len() > MAX_MANIFEST {
            return Err(budget());
        }
        Ok(image)
    }
    fn build(
        root: &DiskRoot,
        admitted: &'a AdmittedState,
        prefix: &[String],
        root_name: &'static str,
        stop: &AtomicBool,
    ) -> Result<Self> {
        let state = admitted.state();
        let mut base = PathBuf::from("tree");
        for name in prefix {
            name_policy::validate_name(name)?;
            base.push(name);
        }
        if prefix.is_empty() {
            return Err(invalid("backup image lacks its direct-upstream prefix"));
        }
        base.push("histories");
        let mut image = Self {
            root_name,
            directories: BTreeSet::new(),
            files: BTreeMap::new(),
            metadata_bytes: 0,
            manifest: Vec::new(),
            placement: EntityPlacement::empty(),
            file_units: 0,
            directory_repositories: 0,
            bodies: 0,
            body_bytes: 0,
        };
        let paths = Paths::build(state, &base, &mut image, stop)?;
        image.placement = EntityPlacement::capture(state, &paths, root_name, stop)?;
        let databases = PreparedDatabases::capture(admitted, stop)?;
        for database in databases.into_databases() {
            preparation_checkpoint(stop)?;
            let (scope, bytes) = database.into_parts();
            let path = match scope {
                Scope::File { history, file } => {
                    image.file_units += 1;
                    paths
                        .files
                        .get(&(history, file))
                        .ok_or_else(|| invalid("complete file database has no single entity"))?
                        .join(super::file_database::NAME)
                }
                Scope::Directory {
                    history, directory, ..
                } => {
                    image.directory_repositories += 1;
                    paths
                        .directories
                        .get(&(history, directory))
                        .ok_or_else(|| {
                            invalid("directory repository has no complete forest entity")
                        })?
                        .join(super::directory_database::NAME)
                }
            };
            image.add_file(path, Data::Bytes(bytes))?;
        }
        for version in state.versions.values() {
            preparation_checkpoint(stop)?;
            let home = paths
                .files
                .get(&(version.history_tree_id.clone(), version.file_id.clone()))
                .ok_or_else(|| invalid("retained version has no whole-file entity"))?;
            let directory = home.join(".zxs-versions").join(&version.version_id);
            image.add_file(
                directory.join("version.json"),
                Data::Bytes(canonical_json(version)?),
            )?;
            image.add_file(
                directory.join("content-state.json"),
                Data::Bytes(canonical_json(&state.content_states[&version.version_id])?),
            )?;
            if state.content_states[&version.version_id] == ContentState::Available {
                image.add_payload(root, directory, version, stop)?;
            }
        }
        Ok(image)
    }
    fn add_payload(
        &mut self,
        root: &DiskRoot,
        directory: PathBuf,
        version: &'a FileVersion,
        stop: &AtomicBool,
    ) -> Result<()> {
        let source = root
            .path()
            .join("histories")
            .join(&version.history_tree_id)
            .join("versions")
            .join(&version.version_id);
        let checksum_bytes = checksum_manifest_length(version)?;
        if checksum_bytes > MAX_BYTES - self.metadata_bytes {
            return Err(budget());
        }
        crate::history_package::verify_package_payload_at(root, &source, version, &mut |_| {
            preparation_checkpoint(stop)
        })?;
        let checksums = root.read_bytes(&source.join("checksums.bin"), checksum_bytes)?;
        if checksums.len() != checksum_bytes
            || blake3::hash(&checksums).to_hex().as_str() != version.checksum_manifest_hash
        {
            return Err(Error::new(
                ErrorCode::ChecksumMismatch,
                "target-local checksum bytes changed before image capture",
            ));
        }
        self.add_file(directory.join("checksums.bin"), Data::Bytes(checksums))?;
        self.add_file(
            directory.join("content").join(&version.creation_name),
            Data::Body {
                version,
                source: source.join("content").join(&version.creation_name),
            },
        )?;
        self.bodies = self.bodies.checked_add(1).ok_or_else(budget)?;
        self.body_bytes = self
            .body_bytes
            .checked_add(version.logical_size)
            .ok_or_else(budget)?;
        Ok(())
    }
    fn charge_path(&mut self, path: &Path) -> Result<()> {
        let path = path
            .to_str()
            .ok_or_else(|| invalid("image path is not UTF-8"))?;
        HistoryRelative::parse(path)?;
        self.metadata_bytes = self
            .metadata_bytes
            .checked_add(path.len())
            .ok_or_else(budget)?;
        if self.metadata_bytes > MAX_BYTES
            || self.files.len() + self.directories.len() >= MAX_ENTRIES
        {
            return Err(budget());
        }
        Ok(())
    }
    fn add_directory(&mut self, path: PathBuf) -> Result<()> {
        for part in path.ancestors().filter(|part| !part.as_os_str().is_empty()) {
            if self.files.contains_key(part) {
                return Err(invalid("image directory overlaps a file"));
            }
            if !self.directories.contains(part) {
                self.charge_path(part)?;
                self.directories.insert(part.to_owned());
            }
        }
        Ok(())
    }
    fn add_file(&mut self, path: PathBuf, data: Data<'a>) -> Result<()> {
        if self.files.contains_key(&path)
            || self.directories.contains(&path)
            || path == Path::new(MANIFEST)
        {
            return Err(invalid("image plan repeats an immutable file or directory"));
        }
        self.charge_path(&path)?;
        if let Data::Bytes(bytes) = &data {
            self.metadata_bytes = self
                .metadata_bytes
                .checked_add(bytes.len())
                .ok_or_else(budget)?;
            if self.metadata_bytes > MAX_BYTES {
                return Err(budget());
            }
        }
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            self.add_directory(parent.to_owned())?;
        }
        self.files.insert(path, data);
        Ok(())
    }
    pub(crate) fn placement(&self) -> &EntityPlacement {
        &self.placement
    }
    pub(crate) fn manifest_hash(&self) -> String {
        blake3::hash(&self.manifest).to_hex().to_string()
    }
    pub(crate) fn require_capacity(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<()> {
        store::require_capacity(root, self, stop)
    }
    pub(crate) fn install(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<()> {
        store::install(root, self, stop)
    }
    pub(crate) fn verify(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<()> {
        store::verify(root, self, stop)
    }
}

struct Paths {
    files: BTreeMap<(String, String), PathBuf>,
    directories: BTreeMap<(String, String), PathBuf>,
}
impl Paths {
    fn build(state: &State, base: &Path, image: &mut Image<'_>, stop: &AtomicBool) -> Result<Self> {
        let goals = layout_goals::build(state, None)?;
        let mut paths = Self {
            files: BTreeMap::new(),
            directories: BTreeMap::new(),
        };
        for history in goals.histories.values() {
            preparation_checkpoint(stop)?;
            let anchor = base.join(&history.history_id);
            for (id, directory) in &history.directories {
                let path = anchor.join(directory.to.as_path());
                let key = (history.history_id.clone(), id.as_str().to_owned());
                if paths.directories.get(&key).is_some_and(|old| old != &path) {
                    return Err(invalid(
                        "one directory repository has conflicting image homes",
                    ));
                }
                paths.directories.insert(key, path.clone());
                image.add_directory(path)?;
            }
            for file in history.files.values() {
                let path = anchor
                    .join("directory-repositories")
                    .join(file.root_directory_id.as_str())
                    .join(file.root_relative_path.as_path());
                let key = (history.history_id.clone(), file.file_id.clone());
                // Multiple primary placements of one managed file cannot be
                // flattened into an apparently complete independent unit.
                if paths.files.insert(key, path).is_some() {
                    return Err(invalid(
                        "managed file is split across original primary layouts",
                    ));
                }
            }
        }
        for version in state.versions.values() {
            preparation_checkpoint(stop)?;
            let key = (version.history_tree_id.clone(), version.file_id.clone());
            if paths.files.contains_key(&key) {
                continue;
            }
            let path = base
                .join(&version.history_tree_id)
                .join("file-containers")
                .join(&version.file_id);
            let path = match state.file_locations.get(&version.file_id) {
                Some(location) => path
                    .join("namespace")
                    .join(name_policy::PortablePath::parse(location)?.as_str()),
                None => path.join("retained"),
            };
            paths.files.insert(key, path);
        }
        // Pure planner supplies all initialized parent/child repositories and
        // their empty directories. Uninitialized namespace directories remain
        // separately keyed by outer repository and preserve current spellings.
        for repository in state.repositories.values() {
            let branch = &state.branches[&repository.main_branch_id];
            let commit = &state.commits[&branch.head_commit_id];
            for entry in &commit.entries {
                preparation_checkpoint(stop)?;
                if entry.kind == storage_contract::NamespaceEntryKind::Directory {
                    image.add_directory(
                        base.join(&repository.history_tree_id)
                            .join("namespace-directories")
                            .join(&repository.repository_id)
                            .join(entry.path()),
                    )?;
                }
            }
        }
        for path in paths.files.values() {
            image.add_directory(path.clone())?;
        }
        Ok(paths)
    }
}
