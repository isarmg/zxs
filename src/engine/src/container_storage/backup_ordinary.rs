//! Complete target-only entities consumed by the ordinary history/container
//! readers. The caller supplies the actual native Publication and writer lease;
//! this storage recipe cannot activate Current or release a source fence.
use super::{
    backup_databases::{PreparedDatabases, Scope},
    layout_goals,
    layout_locator::{
        self, DirectoryId, DirectoryIntent, EntityInode, FileAddress, FileIntent, HistoryAnchor,
        HistoryRelative, LayoutBinding, StableRelative,
    },
};
use crate::{
    ContentState, Error, ErrorCode, FileVersion, Result, canonical_json,
    disk::*,
    history_package::{
        AdmittedState, install_preparation_bytes, preparation_checkpoint, preparation_path_exists,
    },
};
use data_serialization::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

mod store;

const MAX_ENTRIES: usize = 100_000;
const MAX_METADATA_BYTES: usize = 96 * 1024 * 1024;
const MAX_MANIFEST: usize = 32 * 1024 * 1024;
const SCOPES: [&str; 4] = [
    "backup-entities",
    "file-containers",
    "directory-repositories",
    ".zxs-layout",
];

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::ContainerLayoutInvalid, message)
}
fn budget() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "ordinary Backup entities exceed their bounded complete target budget",
    )
}
fn relative(path: &Path) -> Result<HistoryRelative> {
    HistoryRelative::parse(
        path.to_str()
            .ok_or_else(|| invalid("ordinary Backup path is not UTF-8"))?,
    )
}

/// Preserve the ordinary shared-forest namespace, so its existing absence
/// inventory still finds a file if a required locator is lost. The hop prefix
/// sits below the fixed root repository ID, not outside the history boundary.
pub(super) fn target_directory(
    goal: &HistoryRelative,
    prefix: &[String],
) -> Result<HistoryRelative> {
    let within = goal
        .as_path()
        .strip_prefix("directory-repositories")
        .map_err(|_| invalid("Backup directory goal has no ordinary repository root"))?;
    let mut parts = within.components();
    let root = parts
        .next()
        .expect("invariant: planned directory has a root repository ID");
    let mut path = PathBuf::from("directory-repositories").join(root.as_os_str());
    for name in prefix {
        path.push(name);
    }
    path.extend(parts);
    relative(&path)
}

/// One additional independent ordinary body copy plus its worst-case largest
/// atomic staging, bounded metadata and inode bindings before a NEW choice.
pub(crate) fn capacity_before_choice(body_bytes: u64) -> Result<u64> {
    body_bytes
        .checked_mul(2)
        .and_then(|n| n.checked_add(MAX_METADATA_BYTES as u64))
        .and_then(|n| n.checked_add(2 * MAX_MANIFEST as u64))
        .and_then(|n| n.checked_add(MAX_ENTRIES as u64 * 8192))
        .and_then(|n| n.checked_add(16 * 1024 * 1024))
        .ok_or_else(budget)
}

#[derive(Serialize)]
struct Descriptor {
    #[serialization(with = "storage_contract::u64_string")]
    bytes: u64,
    digest: String,
}
enum Data<'a> {
    Bytes(Vec<u8>),
    Body(&'a FileVersion),
}
impl Data<'_> {
    fn descriptor(&self) -> Descriptor {
        match self {
            Self::Bytes(bytes) => Descriptor {
                bytes: bytes.len() as u64,
                digest: blake3::hash(bytes).to_hex().to_string(),
            },
            Self::Body(version) => Descriptor {
                bytes: version.logical_size,
                digest: version.content_hash.to_string(),
            },
        }
    }
}
struct Directory {
    history: String,
    id: DirectoryId,
    to: HistoryRelative,
}
struct FileHome {
    history: String,
    id: String,
    path: PathBuf,
    address: FileAddress,
}

/// No disk JSON is decoded into this recipe. The exact admitted target State
/// and direct-upstream prefix determine every address; stored records only
/// compare against it. Native packages and the isolated images remain intact.
pub(crate) struct Image<'a> {
    files: BTreeMap<PathBuf, Data<'a>>,
    directories: BTreeSet<PathBuf>,
    histories: BTreeSet<String>,
    directory_homes: BTreeMap<(String, DirectoryId), Directory>,
    file_homes: BTreeMap<(String, String), FileHome>,
    locator_paths: BTreeSet<PathBuf>,
    binding: LayoutBinding,
    manifest: Vec<u8>,
    metadata_bytes: usize,
    pub(crate) bodies: u64,
    pub(crate) body_bytes: u64,
    pub(crate) directory_repositories: usize,
}

impl<'a> Image<'a> {
    pub(crate) fn capture(
        admitted: &'a AdmittedState,
        slot: &str,
        prefix: &[String],
        publication: &str,
        stop: &AtomicBool,
    ) -> Result<Self> {
        name_policy::validate_slot(slot)?;
        if prefix.is_empty() {
            return Err(invalid(
                "ordinary Backup entities lack the admitted upstream prefix",
            ));
        }
        let mut forest = PathBuf::from("backup-entities");
        for name in prefix {
            name_policy::validate_name(name)?;
            forest.push(name);
        }
        relative(&forest)?;
        let binding = LayoutBinding::parse(publication)?;
        let mut image = Self {
            files: BTreeMap::new(),
            directories: BTreeSet::new(),
            histories: BTreeSet::new(),
            directory_homes: BTreeMap::new(),
            file_homes: BTreeMap::new(),
            locator_paths: BTreeSet::new(),
            binding,
            manifest: Vec::new(),
            metadata_bytes: 0,
            bodies: 0,
            body_bytes: 0,
            directory_repositories: 0,
        };
        let state = admitted.state();
        for history in state.histories.keys() {
            image.histories.insert(history.clone());
        }
        image.capture_forest(state, prefix, stop)?;
        image.capture_files(state, slot, stop)?;
        image.capture_databases(admitted, stop)?;
        image.capture_empty_directories(state, &forest, stop)?;
        #[derive(Serialize)]
        struct Manifest<'b> {
            format: &'static str,
            publication: &'b str,
            slot: &'b str,
            prefix: &'b [String],
            directories: &'b BTreeSet<PathBuf>,
            files: BTreeMap<&'b PathBuf, Descriptor>,
            locators: &'b BTreeSet<PathBuf>,
        }
        image.manifest = canonical_json(&Manifest {
            format: "zxs-backup-ordinary-serving-recipe",
            publication,
            slot,
            prefix,
            directories: &image.directories,
            files: image
                .files
                .iter()
                .map(|(path, data)| (path, data.descriptor()))
                .collect(),
            locators: &image.locator_paths,
        })?;
        if image.manifest.len() > MAX_MANIFEST {
            return Err(budget());
        }
        Ok(image)
    }

    fn capture_forest(
        &mut self,
        state: &crate::State,
        prefix: &[String],
        stop: &AtomicBool,
    ) -> Result<()> {
        let goals = layout_goals::build(state, None)?;
        for history in goals.histories.values() {
            preparation_checkpoint(stop)?;
            let anchor = PathBuf::from("histories").join(&history.history_id);
            for goal in history.directories.values() {
                let to = target_directory(&goal.to, prefix)?;
                let directory = Directory {
                    history: history.history_id.clone(),
                    id: goal.id.clone(),
                    to,
                };
                let key = (directory.history.clone(), directory.id.clone());
                if let Some(previous) = self.directory_homes.get(&key) {
                    if previous.to != directory.to {
                        return Err(invalid(
                            "ordinary Backup directory has conflicting primary homes",
                        ));
                    }
                    continue;
                }
                self.add_directory(anchor.join(directory.to.as_path()))?;
                self.add_locator(
                    anchor
                        .join(".zxs-layout/directories")
                        .join(format!("{}.json", directory.id.as_str())),
                )?;
                self.directory_homes.insert(key, directory);
            }
            for goal in history.files.values() {
                let parent = match &goal.to {
                    FileAddress::Directory {
                        directory_id,
                        relative_path,
                    } => {
                        let directory = &self.directory_homes
                            [&(history.history_id.clone(), directory_id.clone())];
                        anchor
                            .join(directory.to.as_path())
                            .join(relative_path.as_path())
                    }
                    FileAddress::Stable { .. } => {
                        return Err(invalid(
                            "managed Backup file has no complete directory owner",
                        ));
                    }
                };
                self.add_home(FileHome {
                    history: history.history_id.clone(),
                    id: goal.file_id.clone(),
                    path: parent,
                    address: goal.to.clone(),
                })?;
            }
        }
        Ok(())
    }

    fn capture_files(
        &mut self,
        state: &'a crate::State,
        slot: &str,
        stop: &AtomicBool,
    ) -> Result<()> {
        for version in state.versions.values() {
            preparation_checkpoint(stop)?;
            let placement = &state.placements[&version.version_id];
            if !placement.route_slot_ids.iter().any(|route| route == slot) {
                return Err(invalid(
                    "ordinary Backup target omits a retained version route",
                ));
            }
            let key = (version.history_tree_id.clone(), version.file_id.clone());
            if !self.file_homes.contains_key(&key) {
                let path = state
                    .file_backup_paths
                    .get(&version.file_id)
                    .and_then(|paths| paths.get(slot))
                    .ok_or_else(|| invalid("ordinary Backup file lacks its exact target path"))?;
                name_policy::PortablePath::parse(path)?;
                let name = format!("namespace/{path}");
                let stable = PathBuf::from("histories")
                    .join(&version.history_tree_id)
                    .join("file-containers")
                    .join(&version.file_id);
                self.add_file(
                    stable.join("namespace").join(super::NAMESPACE_MARKER),
                    Data::Bytes(canonical_json(&version.file_id)?),
                )?;
                self.add_home(FileHome {
                    history: key.0.clone(),
                    id: key.1.clone(),
                    path: stable.join(&name),
                    address: FileAddress::Stable {
                        relative_path: StableRelative::parse(&name)?,
                    },
                })?;
            }
            if state.content_states[&version.version_id] == ContentState::Available {
                let home = &self.file_homes[&key];
                self.add_file(
                    home.path.join("versions").join(&version.storage_name),
                    Data::Body(version),
                )?;
                self.bodies += 1;
                self.body_bytes = self
                    .body_bytes
                    .checked_add(version.logical_size)
                    .ok_or_else(budget)?;
            }
        }
        Ok(())
    }

    fn capture_databases(&mut self, admitted: &AdmittedState, stop: &AtomicBool) -> Result<()> {
        for database in PreparedDatabases::capture(admitted, stop)?.into_databases() {
            preparation_checkpoint(stop)?;
            let (scope, bytes) = database.into_parts();
            let path = match scope {
                Scope::File { history, file } => self
                    .file_homes
                    .get(&(history, file))
                    .ok_or_else(|| invalid("ordinary Backup FILE database has no whole entity"))?
                    .path
                    .join(super::file_database::NAME),
                Scope::Directory {
                    history, directory, ..
                } => {
                    let home = self
                        .directory_homes
                        .get(&(history.clone(), DirectoryId::parse(&directory)?))
                        .ok_or_else(|| {
                            invalid("ordinary Backup directory database has no complete forest")
                        })?;
                    self.directory_repositories += 1;
                    PathBuf::from("histories")
                        .join(history)
                        .join(home.to.as_path())
                        .join(super::directory_database::NAME)
                }
            };
            self.add_file(path, Data::Bytes(bytes))?;
        }
        Ok(())
    }

    fn capture_empty_directories(
        &mut self,
        state: &crate::State,
        forest: &Path,
        stop: &AtomicBool,
    ) -> Result<()> {
        // Empty/uninitialized directory entries remain in their exact outer
        // repository and hop, separate from initialized repository identities.
        for repository in state.repositories.values() {
            let branch = &state.branches[&repository.main_branch_id];
            for entry in &state.commits[&branch.head_commit_id].entries {
                preparation_checkpoint(stop)?;
                if entry.kind == storage_contract::NamespaceEntryKind::Directory {
                    self.add_directory(
                        PathBuf::from("histories")
                            .join(&repository.history_tree_id)
                            .join(forest)
                            .join("namespace-directories")
                            .join(&repository.repository_id)
                            .join(entry.path()),
                    )?;
                }
            }
        }
        Ok(())
    }

    fn charge(&mut self, path: &Path, bytes: usize) -> Result<()> {
        relative(path)?;
        self.metadata_bytes = self
            .metadata_bytes
            .checked_add(path.as_os_str().len())
            .and_then(|n| n.checked_add(bytes))
            .ok_or_else(budget)?;
        if self.metadata_bytes > MAX_METADATA_BYTES {
            return Err(budget());
        }
        self.require_entry()
    }
    fn require_entry(&self) -> Result<()> {
        if self.files.len() + self.directories.len() + self.locator_paths.len() >= MAX_ENTRIES {
            return Err(budget());
        }
        Ok(())
    }
    fn add_directory(&mut self, path: PathBuf) -> Result<()> {
        for parent in path
            .ancestors()
            .take_while(|part| part.components().count() > 2)
        {
            if self.files.contains_key(parent) || self.locator_paths.contains(parent) {
                return Err(invalid("ordinary Backup directory collides with a file"));
            }
            if !self.directories.contains(parent) {
                self.charge(parent, 0)?;
                self.directories.insert(parent.to_owned());
            }
        }
        Ok(())
    }
    fn add_file(&mut self, path: PathBuf, data: Data<'a>) -> Result<()> {
        if self.files.contains_key(&path)
            || self.directories.contains(&path)
            || self.locator_paths.contains(&path)
        {
            return Err(invalid("ordinary Backup plan repeats a file"));
        }
        self.charge(
            &path,
            match &data {
                Data::Bytes(bytes) => bytes.len(),
                Data::Body(_) => 0,
            },
        )?;
        self.add_directory(
            path.parent()
                .expect("invariant: entity file has a parent")
                .to_owned(),
        )?;
        // Missing ancestors invalidate the initial available-entry check.
        self.require_entry()?;
        self.files.insert(path, data);
        Ok(())
    }
    fn add_locator(&mut self, path: PathBuf) -> Result<()> {
        self.charge(&path, 0)?;
        self.add_directory(path.parent().expect("invariant: locator parent").to_owned())?;
        self.require_entry()?;
        if !self.locator_paths.insert(path) {
            return Err(invalid("ordinary Backup locator is repeated"));
        }
        Ok(())
    }
    fn add_home(&mut self, home: FileHome) -> Result<()> {
        let key = (home.history.clone(), home.id.clone());
        if self.file_homes.contains_key(&key) {
            return Err(invalid(
                "ordinary Backup file has more than one physical home",
            ));
        }
        self.add_directory(home.path.join("versions"))?;
        self.add_file(
            home.path.join(super::OWNER_MARKER),
            Data::Bytes(canonical_json(&home.id)?),
        )?;
        self.add_locator(
            PathBuf::from("histories")
                .join(&home.history)
                .join(".zxs-layout/files")
                .join(format!("{}.json", home.id)),
        )?;
        self.file_homes.insert(key, home);
        Ok(())
    }
    pub(crate) fn hash(&self) -> String {
        blake3::hash(&self.manifest).to_hex().to_string()
    }
    pub(crate) fn whole_files(&self) -> usize {
        self.file_homes.len()
    }
    pub(crate) fn require_absent(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<()> {
        store::require_absent(root, self, stop)
    }
    pub(crate) fn require_capacity(
        &self,
        root: &DiskRoot,
        reserve: u64,
        stop: &AtomicBool,
    ) -> Result<()> {
        store::require_capacity(root, self, reserve, stop)
    }
    pub(crate) fn install(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<()> {
        store::install(root, self, stop)
    }
    pub(crate) fn verify(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<Vec<u8>> {
        store::verify(root, self, stop)
    }
}
