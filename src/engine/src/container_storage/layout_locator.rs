//! Finite, identity-bound addresses for decided directory-layout moves.
//!
//! These records locate an entity; they do not authorize a layout or a rename.
//! The caller retains the writer fence, proves the candidate and its complete
//! target set, and persists every move intent before changing any source name.
use crate::{Error, Result, disk::DiskRoot};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::linux::FileIdentity;
use std::{
    collections::{BTreeMap, BTreeSet},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};

pub(super) const FILE_STAGE: &str = ".zxs-layout-stage";
const METADATA: &str = ".zxs-layout";
const OWNER_MARKER: &str = ".zxs-container-owner.json";
const MAX_LOCATOR_BYTES: usize = 64 * 1024;
const MAX_PHYSICAL_PATH_BYTES: usize = 4096;
const MAX_PHYSICAL_COMPONENTS: usize = 64;

fn invalid(message: impl Into<String>) -> Error {
    Error::new(storage_contract::ErrorCode::ContainerLayoutInvalid, message)
}

/// Physical directory identity only. A registered repository root, ordinary
/// EntryId directory, and its deterministic retained-files directory share this
/// address space; none proves repository initialization or a file's owner.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct DirectoryId(String);
impl DirectoryId {
    pub(super) fn parse(value: &str) -> Result<Self> {
        let base = value.strip_prefix("retained_").unwrap_or(value);
        let prefix = if base.starts_with("repo_") {
            "repo"
        } else {
            "entry"
        };
        storage_contract::validate_id(base, prefix)?;
        Ok(Self(value.to_owned()))
    }
    /// There is one physical extra-directory anchor per verified logical ID.
    /// Applying this operation again retains that same anchor, never a nested ID.
    pub(super) fn retained(directory: &Self) -> Self {
        if directory.0.starts_with("retained_") {
            directory.clone()
        } else {
            Self(format!("retained_{}", directory.as_str()))
        }
    }
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct LayoutBinding(String);
impl LayoutBinding {
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }

    pub(super) fn parse(value: &str) -> Result<Self> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid(
                "layout binding must be one lowercase BLAKE3 digest",
            ));
        }
        Ok(Self(value.to_owned()))
    }
}

/// Internal physical address, including metadata and staging prefixes. Logical
/// portable-path budgets belong to the caller's pure namespace planner.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct HistoryRelative(String);
impl HistoryRelative {
    pub(super) fn parse(value: &str) -> Result<Self> {
        if value.len() > MAX_PHYSICAL_PATH_BYTES {
            return Err(Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "internal physical layout path exceeds 4096 bytes",
            ));
        }
        platform_io::fs_safety::RelativePath::new(value)?;
        for (index, name) in value.split('/').enumerate() {
            if index == MAX_PHYSICAL_COMPONENTS {
                return Err(Error::new(
                    storage_contract::ErrorCode::ResourceLimit,
                    "internal physical layout path exceeds 64 components",
                ));
            }
            name_policy::validate_name(name)?;
        }
        Ok(Self(value.to_owned()))
    }
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
    pub(super) fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct StableRelative(HistoryRelative);
impl StableRelative {
    pub(super) fn parse(value: &str) -> Result<Self> {
        Ok(Self(HistoryRelative::parse(value)?))
    }
    pub(super) fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct EntityInode(FileIdentity);
impl EntityInode {
    pub(super) fn capture(root: &DiskRoot, path: &Path) -> Result<Self> {
        Ok(Self(root.directory_identity(path)?))
    }
    pub(super) fn identity(self) -> FileIdentity {
        self.0
    }
    pub(super) fn matches(self, root: &DiskRoot, path: &Path) -> Result<bool> {
        Ok(self == Self::capture(root, path)?)
    }
}

/// An admitted history below the supplied held root, which may itself be the
/// history/package root. No operation opens or derives a wider disk root.
pub(super) struct HistoryAnchor<'a> {
    root: &'a DiskRoot,
    path: PathBuf,
    history_id: String,
    inode: EntityInode,
}
impl<'a> HistoryAnchor<'a> {
    pub(super) fn new(root: &'a DiskRoot, history_path: &Path, history_id: &str) -> Result<Self> {
        storage_contract::validate_id(history_id, "history")?;
        let relative = history_path
            .strip_prefix(root.path())
            .map_err(|_| invalid("layout history is outside the held root"))?;
        let canonical = if relative.as_os_str().is_empty() {
            root.path().to_path_buf()
        } else {
            root.path().join(relative)
        };
        if relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
            || canonical.as_os_str() != history_path.as_os_str()
        {
            return Err(invalid("layout history path is not canonical"));
        }
        let inode = EntityInode::capture(root, history_path)?;
        Ok(Self {
            root,
            path: history_path.to_path_buf(),
            history_id: history_id.to_owned(),
            inode,
        })
    }
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
    pub(super) fn root(&self) -> &DiskRoot {
        self.root
    }
    pub(super) fn history_id(&self) -> &str {
        &self.history_id
    }
    pub(super) fn check(&self, history_id: &str) -> Result<()> {
        if history_id != self.history_id {
            return Err(invalid("layout locator belongs to another history"));
        }
        if EntityInode::capture(self.root, &self.path)? != self.inode {
            return Err(invalid("layout history directory identity changed"));
        }
        Ok(())
    }
    fn absolute(&self, relative: &HistoryRelative) -> PathBuf {
        self.path.join(relative.as_path())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum FileAddress {
    Stable {
        relative_path: StableRelative,
    },
    Directory {
        directory_id: DirectoryId,
        /// Exactly one container basename within the recorded physical parent.
        /// FileIntent construction and persistent admission enforce this bound.
        relative_path: HistoryRelative,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FileIntent {
    history_id: String,
    file_id: String,
    binding: LayoutBinding,
    inode: EntityInode,
    from: FileAddress,
    to: FileAddress,
}
impl FileIntent {
    pub(super) fn new(
        history_id: &str,
        file_id: &str,
        binding: LayoutBinding,
        inode: EntityInode,
        from: FileAddress,
        to: FileAddress,
    ) -> Result<Self> {
        storage_contract::validate_id(history_id, "history")?;
        storage_contract::validate_id(file_id, "file")?;
        for address in [&from, &to] {
            if let FileAddress::Directory { relative_path, .. } = address
                && relative_path.as_path().components().count() != 1
            {
                return Err(invalid(
                    "file directory addresses require one container basename",
                ));
            }
        }
        Ok(Self {
            history_id: history_id.to_owned(),
            file_id: file_id.to_owned(),
            binding,
            inode,
            from,
            to,
        })
    }
    pub(super) fn file_id(&self) -> &str {
        &self.file_id
    }
    pub(super) fn history_id(&self) -> &str {
        &self.history_id
    }
    pub(super) fn binding(&self) -> &LayoutBinding {
        &self.binding
    }
    pub(super) fn inode(&self) -> EntityInode {
        self.inode
    }
    pub(super) fn from(&self) -> &FileAddress {
        &self.from
    }
    pub(super) fn to(&self) -> &FileAddress {
        &self.to
    }
    pub(super) fn canonical_bytes(&self) -> Result<Vec<u8>> {
        Ok(crate::canonical_json(&RawFileIntent::from(self))?)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DirectoryIntent {
    history_id: String,
    directory_id: DirectoryId,
    binding: LayoutBinding,
    inode: EntityInode,
    from: HistoryRelative,
    staging: HistoryRelative,
    to: HistoryRelative,
}
impl DirectoryIntent {
    pub(super) fn new(
        history_id: &str,
        directory_id: DirectoryId,
        binding: LayoutBinding,
        inode: EntityInode,
        from: HistoryRelative,
        to: HistoryRelative,
    ) -> Result<Self> {
        storage_contract::validate_id(history_id, "history")?;
        let staging = directory_staging(&directory_id)?;
        Ok(Self {
            history_id: history_id.to_owned(),
            directory_id,
            binding,
            inode,
            from,
            staging,
            to,
        })
    }
    pub(super) fn directory_id(&self) -> &DirectoryId {
        &self.directory_id
    }
    pub(super) fn history_id(&self) -> &str {
        &self.history_id
    }
    pub(super) fn binding(&self) -> &LayoutBinding {
        &self.binding
    }
    pub(super) fn inode(&self) -> EntityInode {
        self.inode
    }
    pub(super) fn from(&self) -> &HistoryRelative {
        &self.from
    }
    pub(super) fn staging(&self) -> &HistoryRelative {
        &self.staging
    }
    pub(super) fn to(&self) -> &HistoryRelative {
        &self.to
    }
    pub(super) fn canonical_bytes(&self) -> Result<Vec<u8>> {
        Ok(crate::canonical_json(&RawDirectoryIntent::from(self))?)
    }
}

pub(super) fn file_staging(file_id: &str) -> Result<HistoryRelative> {
    storage_contract::validate_id(file_id, "file")?;
    HistoryRelative::parse(&format!("file-containers/{file_id}/{FILE_STAGE}"))
}
pub(super) fn directory_staging(directory: &DirectoryId) -> Result<HistoryRelative> {
    HistoryRelative::parse(&format!("{METADATA}/staging/{}", directory.as_str()))
}
pub(super) fn file_locator_path(anchor: &HistoryAnchor<'_>, file_id: &str) -> Result<PathBuf> {
    storage_contract::validate_id(file_id, "file")?;
    Ok(anchor
        .path
        .join(METADATA)
        .join("files")
        .join(format!("{file_id}.json")))
}
pub(super) fn directory_locator_path(
    anchor: &HistoryAnchor<'_>,
    directory: &DirectoryId,
) -> PathBuf {
    anchor
        .path
        .join(METADATA)
        .join("directories")
        .join(format!("{}.json", directory.as_str()))
}

// Disk representations stay private. Every read reconstructs the constrained
// domain values, including fixed staging names and canonical integer strings.
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct RawInode {
    #[serialization(with = "storage_contract::u64_string")]
    device: u64,
    #[serialization(with = "storage_contract::u64_string")]
    inode: u64,
}
impl From<EntityInode> for RawInode {
    fn from(value: EntityInode) -> Self {
        Self {
            device: value.0.device,
            inode: value.0.inode,
        }
    }
}
impl From<RawInode> for EntityInode {
    fn from(value: RawInode) -> Self {
        Self(FileIdentity {
            device: value.device,
            inode: value.inode,
        })
    }
}
#[derive(Serialize, Deserialize)]
#[serialization(tag = "kind", deny_unknown_fields)]
enum RawAddress {
    #[serialization(rename = "stable")]
    Stable { relative_path: String },
    #[serialization(rename = "directory")]
    Directory {
        directory_id: String,
        relative_path: String,
    },
}
impl From<&FileAddress> for RawAddress {
    fn from(value: &FileAddress) -> Self {
        match value {
            FileAddress::Stable { relative_path } => Self::Stable {
                relative_path: relative_path.as_str().to_owned(),
            },
            FileAddress::Directory {
                directory_id,
                relative_path,
            } => Self::Directory {
                directory_id: directory_id.as_str().to_owned(),
                relative_path: relative_path.as_str().to_owned(),
            },
        }
    }
}
impl RawAddress {
    fn admit(self) -> Result<FileAddress> {
        Ok(match self {
            Self::Stable { relative_path } => FileAddress::Stable {
                relative_path: StableRelative::parse(&relative_path)?,
            },
            Self::Directory {
                directory_id,
                relative_path,
            } => FileAddress::Directory {
                directory_id: DirectoryId::parse(&directory_id)?,
                relative_path: HistoryRelative::parse(&relative_path)?,
            },
        })
    }
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct RawFileIntent {
    history_id: String,
    file_id: String,
    layout_binding: String,
    source_inode: RawInode,
    from: RawAddress,
    staging: String,
    to: RawAddress,
}
impl From<&FileIntent> for RawFileIntent {
    fn from(value: &FileIntent) -> Self {
        Self {
            history_id: value.history_id.clone(),
            file_id: value.file_id.clone(),
            layout_binding: value.binding.0.clone(),
            source_inode: value.inode.into(),
            from: (&value.from).into(),
            staging: FILE_STAGE.to_owned(),
            to: (&value.to).into(),
        }
    }
}
impl RawFileIntent {
    fn admit(self) -> Result<FileIntent> {
        if self.staging != FILE_STAGE {
            return Err(invalid(
                "file locator has a noncanonical stable staging address",
            ));
        }
        FileIntent::new(
            &self.history_id,
            &self.file_id,
            LayoutBinding::parse(&self.layout_binding)?,
            self.source_inode.into(),
            self.from.admit()?,
            self.to.admit()?,
        )
    }
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct RawDirectoryIntent {
    history_id: String,
    directory_id: String,
    layout_binding: String,
    source_inode: RawInode,
    from: String,
    staging: String,
    to: String,
}
impl From<&DirectoryIntent> for RawDirectoryIntent {
    fn from(value: &DirectoryIntent) -> Self {
        Self {
            history_id: value.history_id.clone(),
            directory_id: value.directory_id.0.clone(),
            layout_binding: value.binding.0.clone(),
            source_inode: value.inode.into(),
            from: value.from.0.clone(),
            staging: value.staging.0.clone(),
            to: value.to.0.clone(),
        }
    }
}
impl RawDirectoryIntent {
    fn admit(self) -> Result<DirectoryIntent> {
        let intent = DirectoryIntent::new(
            &self.history_id,
            DirectoryId::parse(&self.directory_id)?,
            LayoutBinding::parse(&self.layout_binding)?,
            self.source_inode.into(),
            HistoryRelative::parse(&self.from)?,
            HistoryRelative::parse(&self.to)?,
        )?;
        if intent.staging.as_str() != self.staging {
            return Err(invalid(
                "directory locator has a noncanonical stable staging address",
            ));
        }
        Ok(intent)
    }
}

fn read_record<T: data_serialization::de::DeserializeOwned>(
    anchor: &HistoryAnchor<'_>,
    path: &Path,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<T> {
    observe(0)?;
    if let Some(metadata) = anchor.root.optional_metadata(path)?
        && (!metadata.is_file() || metadata.file_type().is_symlink() || metadata.nlink() != 1)
    {
        return Err(invalid(
            "layout metadata must be one ordinary single-link file",
        ));
    }
    let bytes = anchor
        .root
        .read_bytes_observed(path, MAX_LOCATOR_BYTES, observe)?;
    storage_contract::decode_strict(&bytes).map_err(Into::into)
}

/// Missing records are errors. Creation is a separate, explicit store operation;
/// callers cannot turn a lost pointer into permission to create another entity.
pub(super) fn load_file(
    anchor: &HistoryAnchor<'_>,
    file_id: &str,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<FileIntent> {
    anchor.check(anchor.history_id())?;
    let intent =
        read_record::<RawFileIntent>(anchor, &file_locator_path(anchor, file_id)?, observe)?
            .admit()?;
    anchor.check(&intent.history_id)?;
    if intent.file_id != file_id {
        return Err(invalid(
            "file locator key differs from its recorded file identity",
        ));
    }
    Ok(intent)
}
pub(super) fn load_directory(
    anchor: &HistoryAnchor<'_>,
    directory_id: &DirectoryId,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<DirectoryIntent> {
    anchor.check(anchor.history_id())?;
    let intent = read_record::<RawDirectoryIntent>(
        anchor,
        &directory_locator_path(anchor, directory_id),
        observe,
    )?
    .admit()?;
    anchor.check(&intent.history_id)?;
    if intent.directory_id != *directory_id {
        return Err(invalid(
            "directory locator key differs from its recorded directory identity",
        ));
    }
    Ok(intent)
}

/// Only absence of a component means absence. Every existing ancestor remains
/// a private directory on the held mount; links and non-directories are errors.
fn probe_directory(
    anchor: &HistoryAnchor<'_>,
    relative: &HistoryRelative,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<Option<EntityInode>> {
    probe_directory_with_label(anchor, relative, observe, "layout candidate")
}

pub(super) fn probe_directory_with_label(
    anchor: &HistoryAnchor<'_>,
    relative: &HistoryRelative,
    observe: &mut impl FnMut(usize) -> Result<()>,
    label: &'static str,
) -> Result<Option<EntityInode>> {
    let mut path = anchor.path.clone();
    let mut identity = None;
    for component in relative.as_path().components() {
        observe(0)?;
        path.push(component);
        let Some(metadata) = anchor.root.optional_metadata(&path)? else {
            return Ok(None);
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(invalid(format!(
                "{label} is not a directory: {path:?}"
            )));
        }
        identity = Some(EntityInode::capture(anchor.root, &path)?);
    }
    Ok(identity)
}
fn unique(selected: &mut Option<PathBuf>, path: PathBuf) -> Result<()> {
    if selected.replace(path).is_some() {
        return Err(Error::new(
            storage_contract::ErrorCode::ContainerAmbiguous,
            "layout locator resolves to more than one entity",
        ));
    }
    Ok(())
}
/// Resolve only this directory's recorded candidates. If an unchanged child
/// follows a staged ancestor, recovery must place that ancestor first or record
/// the child's actual intermediate prefix before asking to locate the child.
pub(super) fn locate_directory(
    anchor: &HistoryAnchor<'_>,
    intent: &DirectoryIntent,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<PathBuf> {
    anchor.check(&intent.history_id)?;
    let mut selected = None;
    let candidates = [&intent.from, &intent.staging, &intent.to]
        .into_iter()
        .collect::<BTreeSet<_>>();
    for relative in candidates {
        if probe_directory(anchor, relative, observe)? == Some(intent.inode) {
            unique(&mut selected, anchor.absolute(relative))?;
        }
    }
    selected.ok_or_else(|| {
        invalid(format!(
            "directory locator has no entity with its recorded inode: {}",
            intent.directory_id.as_str(),
        ))
    })
}

fn current_directory_locations(
    anchor: &HistoryAnchor<'_>,
    intents: &BTreeMap<DirectoryId, DirectoryIntent>,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<BTreeMap<DirectoryId, PathBuf>> {
    let mut locations = BTreeMap::new();
    let mut occupied = BTreeSet::new();
    for (directory_id, intent) in intents {
        if directory_id != intent.directory_id() {
            return Err(invalid(
                "directory intent map key differs from its stable identity",
            ));
        }
        let recorded = load_directory(anchor, directory_id, observe)?;
        if recorded != *intent {
            return Err(invalid(
                "directory intent map differs from its durable record",
            ));
        }
        let current = locate_directory(anchor, intent, observe)?;
        if !occupied.insert(current.clone()) {
            return Err(invalid(
                "one physical directory has multiple recorded identities",
            ));
        }
        locations.insert(directory_id.clone(), current);
    }
    Ok(locations)
}

fn translated_directory_intents(
    anchor: &HistoryAnchor<'_>,
    intents: &BTreeMap<DirectoryId, DirectoryIntent>,
    locations: &BTreeMap<DirectoryId, PathBuf>,
    moving: &Path,
    destination: &HistoryRelative,
) -> Result<Vec<(DirectoryIntent, DirectoryIntent)>> {
    let mut updates = Vec::new();
    for (directory_id, current) in locations {
        let Ok(suffix) = current.strip_prefix(moving) else {
            continue;
        };
        let from = current
            .strip_prefix(anchor.path())
            .expect("invariant: located directories remain below the admitted history");
        let from = HistoryRelative::parse(
            from.to_str()
                .expect("invariant: located directory addresses are UTF-8"),
        )?;
        let to = if suffix.as_os_str().is_empty() {
            destination.clone()
        } else {
            let path = destination.as_path().join(suffix);
            HistoryRelative::parse(
                path.to_str()
                    .expect("invariant: translated directory addresses are UTF-8"),
            )?
        };
        let previous = &intents[directory_id];
        let next = DirectoryIntent::new(
            anchor.history_id(),
            directory_id.clone(),
            previous.binding().clone(),
            previous.inode(),
            from,
            to,
        )?;
        updates.push((previous.clone(), next));
    }
    Ok(updates)
}

/// Relocate one physical directory with the known descendants that follow its
/// prefix. The caller proves that this map contains the complete moving closure
/// and retains State/slot authorization, the writer fence and target preflight.
/// This function verifies only the supplied records; it does not discover a
/// complete namespace or authorize replacing any destination.
///
/// All translated paths are admitted before the first store. Each successful
/// store updates the map; any error stops the operation without rollback. After
/// an uncertain store result, reload durable records before retrying the map.
pub(super) fn relocate_directory(
    anchor: &HistoryAnchor<'_>,
    intents: &mut BTreeMap<DirectoryId, DirectoryIntent>,
    moving_id: &DirectoryId,
    destination: HistoryRelative,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<()> {
    anchor.check(anchor.history_id())?;
    if !intents.contains_key(moving_id) {
        return Err(invalid(
            "moving directory is absent from the supplied intent map",
        ));
    }
    let locations = current_directory_locations(anchor, intents, observe)?;
    let source = &locations[moving_id];
    let target = anchor.absolute(&destination);
    if target != *source && target.starts_with(source) {
        return Err(invalid(
            "a physical directory cannot be relocated into its own descendant",
        ));
    }
    let updates = translated_directory_intents(anchor, intents, &locations, source, &destination)?;
    let mut retry_addresses = Vec::new();
    for (previous, next) in &updates {
        retry_addresses.extend([
            previous.from().clone(),
            previous.staging().clone(),
            previous.to().clone(),
            next.from().clone(),
            next.to().clone(),
        ]);
    }
    if source == &target {
        return sync_retry_tails(anchor, &retry_addresses, observe);
    }
    for (previous, next) in &updates {
        observe(0)?;
        store_directory(anchor, next, Some(previous))?;
        intents.insert(next.directory_id().clone(), next.clone());
    }
    observe(0)?;
    anchor.check(anchor.history_id())?;
    for (_, next) in &updates {
        if !next
            .inode()
            .matches(anchor.root(), &anchor.absolute(next.from()))?
        {
            return Err(invalid(
                "directory source inode changed after its move intent was stored",
            ));
        }
    }
    anchor.root().rename_noreplace_synced(source, &target)?;
    sync_retry_tails(anchor, &retry_addresses, observe)
}

fn file_address(
    anchor: &HistoryAnchor<'_>,
    intent: &FileIntent,
    address: &FileAddress,
    directories: &mut BTreeMap<DirectoryId, PathBuf>,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<HistoryRelative> {
    match address {
        FileAddress::Stable { relative_path } => HistoryRelative::parse(&format!(
            "file-containers/{}/{}",
            intent.file_id,
            relative_path.as_str(),
        )),
        FileAddress::Directory {
            directory_id,
            relative_path,
        } => {
            if !directories.contains_key(directory_id) {
                let directory = load_directory(anchor, directory_id, observe)?;
                let path = locate_directory(anchor, &directory, observe)?;
                directories.insert(directory_id.clone(), path);
            }
            let path = directories[directory_id].join(relative_path.as_path());
            let relative = path
                .strip_prefix(&anchor.path)
                .expect("invariant: directory addresses remain below their history");
            HistoryRelative::parse(
                relative
                    .to_str()
                    .expect("invariant: admitted layout paths are UTF-8"),
            )
        }
    }
}

/// Resolve exactly from, fixed stable staging and to. Foreign occupants do not
/// become this file. Unowned structural candidates are left for the executor's
/// target inventory; this function never deletes, creates or adopts them.
pub(super) fn locate_file(
    anchor: &HistoryAnchor<'_>,
    intent: &FileIntent,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<PathBuf> {
    anchor.check(&intent.history_id)?;
    let mut directories = BTreeMap::new();
    let mut candidates = BTreeSet::from([file_staging(&intent.file_id)?]);
    for address in [&intent.from, &intent.to] {
        candidates.insert(file_address(
            anchor,
            intent,
            address,
            &mut directories,
            observe,
        )?);
    }
    let mut selected = None;
    for relative in candidates {
        let Some(inode) = probe_directory(anchor, &relative, observe)? else {
            continue;
        };
        let path = anchor.absolute(&relative);
        let marker = path.join(OWNER_MARKER);
        observe(0)?;
        let owner = match anchor.root.optional_metadata(&marker)? {
            None => None,
            Some(metadata) => {
                if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.nlink() != 1
                {
                    return Err(invalid(
                        "file owner marker must be one ordinary single-link file",
                    ));
                }
                let owner: String = read_record(anchor, &marker, observe)?;
                storage_contract::validate_id(&owner, "file")?;
                Some(owner)
            }
        };
        if owner.as_deref() == Some(intent.file_id.as_str()) {
            if inode != intent.inode {
                return Err(invalid(
                    "file identity appears with a different container inode",
                ));
            }
            super::reject_unsupported_container(anchor.root, &path)?;
            let versions = HistoryRelative::parse(&format!("{}/versions", relative.as_str()))?;
            if probe_directory(anchor, &versions, observe)?.is_none() {
                return Err(invalid("located file container has no version directory"));
            }
            unique(&mut selected, path)?;
        } else if inode == intent.inode {
            return Err(invalid(
                "recorded file container inode has a missing or different owner marker",
            ));
        }
    }
    selected.ok_or_else(|| {
        invalid(format!(
            "file locator has no entity with its recorded owner and inode: {}/{}",
            intent.history_id, intent.file_id,
        ))
    })
}

/// Complete directory barriers even when a previous rename/write became visible
/// before its synchronization failed. Missing parents are not created. The
/// executor supplies every source and destination used by the batch, including
/// pre-move directory roots; resolving only their current roots is insufficient.
pub(super) fn sync_retry_tails(
    anchor: &HistoryAnchor<'_>,
    addresses: &[HistoryRelative],
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<()> {
    anchor.check(anchor.history_id())?;
    let mut parents = BTreeSet::new();
    for address in addresses {
        let mut parent = address.as_path().parent();
        while let Some(path) = parent {
            if path.as_os_str().is_empty() {
                break;
            }
            parents.insert(HistoryRelative::parse(
                path.to_str()
                    .expect("invariant: admitted layout parents are UTF-8"),
            )?);
            parent = path.parent();
        }
    }
    for parent in parents.iter().rev() {
        if probe_directory(anchor, parent, observe)?.is_some() {
            anchor.root.sync_dir(&anchor.absolute(parent))?;
        }
    }
    observe(0)?;
    anchor.root.sync_dir(&anchor.path)
}

fn record_present(anchor: &HistoryAnchor<'_>, path: &Path) -> Result<bool> {
    let relative = path
        .strip_prefix(&anchor.path)
        .expect("invariant: locator metadata stays below its history");
    let parent = relative
        .parent()
        .expect("invariant: locator metadata has a parent");
    if !parent.as_os_str().is_empty()
        && probe_directory(
            anchor,
            &HistoryRelative::parse(
                parent
                    .to_str()
                    .expect("invariant: locator metadata is UTF-8"),
            )?,
            &mut |_| Ok(()),
        )?
        .is_none()
    {
        return Ok(false);
    }
    Ok(anchor.root.optional_metadata(path)?.is_some())
}
fn sync_record(anchor: &HistoryAnchor<'_>, path: &Path) -> Result<()> {
    let relative = path
        .strip_prefix(&anchor.path)
        .expect("invariant: locator metadata stays below its history");
    sync_retry_tails(
        anchor,
        &[HistoryRelative::parse(
            relative
                .to_str()
                .expect("invariant: locator metadata is UTF-8"),
        )?],
        &mut |_| Ok(()),
    )
}

/// `previous == None` is explicit first registration, not a missing-pointer
/// fallback. An exact existing new record is an idempotent retry. Replacement
/// requires the exact previously admitted record under the retained writer lock.
pub(super) fn store_file(
    anchor: &HistoryAnchor<'_>,
    intent: &FileIntent,
    previous: Option<&FileIntent>,
) -> Result<()> {
    anchor.check(&intent.history_id)?;
    let path = file_locator_path(anchor, &intent.file_id)?;
    let replace = if record_present(anchor, &path)? {
        let current = load_file(anchor, &intent.file_id, &mut |_| Ok(()))?;
        if current == *intent {
            return sync_record(anchor, &path);
        }
        if previous != Some(&current) {
            return Err(invalid(
                "file locator replacement does not match its admitted previous intent",
            ));
        }
        if current.inode != intent.inode {
            return Err(invalid(
                "file locator replacement changes its stable container inode",
            ));
        }
        true
    } else {
        if previous.is_some() {
            return Err(invalid(
                "previous file locator is missing; refusing a second entity registration",
            ));
        }
        false
    };
    anchor
        .root
        .write_json(&path, &RawFileIntent::from(intent), replace)?;
    sync_record(anchor, &path)
}
pub(super) fn store_directory(
    anchor: &HistoryAnchor<'_>,
    intent: &DirectoryIntent,
    previous: Option<&DirectoryIntent>,
) -> Result<()> {
    anchor.check(&intent.history_id)?;
    let path = directory_locator_path(anchor, &intent.directory_id);
    let replace = if record_present(anchor, &path)? {
        let current = load_directory(anchor, &intent.directory_id, &mut |_| Ok(()))?;
        if current == *intent {
            return sync_record(anchor, &path);
        }
        if previous != Some(&current) {
            return Err(invalid(
                "directory locator replacement does not match its admitted previous intent",
            ));
        }
        if current.inode != intent.inode {
            return Err(invalid(
                "directory locator replacement changes its stable root inode",
            ));
        }
        true
    } else {
        if previous.is_some() {
            return Err(invalid("previous directory locator is missing"));
        }
        false
    };
    anchor
        .root
        .write_json(&path, &RawDirectoryIntent::from(intent), replace)?;
    sync_record(anchor, &path)
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/container_storage/layout_locator_tests.rs"]
mod tests;
