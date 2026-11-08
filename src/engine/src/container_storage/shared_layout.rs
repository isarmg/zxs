//! Physical projection of a frozen, admitted State through held disk roots.
//!
//! Locators are derived caches, never authorization. Every invocation rebuilds
//! the complete selected namespace from State, then verifies its real entities.
//! Private empty preparations are exclusive; abandoned, unreferenced directories
//! are removed only by their original inode-bound owner. Interrupted persistent
//! preparations are retained, never adopted on retry. Names change only after
//! Finalize, or explicitly repairs an already published State under its writer.
use super::{
    layout_batch::{self, Batch, DirectoryMove, FileMove, FutureFile},
    layout_goals::{self, FileGoal, HistoryGoals},
    layout_locator::{
        self, DirectoryId, DirectoryIntent, EntityInode, FileAddress, FileIntent, HistoryAnchor,
        HistoryRelative, LayoutBinding, StableRelative,
    },
};
use crate::{disk::*, *};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

const MODE: &str = "directory-main-workspaces";
const MAX_ENTITIES: usize = 1_000_000;
type HistoryKey = (String, String);
type FileKey = (String, String, String);

/// Structural placement only. Missing replicas are reported, never synthesized
/// as empty files and never counted as payload verification.
#[derive(Debug, Default)]
pub struct DirectoryLayoutReceipt {
    pub state_sequence: u64,
    pub directories: usize,
    pub containers: usize,
    /// (slot ID, history ID, file ID).
    pub unavailable_containers: Vec<(String, String, String)>,
    /// Admitted slot/history pairs whose held resource was unavailable.
    pub unavailable_histories: Vec<(String, String)>,
}

#[derive(Clone)]
pub(super) struct PhysicalProjection {
    pub(super) name: String,
    pub(super) location: String,
    pub(super) root_directory_id: String,
    pub(super) owner_directory_id: String,
}

#[derive(Default)]
pub(super) struct PreparedLayouts {
    histories: Vec<PreparedHistory>,
    projections: BTreeMap<FileKey, PhysicalProjection>,
    sequence: u64,
    missing: Vec<(String, String, String)>,
    missing_histories: Vec<(String, String)>,
}
struct PreparedHistory {
    disk: Arc<DiskHandle>,
    history: PathBuf,
    history_id: String,
    slot_id: String,
    batch: Batch,
    databases: Vec<(DirectoryId, crate::database_install::PreparedInstall)>,
    deferred: Vec<DeferredFile>,
    missing: Vec<String>,
    private: PrivatePreparation,
}

/// Own only directories this invocation created exclusively. Before locator
/// publication they are disposable empty preparations, not recovered entities.
/// Once a locator write may have begun, even an empty directory must be retained.
struct PrivatePreparation {
    disk: Arc<DiskHandle>,
    directories: Vec<(PathBuf, platform_io::fs_safety::linux::FileIdentity)>,
    referenced: bool,
}
impl PrivatePreparation {
    fn new(disk: &Arc<DiskHandle>) -> Self {
        Self {
            disk: Arc::clone(disk),
            directories: Vec::new(),
            referenced: false,
        }
    }

    fn create(&mut self, path: &Path) -> Result<platform_io::fs_safety::linux::FileIdentity> {
        let identity = self.disk.root.create_private_directory_synced(path)?;
        self.directories.push((path.to_owned(), identity));
        Ok(identity)
    }

    fn remove_owned_empty(
        &self,
        path: &Path,
        identity: platform_io::fs_safety::linux::FileIdentity,
    ) -> Result<()> {
        // REMOVEDIR refuses a nonempty directory. Never recurse or delete an
        // unknown entry; the held writer fence remains owned through cleanup.
        self.disk
            .root
            .remove_owned_empty_directory_synced(path, identity)
    }

    fn cleanup_completed_parent(&self) {
        if let Some((path, identity)) = self.directories.first()
            && let Err(error) = self.remove_owned_empty(path, *identity)
        {
            tracing::warn!(path = %path.display(), error = %error,
                "could not remove completed layout preparation parent");
        }
    }
}
impl Drop for PrivatePreparation {
    fn drop(&mut self) {
        if self.referenced {
            return;
        }
        for (path, identity) in self.directories.iter().rev() {
            if let Err(error) = self.remove_owned_empty(path, *identity) {
                tracing::warn!(path = %path.display(), error = %error,
                    "could not remove abandoned private layout preparation");
            }
        }
    }
}
struct DeferredFile {
    file_id: String,
    source: PathBuf,
    inode: EntityInode,
    from: FileAddress,
    goal: FileGoal,
}

fn invalid(detail: impl Into<String>) -> Error {
    Error::new(ErrorCode::ContainerLayoutInvalid, detail)
}
fn relative(anchor: &HistoryAnchor<'_>, path: &Path) -> Result<HistoryRelative> {
    let path = path
        .strip_prefix(anchor.path())
        .map_err(|_| invalid("physical layout escaped its held history"))?;
    HistoryRelative::parse(
        path.to_str()
            .ok_or_else(|| invalid("layout path is not UTF-8"))?,
    )
}
fn present(root: &DiskRoot, history: &Path, path: &Path) -> Result<bool> {
    super::path_exists_at(root, history, path)
}
fn record_names(root: &DiskRoot, history: &Path, kind: &str) -> Result<Vec<String>> {
    let directory = history.join(".zxs-layout").join(kind);
    if !present(root, history, &directory)? {
        return Ok(Vec::new());
    }
    root.directory_identity(&directory)?;
    let mut result = Vec::new();
    for entry in root.entries(&directory)? {
        if result.len() == MAX_ENTITIES {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "layout cache inventory exceeds its entity budget",
            ));
        }
        let entry = entry?;
        let name = entry
            .as_os_str()
            .to_str()
            .ok_or_else(|| invalid("layout record name is not UTF-8"))?;
        let id = name
            .strip_suffix(".json")
            .ok_or_else(|| invalid("unknown entry in layout record directory"))?;
        result.push(id.to_owned());
    }
    Ok(result)
}

/// Only previously activated histories enter automatic maintenance. Logical Git
/// initialization stays available before resolving duplicate main occurrences.
/// The first physical activation is an explicit Engine operation.
pub(super) fn active_outer_ids(
    config: &ValidatedConfig,
    resources: &DiskResources,
    states: &[&State],
) -> Result<BTreeSet<String>> {
    let mut histories = BTreeMap::new();
    for state in states {
        for history in state.histories.values() {
            if let Some(old) = histories.insert(
                history.history_tree_id.clone(),
                history.repository_id.clone(),
            ) && old != history.repository_id
            {
                return Err(invalid(
                    "history ownership changed across layout preparation",
                ));
            }
        }
    }
    let mut selected = BTreeSet::new();
    for disk in config.io_disks() {
        let Some(held) = resources.available(disk) else {
            continue;
        };
        for (history_id, outer) in &histories {
            let history = disk.root.join("histories").join(history_id);
            if !super::path_directory_exists_at(&held.root, held.root.path(), &history)? {
                continue;
            }
            let records = !record_names(&held.root, &history, "directories")?.is_empty()
                || !record_names(&held.root, &history, "files")?.is_empty();
            let namespace = history.join("directory-repositories");
            let populated = if present(&held.root, &history, &namespace)? {
                held.root.directory_identity(&namespace)?;
                held.root.entries(&namespace)?.next().transpose()?.is_some()
            } else {
                false
            };
            if records || populated {
                selected.insert(outer.clone());
            }
        }
    }
    Ok(selected)
}

fn authorized_directory_ids(
    states: &[&State],
    selected: &BTreeSet<String>,
) -> BTreeSet<DirectoryId> {
    let mut result = BTreeSet::new();
    for state in states {
        for repository in state.repositories.values() {
            if selected.contains(&repository.repository_id) {
                result.insert(
                    DirectoryId::parse(&repository.repository_id)
                        .expect("invariant: admitted repository ID"),
                );
            }
        }
        for branch in state.directory_branches.values() {
            if selected.contains(&branch.repository_id) {
                result.insert(
                    DirectoryId::parse(&branch.directory_id)
                        .expect("invariant: admitted directory branch ID"),
                );
                for entry in &branch.entries {
                    if entry.kind == NamespaceEntryKind::Directory {
                        result.insert(
                            DirectoryId::parse(&entry.entry_id)
                                .expect("invariant: admitted directory entry ID"),
                        );
                    }
                }
            }
        }
        for commit in state.commits.values() {
            if state
                .histories
                .get(&commit.history_tree_id)
                .is_some_and(|history| selected.contains(&history.repository_id))
            {
                for entry in &commit.entries {
                    if entry.kind == NamespaceEntryKind::Directory {
                        result.insert(
                            DirectoryId::parse(&entry.entry_id)
                                .expect("invariant: admitted historical directory entry ID"),
                        );
                    }
                }
            }
        }
        for tree in state.directory_trees.values() {
            if state
                .directory_branches
                .get(&tree.directory_branch_id)
                .is_some_and(|branch| selected.contains(&branch.repository_id))
            {
                for entry in &tree.entries {
                    if entry.kind == NamespaceEntryKind::Directory {
                        result.insert(
                            DirectoryId::parse(&entry.entry_id)
                                .expect("invariant: admitted directory tree entry ID"),
                        );
                    }
                }
            }
        }
    }
    let extras = result.iter().map(DirectoryId::retained).collect::<Vec<_>>();
    result.extend(extras);
    result
}

fn stable_source(anchor: &HistoryAnchor<'_>, file: &str, source: &Path) -> Result<FileAddress> {
    let stable = anchor.path().join("file-containers").join(file);
    let suffix = source
        .strip_prefix(&stable)
        .map_err(|_| invalid("unregistered file container is outside its stable source root"))?;
    Ok(FileAddress::Stable {
        relative_path: StableRelative::parse(
            suffix
                .to_str()
                .ok_or_else(|| invalid("stable source is not UTF-8"))?,
        )?,
    })
}
fn current_address(
    anchor: &HistoryAnchor<'_>,
    file: &str,
    source: &Path,
    source_ids: &BTreeMap<PathBuf, DirectoryId>,
) -> Result<FileAddress> {
    if let Some(id) = source.parent().and_then(|parent| source_ids.get(parent)) {
        return Ok(FileAddress::Directory {
            directory_id: id.clone(),
            relative_path: HistoryRelative::parse(
                source
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| invalid("container basename is not UTF-8"))?,
            )?,
        });
    }
    stable_source(anchor, file, source)
}
fn stable_goal(state: &State, file: &str, slot: &str) -> Result<(FileAddress, LayoutBinding)> {
    let (location, name) = super::projection_path(state, file, slot);
    let path = match location {
        Some(location) => {
            super::validate_new_projection_path(Some(location))?;
            format!("namespace/{location}")
        }
        None => name
            .ok_or_else(|| invalid("managed file has no admitted outside container name"))?
            .to_owned(),
    };
    let address = FileAddress::Stable {
        relative_path: StableRelative::parse(&path)?,
    };
    let binding = LayoutBinding::parse(&object_hash(
        "directory-layout-outside",
        &(slot, file, &path),
    )?)?;
    Ok((address, binding))
}

fn write_mode(root: &DiskRoot) -> Result<()> {
    let marker = root.path().join("metadata/directory-layout.json");
    if super::path_exists_at(root, root.path(), &marker)? {
        if !super::file_exists_at(root, &marker)? {
            return Err(invalid(
                "directory layout mode marker is not one ordinary file",
            ));
        }
        let bytes = root.read_bytes(&marker, 128)?;
        let value: String = decode_strict(&bytes)?;
        if value != MODE {
            return Err(Error::new(
                ErrorCode::FormatUnsupported,
                "unknown directory layout mode",
            ));
        }
        root.sync_dir(marker.parent().expect("mode marker parent"))?;
    } else {
        root.write_json(&marker, &MODE, false)?;
    }
    Ok(())
}

impl PreparedLayouts {
    pub(super) fn reserve_databases(
        &self,
        reservations: &mut crate::database_install::Reservations,
    ) -> Result<()> {
        for history in &self.histories {
            for (_, install) in &history.databases {
                reservations.add(&history.disk, &history.history, install.reservation())?;
            }
        }
        Ok(())
    }
    pub(super) fn projection(
        &self,
        slot: &str,
        history: &str,
        file: &str,
    ) -> Option<&PhysicalProjection> {
        self.projections
            .get(&(slot.to_owned(), history.to_owned(), file.to_owned()))
    }
    pub(super) fn files(&self) -> impl Iterator<Item = &String> {
        self.projections.keys().map(|(_, _, file)| file)
    }
    pub(super) fn apply(
        self,
        observe: &mut impl FnMut(&'static str) -> Result<()>,
    ) -> Result<DirectoryLayoutReceipt> {
        let mut receipt = DirectoryLayoutReceipt {
            state_sequence: self.sequence,
            unavailable_containers: self.missing,
            unavailable_histories: self.missing_histories,
            ..Default::default()
        };
        for mut history in self.histories {
            let root = &history.disk.root;
            // Durable before the first new-format locator or visible move. This
            // disables old optional checkpoints with incomplete path coverage.
            write_mode(root)?;
            for directory in [".zxs-layout/staging", "directory-repositories"] {
                root.private_dir(&history.history.join(directory))?;
            }
            let anchor = HistoryAnchor::new(root, &history.history, &history.history_id)?;
            for deferred in history.deferred {
                if !deferred.inode.matches(root, &deferred.source)? {
                    return Err(invalid("legacy source inode changed after preparation"));
                }
                // Only Finalize/explicit published-State repair authorizes this
                // first owner mark. No marker is written during preparation.
                let marker = deferred.source.join(super::OWNER_MARKER);
                if !super::marker_matches_at(root, &marker, &deferred.file_id, &mut |_| Ok(()))? {
                    root.write_json(&marker, &deferred.file_id, false)?;
                }
                root.sync_dir(&deferred.source)?;
                let intent = FileIntent::new(
                    &history.history_id,
                    &deferred.file_id,
                    deferred.goal.binding,
                    deferred.inode,
                    deferred.from,
                    deferred.goal.to,
                )?;
                history.batch.files.push(FileMove {
                    intent,
                    previous: None,
                });
            }
            // Fixed file staging parents and admitted outside namespace parents
            // are metadata anchors. User-visible shared targets are never mkdir'd.
            for change in &history.batch.files {
                let stable = history
                    .history
                    .join("file-containers")
                    .join(change.intent.file_id());
                root.private_dir(&stable)?;
                if let FileAddress::Stable { relative_path } = change.intent.to() {
                    let target = stable.join(relative_path.as_str());
                    let parent = target.parent().expect("stable target parent");
                    root.private_dir(parent)?;
                    if relative_path.as_str().starts_with("namespace/") {
                        let marker = stable.join("namespace").join(super::NAMESPACE_MARKER);
                        if !super::marker_matches_at(
                            root,
                            &marker,
                            change.intent.file_id(),
                            &mut |_| Ok(()),
                        )? {
                            root.write_json(&marker, &change.intent.file_id(), false)?;
                        }
                    }
                }
            }
            // The batch may partially persist locators before failing. From
            // this point the preparation owner must preserve every new entity.
            history.private.referenced = true;
            layout_batch::apply(&anchor, &history.batch, &mut |_| Ok(()))?;
            for (id, database) in history.databases {
                let intent = layout_locator::load_directory(&anchor, &id, &mut |_| Ok(()))?;
                let target = layout_locator::locate_directory(&anchor, &intent, &mut |_| Ok(()))?;
                database.apply(root, observe)?;
                // All mutation used the originally held entity, even if its
                // name was replaced. Do not claim completion at another inode.
                if !intent.inode().matches(root, &target)? {
                    return Err(invalid("database entity name changed during installation"));
                }
            }
            history.private.cleanup_completed_parent();
            receipt.directories = receipt
                .directories
                .checked_add(history.batch.directories.len())
                .ok_or_else(|| {
                    Error::new(ErrorCode::ResourceLimit, "directory receipt count overflow")
                })?;
            receipt.containers = receipt
                .containers
                .checked_add(history.batch.files.len())
                .ok_or_else(|| {
                    Error::new(ErrorCode::ResourceLimit, "file receipt count overflow")
                })?;
            // Missing goals were retained at preparation and not fabricated.
            for file in history.missing {
                receipt.unavailable_containers.push((
                    history.slot_id.clone(),
                    history.history_id.clone(),
                    file,
                ));
            }
        }
        Ok(receipt)
    }
}

/// Prepare only, with no published name/locator/owner mutation. This invocation
/// cleans its proven, unreferenced empty directories on refusal/cancellation.
/// Uncertain creations or cleanup failures remain preserved, never adopted.
pub(super) fn prepare(
    config: &ValidatedConfig,
    resources: &Arc<DiskResources>,
    previous: &State,
    next: &State,
    explicit: Option<&BTreeSet<String>>,
    repair: bool,
    admission: crate::database_install::InstallAdmission,
) -> Result<PreparedLayouts> {
    let selected = match explicit {
        Some(selected) => selected.clone(),
        None => active_outer_ids(config, resources, &[previous, next])?,
    };
    let goals = layout_goals::build(next, Some(&selected))?;
    let mut prepared = PreparedLayouts {
        sequence: goals.state_sequence,
        ..Default::default()
    };
    let authority = authorized_directory_ids(&[previous, next], &selected);
    let authorized_files: BTreeSet<_> = [previous, next]
        .into_iter()
        .flat_map(|state| {
            state
                .versions
                .values()
                .map(|version| (version.history_tree_id.as_str(), version.file_id.as_str()))
        })
        .collect();
    let current_primaries: BTreeSet<_> = next
        .versions
        .values()
        .map(|version| {
            let placement = &next.placements[&version.version_id];
            (
                placement.primary_slot_id.as_str(),
                version.history_tree_id.as_str(),
                version.file_id.as_str(),
            )
        })
        .collect();
    let mut groups: BTreeMap<HistoryKey, Option<&HistoryGoals>> = goals
        .histories
        .values()
        .map(|goal| ((goal.slot_id.clone(), goal.history_id.clone()), Some(goal)))
        .collect();
    // Include every active old slot/history, even when its last primary moved
    // away. Such files return to their admitted per-file outside/backup address.
    for state in [previous, next] {
        for history in state
            .histories
            .values()
            .filter(|history| selected.contains(&history.repository_id))
        {
            for disk in config.io_disks() {
                let Some(held) = resources.available(disk) else {
                    continue;
                };
                let path = disk.root.join("histories").join(&history.history_tree_id);
                if super::path_directory_exists_at(&held.root, held.root.path(), &path)?
                    && (!record_names(&held.root, &path, "directories")?.is_empty()
                        || !record_names(&held.root, &path, "files")?.is_empty())
                {
                    groups
                        .entry((disk.slot_id.clone(), history.history_tree_id.clone()))
                        .or_insert(None);
                }
            }
        }
    }
    let mut database_bytes = 0usize;
    for ((slot, history_id), goal) in groups {
        let disk = config
            .disks
            .iter()
            .find(|disk| disk.slot_id == slot)
            .ok_or_else(|| {
                Error::new(ErrorCode::DiskNotFound, "layout references an absent slot")
            })?;
        let held = match resources.available(disk) {
            Some(held) => held,
            None if repair || config.excludes_io_disk(&disk.disk_id) => {
                prepared
                    .missing_histories
                    .push((slot.clone(), history_id.clone()));
                if let Some(goal) = goal {
                    prepared.missing.extend(
                        goal.files
                            .keys()
                            .map(|file| (slot.clone(), history_id.clone(), file.clone())),
                    );
                }
                continue;
            }
            None => resources.get(disk)?,
        };
        if held.root.available()? < config.metadata_reserve_bytes {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "directory layout preparation requires its metadata reserve",
            ));
        }
        let history = disk.root.join("histories").join(&history_id);
        // History roots and private preparation parents are data-root metadata,
        // not the user namespace. State supplies their exact authorized IDs.
        held.root.private_dir(&history)?;
        let anchor = HistoryAnchor::new(&held.root, &history, &history_id)?;
        let mut old = BTreeMap::new();
        let mut sources = BTreeMap::new();
        let mut source_ids = BTreeMap::new();
        for name in record_names(&held.root, &history, "directories")? {
            let id = DirectoryId::parse(&name)?;
            if !authority.contains(&id) {
                return Err(invalid(
                    "cached directory is outside the admitted State closure",
                ));
            }
            let intent = layout_locator::load_directory(&anchor, &id, &mut |_| Ok(()))?;
            let source = layout_locator::locate_directory(&anchor, &intent, &mut |_| Ok(()))?;
            if source_ids.insert(source.clone(), id.clone()).is_some() {
                return Err(invalid("two directory IDs identify one source"));
            }
            sources.insert(id.clone(), source);
            old.insert(id, intent);
        }
        let request = new_id("req")?;
        let private = history.join(".zxs-layout/prepared").join(request);
        let mut preparation = PrivatePreparation::new(held);
        let mut directories = Vec::new();
        if let Some(goal) = goal {
            for next_directory in goal.directories.values() {
                let id = &next_directory.id;
                let inode = if let Some(previous) = old.get(id) {
                    previous.inode()
                } else {
                    if preparation.directories.is_empty() {
                        held.root.private_dir(
                            private
                                .parent()
                                .expect("invariant: private preparation has a parent"),
                        )?;
                        preparation.create(&private)?;
                    }
                    let source = private.join(id.as_str());
                    let created = preparation.create(&source)?;
                    let inode = EntityInode::capture(&held.root, &source)?;
                    if inode.identity() != created {
                        return Err(invalid("exclusive prepared directory identity changed"));
                    }
                    source_ids.insert(source.clone(), id.clone());
                    sources.insert(id.clone(), source.clone());
                    inode
                };
                let source = &sources[id];
                let previous_parent = source
                    .parent()
                    .and_then(|parent| source_ids.get(parent))
                    .cloned();
                let intent = DirectoryIntent::new(
                    &history_id,
                    id.clone(),
                    next_directory.binding.clone(),
                    inode,
                    relative(&anchor, source)?,
                    next_directory.to.clone(),
                )?;
                directories.push(DirectoryMove {
                    intent,
                    previous: old.remove(id),
                    previous_parent,
                    next_parent: next_directory.parent.clone(),
                    retired: false,
                });
            }
        }
        for (id, previous) in old {
            let source = &sources[&id];
            let previous_parent = source
                .parent()
                .and_then(|parent| source_ids.get(parent))
                .cloned();
            let intent = DirectoryIntent::new(
                &history_id,
                id,
                previous.binding().clone(),
                previous.inode(),
                relative(&anchor, source)?,
                previous.staging().clone(),
            )?;
            directories.push(DirectoryMove {
                intent,
                previous: Some(previous),
                previous_parent,
                next_parent: None,
                retired: true,
            });
        }
        // All preparations now exist. Derive physical parents only after the
        // complete source closure was assembled, independent of map order.
        for change in &mut directories {
            let source = &sources[change.intent.directory_id()];
            change.previous_parent = source
                .parent()
                .and_then(|parent| source_ids.get(parent))
                .cloned();
        }
        let mut files = Vec::new();
        let mut deferred = Vec::new();
        let mut future = Vec::new();
        let mut missing = Vec::new();
        let mut known = BTreeSet::new();
        for name in record_names(&held.root, &history, "files")? {
            validate_id(&name, "file")?;
            // A removed old route can still leave an owned retained body on an
            // admitted root. Maintenance may preserve/place it, but this is not
            // evidence that it counts toward any current protection route.
            if !authorized_files.contains(&(history_id.as_str(), name.as_str())) {
                return Err(invalid(
                    "cached file is outside the admitted State history closure",
                ));
            }
            let previous = layout_locator::load_file(&anchor, &name, &mut |_| Ok(()))?;
            let source = layout_locator::locate_file(&anchor, &previous, &mut |_| Ok(()))?;
            let (to, binding) = if let Some(goal) = goal.and_then(|goal| goal.files.get(&name)) {
                (goal.to.clone(), goal.binding.clone())
            } else {
                // The absence of a managed goal alone is not outside proof.
                let routed_primary = current_primaries.contains(&(
                    slot.as_str(),
                    history_id.as_str(),
                    name.as_str(),
                ));
                if routed_primary
                    && !goals
                        .outside_files
                        .contains_key(&(history_id.clone(), name.clone()))
                {
                    return Err(invalid(
                        "managed file lost its goal without a proven current outside owner",
                    ));
                }
                stable_goal(next, &name, &slot)?
            };
            let intent = FileIntent::new(
                &history_id,
                &name,
                binding,
                previous.inode(),
                current_address(&anchor, &name, &source, &source_ids)?,
                to,
            )?;
            files.push(FileMove {
                intent,
                previous: Some(previous),
            });
            known.insert(name);
        }
        if let Some(goal) = goal {
            for file in goal.files.values() {
                prepared.projections.insert(
                    (slot.clone(), history_id.clone(), file.file_id.clone()),
                    PhysicalProjection {
                        name: file.cache_name.clone(),
                        location: file.root_relative_path.as_str().to_owned(),
                        root_directory_id: file.root_directory_id.as_str().to_owned(),
                        owner_directory_id: file.logical_owner_directory_id.as_str().to_owned(),
                    },
                );
                if known.contains(&file.file_id) {
                    continue;
                }
                // Missing/purged replicas are not materialized by a name change.
                let Some(source) = super::container_at(
                    &held.root,
                    &history,
                    Some(&history_id),
                    &file.file_id,
                    &mut |_| Ok(()),
                )?
                else {
                    missing.push(file.file_id.clone());
                    continue;
                };
                if !super::directory_exists_at(&held.root, &source.join("versions"))? {
                    return Err(invalid("legacy source container has no version directory"));
                }
                let inode = EntityInode::capture(&held.root, &source)?;
                let from = stable_source(&anchor, &file.file_id, &source)?;
                future.push(FutureFile {
                    file_id: file.file_id.clone(),
                    to: file.to.clone(),
                });
                deferred.push(DeferredFile {
                    file_id: file.file_id.clone(),
                    source,
                    inode,
                    from,
                    goal: file.clone(),
                });
            }
        }
        // Outside targets remain in an owner-marked per-file metadata namespace.
        // Empty parent preparations do not retarget the shared published entity.
        for change in &files {
            if let FileAddress::Stable { relative_path } = change.intent.to() {
                let stable = history
                    .join("file-containers")
                    .join(change.intent.file_id());
                held.root.private_dir(&stable)?;
                if relative_path.as_str().starts_with("namespace/") {
                    let namespace = stable.join("namespace");
                    if present(&held.root, &history, &namespace)? {
                        if !super::marker_matches_at(
                            &held.root,
                            &namespace.join(super::NAMESPACE_MARKER),
                            change.intent.file_id(),
                            &mut |_| Ok(()),
                        )? {
                            return Err(invalid(
                                "outside namespace has no admitted file owner marker",
                            ));
                        }
                    } else {
                        held.root.create_private_directory_synced(&namespace)?;
                        held.root.write_json(
                            &namespace.join(super::NAMESPACE_MARKER),
                            &change.intent.file_id(),
                            false,
                        )?;
                    }
                }
                let target = stable.join(relative_path.as_str());
                held.root
                    .private_dir(target.parent().expect("outside target parent"))?;
            }
        }
        let database_owners = directories
            .iter()
            .filter_map(|change| {
                let id = change.intent.directory_id();
                next.directory_git_repositories
                    .get(id.as_str())
                    .or_else(|| previous.directory_git_repositories.get(id.as_str()))
                    .map(|git| (id.clone(), git.repository_id.clone()))
            })
            .collect();
        let mut databases = Vec::new();
        if let Some(goal) = goal {
            for id in goal
                .directories
                .keys()
                .filter(|id| next.directory_git_repositories.contains_key(id.as_str()))
            {
                let database = super::directory_database::prepare(next, id.as_str())?;
                database_bytes += database.bytes.len();
                if database_bytes > super::directory_database::MAX_TOTAL_BYTES {
                    return Err(Error::new(
                        ErrorCode::ResourceLimit,
                        "directory databases exceed the layout preparation byte budget",
                    ));
                }
                let change = directories
                    .iter()
                    .find(|change| change.intent.directory_id() == id)
                    .expect("invariant: database has its directory move");
                let repository = &next.directory_git_repositories[id.as_str()].repository_id;
                let scope = crate::database_install::Scope::directory(
                    &history_id,
                    repository,
                    id.as_str(),
                )?;
                let install = crate::database_install::PreparedInstall::capture(
                    &held.root,
                    &history,
                    (&sources[id], change.intent.inode().identity()),
                    (scope, admission),
                    database.bytes,
                    super::directory_database::MAX_BYTES,
                    |bytes| super::directory_database::verify_scope(bytes, repository, id.as_str()),
                )?;
                databases.push((id.clone(), install));
            }
        }
        let (reserved_bytes, reserved_entries) =
            databases
                .iter()
                .try_fold((0u64, 0usize), |(bytes, entries), (_, install)| {
                    let (next_bytes, next_entries) = install.reservation();
                    Ok::<_, Error>((
                        bytes
                            .checked_add(next_bytes)
                            .ok_or_else(|| invalid("database retention budget overflow"))?,
                        entries
                            .checked_add(next_entries)
                            .ok_or_else(|| invalid("database retention entry budget overflow"))?,
                    ))
                })?;
        admission.check(&held.root, &history, reserved_bytes, reserved_entries)?;
        let batch = Batch {
            directories,
            files,
            databases: database_owners,
        };
        // Stage parents are metadata only; needed by the zero-rename preflight.
        held.root
            .private_dir(&history.join(".zxs-layout/staging"))?;
        held.root
            .private_dir(&history.join("directory-repositories"))?;
        layout_batch::preflight(&anchor, &batch, &future, &mut |_| Ok(()))?;
        prepared.histories.push(PreparedHistory {
            disk: Arc::clone(held),
            history,
            history_id,
            slot_id: slot,
            batch,
            databases,
            deferred,
            missing,
            private: preparation,
        });
    }
    Ok(prepared)
}
