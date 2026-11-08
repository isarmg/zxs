//! Pure placement plan over an admitted, immutable State.
//! The plan does not prove body availability or authorize filesystem publication.
use name_policy::PortablePath;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use storage_contract::{
    DirectoryBranch, DirectoryTree, Entry, FileContext, FileVersion, NamespaceEntryKind,
    Repository, State,
};

/// One body-container identity, independent of names, branches and content hashes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct LayoutFileKey<'a> {
    pub(crate) history_tree_id: &'a str,
    pub(crate) file_id: &'a str,
}

#[derive(Debug)]
pub(crate) struct LayoutOccurrence<'a> {
    pub(crate) directory_branch_id: &'a str,
    pub(crate) entry: &'a Entry,
    pub(crate) context: Option<&'a FileContext>,
}

#[derive(Debug)]
pub(crate) struct FileLayout<'a> {
    pub(crate) directory_id: &'a str,
    /// Relative to this independent repository, not the outer disk root.
    /// Parent spellings follow main's directory entries; `main.entry` remains
    /// the original logical occurrence, whose parent spelling may differ.
    pub(crate) path: PortablePath,
    /// None means the container is in the explicitly requested flat directory.
    pub(crate) main: Option<LayoutOccurrence<'a>>,
    /// Includes retained descriptors in every content state. This does not claim
    /// any body exists, and never authorizes creating an empty container.
    pub(crate) versions: Vec<&'a FileVersion>,
}

/// An effective scope witness: a current stable-main or explicit unmanaged
/// position wins; only absent current positions use retained scope witnesses.
/// Every snapshot uses currently initialized stable IDs, not initialization time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum LayoutOwner<'a> {
    Repository(&'a str),
    Unmanaged,
}

#[derive(Debug)]
pub(crate) struct OutsideFile<'a> {
    pub(crate) owners: BTreeSet<LayoutOwner<'a>>,
    pub(crate) versions: Vec<&'a FileVersion>,
}

#[derive(Debug)]
pub(crate) struct RepositoryLayout<'a> {
    pub(crate) directory_id: &'a str,
    pub(crate) main: &'a DirectoryBranch,
    /// None is the requested root. Child roots are relative to that root.
    /// This contains no live namespace mount or filesystem authorization.
    pub(crate) root: Option<PortablePath>,
    /// Main's ordinary directories, including empty directories. Keys are
    /// projected paths; values retain their original logical entry spelling.
    pub(crate) directories: BTreeMap<String, &'a Entry>,
    /// Direct independent children only, with repository-local mount paths.
    pub(crate) children: BTreeMap<&'a str, PortablePath>,
    pub(crate) extra_directory: PortablePath,
}

/// The borrow pins the exact admitted State for this plan's entire lifetime.
/// It cannot outlive a mutable Engine operation. No serialized plan or sequence
/// number is sufficient authorization for future filesystem operations.
#[derive(Debug)]
pub(crate) struct LayoutPlan<'a> {
    repository_id: &'a str,
    root_directory_id: &'a str,
    state: &'a State,
    repositories: BTreeMap<&'a str, RepositoryLayout<'a>>,
    files: BTreeMap<LayoutFileKey<'a>, FileLayout<'a>>,
    outside_files: BTreeMap<LayoutFileKey<'a>, OutsideFile<'a>>,
}
impl<'a> LayoutPlan<'a> {
    pub(crate) fn repository_id(&self) -> &'a str {
        self.repository_id
    }
    pub(crate) fn root_directory_id(&self) -> &'a str {
        self.root_directory_id
    }
    pub(crate) fn state_sequence(&self) -> u64 {
        self.state.sequence
    }
    pub(crate) fn repositories(&self) -> &BTreeMap<&'a str, RepositoryLayout<'a>> {
        &self.repositories
    }
    pub(crate) fn files(&self) -> &BTreeMap<LayoutFileKey<'a>, FileLayout<'a>> {
        &self.files
    }
    pub(crate) fn outside_files(&self) -> &BTreeMap<LayoutFileKey<'a>, OutsideFile<'a>> {
        &self.outside_files
    }
}

#[derive(Debug, error_definition::Error)]
pub enum LayoutError {
    #[error("repository {repository_id} does not exist")]
    RepositoryMissing { repository_id: String },
    #[error("repository {repository_id} current namespace {branch_id}/{commit_id:?} is unproven")]
    CurrentNamespaceUnproven {
        repository_id: String,
        branch_id: String,
        commit_id: Option<String>,
    },
    #[error(
        "file {history_tree_id}/{file_id} is in main {directory_id}/{main_path} and current unmanaged paths {unmanaged_paths:?}"
    )]
    CurrentMainUnmanagedConflict {
        history_tree_id: String,
        file_id: String,
        directory_id: String,
        main_path: String,
        unmanaged_paths: Vec<String>,
    },
    #[error("root directory repository {directory_id} is not initialized")]
    RootNotInitialized { directory_id: String },
    #[error("repository {directory_id} has no proven exterior placement relative to this root")]
    RepositoryOwnerMissing { directory_id: String },
    #[error("tree {tree_id} has no exact commit ancestry for scope {directory_id} in {commit_id}")]
    SnapshotScopeUnproven {
        tree_id: String,
        directory_id: String,
        commit_id: String,
    },
    #[error("tree {tree_id} has no ordinary directory parent for {path} at {parent}")]
    SnapshotStructureUnproven {
        tree_id: String,
        path: String,
        parent: String,
    },
    #[error("invalid requested flat directory name: {0}")]
    ExtraDirectoryName(#[source] name_policy::Error),
    #[error("flat directory {name} conflicts with main path {path} in {directory_id}")]
    ExtraDirectoryCollision {
        directory_id: String,
        name: String,
        path: String,
    },
    #[error("reserved directory metadata name {name} conflicts with {path} in {directory_id}")]
    ReservedMetadataCollision {
        directory_id: String,
        name: String,
        path: String,
    },
    #[error("main target {path} in {directory_id} has no ordinary directory parent at {parent}")]
    MainStructureConflict {
        directory_id: String,
        path: String,
        parent: String,
    },
    #[error("repository {directory_id} has multiple main mounts: {mounts:?}")]
    MultipleRepositoryMounts {
        directory_id: String,
        mounts: Vec<(String, String)>,
    },
    #[error("main repository composition contains a cycle: {directory_ids:?}")]
    RepositoryCycle { directory_ids: Vec<String> },
    #[error("repository {directory_id} is outside its parent main; its placement is undecided")]
    UnplacedRepository { directory_id: String },
    #[error("file {history_tree_id}/{file_id} has multiple main occurrences: {paths:?}")]
    MultipleMainOccurrences {
        history_tree_id: String,
        file_id: String,
        paths: Vec<(String, String)>,
    },
    #[error("file {history_tree_id}/{file_id} has no proven independent repository owner")]
    FileOwnerMissing {
        history_tree_id: String,
        file_id: String,
    },
    #[error("file {history_tree_id}/{file_id} has ambiguous repository owners: {directory_ids:?}")]
    FileOwnerAmbiguous {
        history_tree_id: String,
        file_id: String,
        directory_ids: Vec<String>,
    },
    #[error(
        "file {history_tree_id}/{file_id} crosses root {root_directory_id}: repositories {directory_ids:?}, unmanaged={unmanaged}"
    )]
    FileOwnerCrossesRoot {
        history_tree_id: String,
        file_id: String,
        root_directory_id: String,
        directory_ids: Vec<String>,
        unmanaged: bool,
    },
    #[error(
        "unwritten file identity {file_id} references body owner {body_file_id}; its layout is undecided"
    )]
    UnwrittenFileIdentity {
        file_id: String,
        body_file_id: String,
    },
    #[error("projected relative path {path} exceeds the portable layout budget: {source}")]
    PhysicalPath {
        path: String,
        #[source]
        source: name_policy::Error,
    },
    #[error("directory layout exceeds its bounded metadata work or path-byte budget")]
    ResourceLimit,
}
type Result<T> = std::result::Result<T, LayoutError>;

const MAX_STEPS: usize = 1_000_000;
const MAX_PATH_BYTES: usize = 16 * 1024 * 1024;
struct Budget {
    steps: usize,
    bytes: usize,
}
impl Budget {
    fn charge(&mut self, bytes: usize) -> Result<()> {
        if self.steps == MAX_STEPS || bytes > MAX_PATH_BYTES - self.bytes {
            return Err(LayoutError::ResourceLimit);
        }
        self.steps += 1;
        self.bytes += bytes;
        Ok(())
    }
}

/// Each snapshot proves boundaries by stable directory identity. String paths
/// only locate those identities inside that exact snapshot. file_locations is
/// never ownership evidence; current namespace positions confer no I/O authority.
struct Boundaries<'a>(BTreeMap<&'a str, &'a str>);
impl<'a> Boundaries<'a> {
    fn new(entries: &'a [Entry], registered: &BTreeSet<&str>, budget: &mut Budget) -> Result<Self> {
        let mut paths = BTreeMap::new();
        for entry in entries {
            budget.charge(entry.path().len())?;
            if entry.kind == NamespaceEntryKind::Directory
                && registered.contains(entry.entry_id.as_str())
            {
                paths.insert(entry.lookup_key(), entry.entry_id.as_str());
            }
        }
        Ok(Self(paths))
    }
    fn owner(&self, path: &str) -> Option<&'a str> {
        let mut parent = path;
        while let Some((next, _)) = parent.rsplit_once('/') {
            if let Some(directory) = self.0.get(next) {
                return Some(directory);
            }
            parent = next;
        }
        None
    }
}

struct MainStructure<'a> {
    scope: String,
    entries: BTreeMap<&'a str, &'a Entry>,
}
struct MainFile<'a> {
    directory_id: &'a str,
    occurrence: LayoutOccurrence<'a>,
    path: PortablePath,
}
struct MainProjection<'a> {
    repositories: BTreeMap<&'a str, RepositoryLayout<'a>>,
    files: BTreeMap<LayoutFileKey<'a>, MainFile<'a>>,
}

fn beneath_main_parent(
    entry: &Entry,
    parent: Option<&PortablePath>,
    budget: &mut Budget,
) -> Result<PortablePath> {
    let depth = entry.path().split('/').count() - 1;
    let bytes = entry.name().len() + parent.map_or(0, |path| path.as_str().len() + 1);
    budget.charge(bytes)?;
    match parent {
        Some(parent) => {
            entry
                .location()
                .rebase(depth, parent)
                .map_err(|source| LayoutError::PhysicalPath {
                    path: format!("{}/{}", parent.as_str(), entry.name()),
                    source,
                })
        }
        None => Ok(entry
            .location()
            .strip_components(depth)
            .expect("invariant: admitted entry has a leaf component")),
    }
}
fn physical(path: String, budget: &mut Budget) -> Result<PortablePath> {
    budget.charge(path.len())?;
    PortablePath::parse(&path).map_err(|source| LayoutError::PhysicalPath { path, source })
}
fn joined(root: Option<&PortablePath>, path: &str, budget: &mut Budget) -> Result<PortablePath> {
    physical(
        match root {
            Some(root) => format!("{}/{path}", root.as_str()),
            None => path.to_owned(),
        },
        budget,
    )
}
fn key<'a>(state: &'a State, entry: &Entry) -> Result<LayoutFileKey<'a>> {
    let version = &state.versions[&entry.version_id];
    if version.file_id != entry.file_id {
        return Err(LayoutError::UnwrittenFileIdentity {
            file_id: entry.file_id.clone(),
            body_file_id: version.file_id.clone(),
        });
    }
    Ok(LayoutFileKey {
        history_tree_id: &version.history_tree_id,
        file_id: &version.file_id,
    })
}

fn validate_main_structure<'a>(
    main: &'a DirectoryBranch,
    boundaries: &Boundaries<'_>,
    budget: &mut Budget,
) -> Result<MainStructure<'a>> {
    let mut own = BTreeMap::new();
    for entry in &main.entries {
        budget.charge(0)?;
        if boundaries.owner(entry.lookup_key()).is_none() {
            own.insert(entry.lookup_key(), entry);
        }
    }
    let scope = name_policy::path_key(&main.path);
    budget.charge(scope.len())?;
    for (path, entry) in &own {
        budget.charge(0)?;
        let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
        if parent != scope
            && !own
                .get(parent)
                .is_some_and(|entry| entry.kind == NamespaceEntryKind::Directory)
        {
            return Err(LayoutError::MainStructureConflict {
                directory_id: main.directory_id.clone(),
                path: entry.path().into(),
                parent: parent.into(),
            });
        }
    }
    Ok(MainStructure {
        scope,
        entries: own,
    })
}

fn insert_main_file<'a>(
    state: &'a State,
    main: &'a DirectoryBranch,
    entry: &'a Entry,
    path: PortablePath,
    files: &mut BTreeMap<LayoutFileKey<'a>, MainFile<'a>>,
) -> Result<()> {
    let key = key(state, entry)?;
    let occurrence = LayoutOccurrence {
        directory_branch_id: &main.directory_branch_id,
        entry,
        context: main.file_contexts.get(entry.lookup_key()),
    };
    if let Some(first) = files.insert(
        key,
        MainFile {
            directory_id: &main.directory_id,
            occurrence,
            path,
        },
    ) {
        return Err(LayoutError::MultipleMainOccurrences {
            history_tree_id: key.history_tree_id.into(),
            file_id: key.file_id.into(),
            paths: vec![
                (
                    first.directory_id.into(),
                    first.occurrence.entry.path().into(),
                ),
                (main.directory_id.clone(), entry.path().into()),
            ],
        });
    }
    Ok(())
}

fn main_layouts<'a>(
    state: &'a State,
    included: &BTreeSet<&'a str>,
    registered: &BTreeSet<&'a str>,
    extra: &PortablePath,
    budget: &mut Budget,
) -> Result<MainProjection<'a>> {
    let mut result = MainProjection {
        repositories: BTreeMap::new(),
        files: BTreeMap::new(),
    };
    for directory in included {
        budget.charge(0)?;
        let git = &state.directory_git_repositories[*directory];
        let main = &state.directory_branches[&git.main_branch_id];
        let boundaries = Boundaries::new(&main.entries, registered, budget)?;
        let structure = validate_main_structure(main, &boundaries, budget)?;
        let mut layout = RepositoryLayout {
            directory_id: directory,
            main,
            root: None,
            directories: BTreeMap::new(),
            children: BTreeMap::new(),
            extra_directory: extra.clone(),
        };
        let mut ordinary = BTreeMap::<&str, (&Entry, PortablePath)>::new();
        // A parent's lookup key is a strict prefix of every child key. This
        // order builds each physical parent once, independent of Entry order.
        for (lookup_key, entry) in structure.entries {
            budget.charge(0)?;
            let parent_key = lookup_key.rsplit_once('/').map_or("", |(parent, _)| parent);
            let parent = if parent_key == structure.scope {
                None
            } else {
                Some(
                    &ordinary
                        .get(parent_key)
                        .expect("invariant: validated own entry has an earlier ordinary parent")
                        .1,
                )
            };
            let path = beneath_main_parent(entry, parent, budget)?;
            if path.key().split('/').next() == Some(extra.key()) {
                return Err(LayoutError::ExtraDirectoryCollision {
                    directory_id: (*directory).into(),
                    name: extra.as_str().into(),
                    path: path.into_string(),
                });
            }
            if path.key().split('/').next()
                == Some(crate::container_storage::directory_database_name())
            {
                return Err(LayoutError::ReservedMetadataCollision {
                    directory_id: (*directory).into(),
                    name: crate::container_storage::directory_database_name().into(),
                    path: path.into_string(),
                });
            }
            match entry.kind {
                NamespaceEntryKind::Directory => {
                    if registered.contains(entry.entry_id.as_str()) {
                        if let Some(first) = layout.children.get(entry.entry_id.as_str()) {
                            return Err(LayoutError::MultipleRepositoryMounts {
                                directory_id: entry.entry_id.clone(),
                                mounts: vec![
                                    ((*directory).into(), first.as_str().into()),
                                    ((*directory).into(), path.into_string()),
                                ],
                            });
                        }
                        layout.children.insert(entry.entry_id.as_str(), path);
                    } else {
                        ordinary.insert(lookup_key, (entry, path));
                    }
                }
                NamespaceEntryKind::File => {
                    insert_main_file(state, main, entry, path, &mut result.files)?;
                }
            }
        }
        for (entry, path) in ordinary.into_values() {
            budget.charge(0)?;
            layout.directories.insert(path.into_string(), entry);
        }
        result.repositories.insert(*directory, layout);
    }
    Ok(result)
}

fn mount_roots<'a>(
    root: &'a str,
    layouts: &mut BTreeMap<&'a str, RepositoryLayout<'a>>,
    budget: &mut Budget,
) -> Result<()> {
    let mut parents = BTreeMap::new();
    for (parent, layout) in layouts.iter() {
        for (child, path) in &layout.children {
            budget.charge(0)?;
            if let Some((first, first_path)) = parents.insert(*child, (*parent, path.as_str())) {
                return Err(LayoutError::MultipleRepositoryMounts {
                    directory_id: (*child).into(),
                    mounts: vec![
                        (first.into(), first_path.into()),
                        ((*parent).into(), path.as_str().into()),
                    ],
                });
            }
        }
    }
    // Kahn's traversal also checks disconnected cycles, rather than hiding them
    // behind a visited set while traversing only the requested root.
    let mut ready = layouts
        .keys()
        .filter(|id| !parents.contains_key(**id))
        .copied()
        .collect::<VecDeque<_>>();
    let mut order = Vec::new();
    while let Some(id) = ready.pop_front() {
        budget.charge(0)?;
        order.push(id);
        ready.extend(layouts[id].children.keys().copied());
    }
    if order.len() != layouts.len() {
        let reached = order.into_iter().collect::<BTreeSet<_>>();
        return Err(LayoutError::RepositoryCycle {
            directory_ids: layouts
                .keys()
                .filter(|id| !reached.contains(**id))
                .map(|id| (*id).into())
                .collect(),
        });
    }
    for id in layouts.keys() {
        if *id != root && !parents.contains_key(*id) {
            return Err(LayoutError::UnplacedRepository {
                directory_id: (*id).into(),
            });
        }
    }
    // Drop borrowed mount paths before updating the output roots.
    drop(parents);
    for id in order {
        let next = layouts[id]
            .children
            .iter()
            .map(|(child, path)| {
                joined(layouts[id].root.as_ref(), path.as_str(), budget).map(|path| (*child, path))
            })
            .collect::<Result<Vec<_>>>()?;
        for (child, path) in next {
            layouts
                .get_mut(child)
                .expect("invariant: registered child has a layout")
                .root = Some(path);
        }
    }
    Ok(())
}

/// The outer initialized root encloses all registered repositories. A selected
/// subdirectory instead reaches only children mounted by each stable main.
fn included_repositories<'a>(
    state: &'a State,
    outer: &str,
    root: &'a str,
    registered: &BTreeSet<&'a str>,
    budget: &mut Budget,
) -> Result<BTreeSet<&'a str>> {
    if root == outer {
        return Ok(registered.clone());
    }
    let mut included = BTreeSet::new();
    let mut pending = vec![root];
    while let Some(directory) = pending.pop() {
        budget.charge(0)?;
        if !included.insert(directory) {
            continue;
        }
        let git = &state.directory_git_repositories[directory];
        let main = &state.directory_branches[&git.main_branch_id];
        let boundaries = Boundaries::new(&main.entries, registered, budget)?;
        for entry in &main.entries {
            budget.charge(0)?;
            if entry.kind == NamespaceEntryKind::Directory
                && registered.contains(entry.entry_id.as_str())
                && boundaries.owner(entry.lookup_key()).is_none()
            {
                pending.push(entry.entry_id.as_str());
            }
        }
    }
    Ok(included)
}

struct CurrentMain<'a> {
    directory_id: &'a str,
    entry: &'a Entry,
}

/// Resolve every registered stable main, including other forest roots. Parent
/// snapshots stop at stable child boundaries, so only each main's own files
/// count. An alias in another retained branch is not another current entity.
fn current_main_homes<'a>(
    state: &'a State,
    registered: &BTreeSet<&'a str>,
    budget: &mut Budget,
) -> Result<BTreeMap<LayoutFileKey<'a>, CurrentMain<'a>>> {
    let mut homes = BTreeMap::<LayoutFileKey<'a>, CurrentMain<'a>>::new();
    for directory in registered {
        budget.charge(0)?;
        let git = &state.directory_git_repositories[*directory];
        let main = &state.directory_branches[&git.main_branch_id];
        let boundaries = Boundaries::new(&main.entries, registered, budget)?;
        let structure = validate_main_structure(main, &boundaries, budget)?;
        for entry in structure.entries.into_values() {
            budget.charge(0)?;
            if entry.kind != NamespaceEntryKind::File {
                continue;
            }
            let key = key(state, entry)?;
            if let Some(first) = homes.insert(
                key,
                CurrentMain {
                    directory_id: directory,
                    entry,
                },
            ) {
                return Err(LayoutError::MultipleMainOccurrences {
                    history_tree_id: key.history_tree_id.into(),
                    file_id: key.file_id.into(),
                    paths: vec![
                        (first.directory_id.into(), first.entry.path().into()),
                        ((*directory).into(), entry.path().into()),
                    ],
                });
            }
        }
    }
    Ok(homes)
}

/// This exact raw State Commit supplies only current namespace placement facts.
/// It is neither a current-registry visibility witness nor permission to read,
/// move, or republish payloads; execution retains its admitted view and fences.
fn current_unmanaged<'a>(
    state: &'a State,
    repository: &'a Repository,
    registered: &BTreeSet<&'a str>,
    budget: &mut Budget,
) -> Result<BTreeMap<LayoutFileKey<'a>, Vec<&'a Entry>>> {
    let branch = state
        .branches
        .get(&repository.main_branch_id)
        .ok_or_else(|| LayoutError::CurrentNamespaceUnproven {
            repository_id: repository.repository_id.clone(),
            branch_id: repository.main_branch_id.clone(),
            commit_id: None,
        })?;
    let unproven = || LayoutError::CurrentNamespaceUnproven {
        repository_id: repository.repository_id.clone(),
        branch_id: branch.branch_id.clone(),
        commit_id: Some(branch.head_commit_id.clone()),
    };
    let commit = state
        .commits
        .get(&branch.head_commit_id)
        .ok_or_else(unproven)?;
    if branch.history_tree_id != repository.history_tree_id
        || branch.file_id.is_some()
        || commit.history_tree_id != branch.history_tree_id
    {
        return Err(unproven());
    }
    let boundaries = Boundaries::new(&commit.entries, registered, budget)?;
    let mut unmanaged = BTreeMap::<LayoutFileKey<'a>, Vec<&'a Entry>>::new();
    for entry in &commit.entries {
        budget.charge(0)?;
        if entry.kind == NamespaceEntryKind::File
            && !registered.contains(repository.repository_id.as_str())
            && boundaries.owner(entry.lookup_key()).is_none()
        {
            unmanaged.entry(key(state, entry)?).or_default().push(entry);
        }
    }
    Ok(unmanaged)
}

fn reject_current_position_conflicts<'a>(
    mains: &BTreeMap<LayoutFileKey<'a>, CurrentMain<'a>>,
    unmanaged: &BTreeMap<LayoutFileKey<'a>, Vec<&'a Entry>>,
    budget: &mut Budget,
) -> Result<()> {
    for (key, main) in mains {
        budget.charge(0)?;
        if let Some(entries) = unmanaged.get(key) {
            return Err(LayoutError::CurrentMainUnmanagedConflict {
                history_tree_id: key.history_tree_id.into(),
                file_id: key.file_id.into(),
                directory_id: main.directory_id.into(),
                main_path: main.entry.path().into(),
                unmanaged_paths: entries
                    .iter()
                    .map(|entry| entry.path().to_owned())
                    .collect(),
            });
        }
    }
    Ok(())
}

struct OwnershipEvidence<'a> {
    resolved: BTreeSet<LayoutFileKey<'a>>,
    files: BTreeMap<LayoutFileKey<'a>, BTreeSet<LayoutOwner<'a>>>,
    directories: BTreeMap<&'a str, BTreeSet<LayoutOwner<'a>>>,
}
struct SnapshotScope<'a> {
    entries: &'a [Entry],
    owner: LayoutOwner<'a>,
}

fn add_snapshot<'a>(
    state: &'a State,
    registered: &BTreeSet<&'a str>,
    snapshot: SnapshotScope<'a>,
    evidence: &mut OwnershipEvidence<'a>,
    budget: &mut Budget,
) -> Result<()> {
    let boundaries = Boundaries::new(snapshot.entries, registered, budget)?;
    for entry in snapshot.entries {
        budget.charge(0)?;
        let owner = boundaries
            .owner(entry.lookup_key())
            .map(LayoutOwner::Repository)
            .unwrap_or(snapshot.owner);
        match entry.kind {
            NamespaceEntryKind::File => {
                let key = key(state, entry)?;
                if !evidence.resolved.contains(&key) {
                    evidence.files.entry(key).or_default().insert(owner);
                }
            }
            NamespaceEntryKind::Directory => {
                if registered.contains(entry.entry_id.as_str()) {
                    evidence
                        .directories
                        .entry(entry.entry_id.as_str())
                        .or_default()
                        .insert(owner);
                }
            }
        }
    }
    Ok(())
}

/// Commit admission proves complete ancestor chains. Scoped Tree admission
/// instead proves containment and references; layout ownership additionally
/// needs every ordinary parent down to its implicit stable scope root.
fn validate_tree_structure(tree: &DirectoryTree, budget: &mut Budget) -> Result<()> {
    let mut kinds = BTreeMap::new();
    for entry in &tree.entries {
        budget.charge(0)?;
        kinds.insert(entry.lookup_key(), entry.kind);
    }
    let scope = name_policy::path_key(&tree.path);
    budget.charge(scope.len())?;
    for entry in &tree.entries {
        budget.charge(0)?;
        let parent = entry
            .lookup_key()
            .rsplit_once('/')
            .map_or("", |(parent, _)| parent);
        if parent != scope && kinds.get(parent).copied() != Some(NamespaceEntryKind::Directory) {
            return Err(LayoutError::SnapshotStructureUnproven {
                tree_id: tree.tree_id.clone(),
                path: entry.path().into(),
                parent: parent.into(),
            });
        }
    }
    Ok(())
}

/// A scoped tree omits its own root and its exterior ancestors. An initialized
/// scope supplies its stable owner directly. An ordinary scope needs the exact
/// historical commit's matching identity and complete directory ancestor chain;
/// no current namespace, physical path, or initialization-time fallback is used.
fn tree_scope_owner<'a>(
    state: &'a State,
    tree: &'a DirectoryTree,
    registered: &BTreeSet<&'a str>,
    outer_owner: LayoutOwner<'a>,
    budget: &mut Budget,
) -> Result<LayoutOwner<'a>> {
    let scope = &state.directory_branches[&tree.directory_branch_id];
    if registered.contains(scope.directory_id.as_str()) {
        return Ok(LayoutOwner::Repository(&scope.directory_id));
    }
    if tree.path.is_empty() && scope.directory_id == scope.repository_id {
        return Ok(outer_owner);
    }
    let unproven = || LayoutError::SnapshotScopeUnproven {
        tree_id: tree.tree_id.clone(),
        directory_id: scope.directory_id.clone(),
        commit_id: tree.commit_id.clone(),
    };
    let commit = state.commits.get(&tree.commit_id).ok_or_else(unproven)?;
    let scope_key = name_policy::path_key(&tree.path);
    budget.charge(scope_key.len())?;
    let mut found = false;
    for entry in &commit.entries {
        budget.charge(0)?;
        if entry.kind == NamespaceEntryKind::Directory
            && entry.entry_id == scope.directory_id
            && entry.lookup_key() == scope_key
        {
            found = true;
            break;
        }
    }
    if !found {
        return Err(unproven());
    }
    let boundaries = Boundaries::new(&commit.entries, registered, budget)?;
    Ok(boundaries
        .owner(&scope_key)
        .map(LayoutOwner::Repository)
        .unwrap_or(outer_owner))
}

fn ownership_evidence<'a>(
    state: &'a State,
    outer: &'a str,
    registered: &BTreeSet<&'a str>,
    resolved: BTreeSet<LayoutFileKey<'a>>,
    budget: &mut Budget,
) -> Result<OwnershipEvidence<'a>> {
    let outer_owner = if registered.contains(outer) {
        LayoutOwner::Repository(outer)
    } else {
        LayoutOwner::Unmanaged
    };
    let mut evidence = OwnershipEvidence {
        resolved,
        files: BTreeMap::new(),
        directories: BTreeMap::new(),
    };
    // Admitted Commit entries include every directory ancestor, including file
    // branch snapshots. A proven ancestor chain with no registered boundary is
    // positive Unmanaged evidence; a missing occurrence supplies no evidence.
    for commit in state.commits.values() {
        budget.charge(0)?;
        if state.histories[&commit.history_tree_id].repository_id == outer {
            add_snapshot(
                state,
                registered,
                SnapshotScope {
                    entries: &commit.entries,
                    owner: outer_owner,
                },
                &mut evidence,
                budget,
            )?;
        }
    }
    for tree in state.directory_trees.values() {
        budget.charge(0)?;
        let scope = &state.directory_branches[&tree.directory_branch_id];
        if scope.repository_id != outer {
            continue;
        }
        let mut needed = false;
        for entry in &tree.entries {
            budget.charge(0)?;
            match entry.kind {
                NamespaceEntryKind::File => {
                    needed |= !evidence.resolved.contains(&key(state, entry)?);
                }
                NamespaceEntryKind::Directory => {
                    needed |= registered.contains(entry.entry_id.as_str());
                }
            }
        }
        if !needed {
            continue;
        }
        let owner = tree_scope_owner(state, tree, registered, outer_owner, budget)?;
        validate_tree_structure(tree, budget)?;
        add_snapshot(
            state,
            registered,
            SnapshotScope {
                entries: &tree.entries,
                owner,
            },
            &mut evidence,
            budget,
        )?;
    }
    Ok(evidence)
}

fn reject_omitted_repositories(
    evidence: &OwnershipEvidence<'_>,
    outer: &str,
    registered: &BTreeSet<&str>,
    included: &BTreeSet<&str>,
    budget: &mut Budget,
) -> Result<()> {
    for directory in registered.difference(included) {
        budget.charge(0)?;
        // The outer root is a proven enclosing scope, not a detached child of
        // the explicitly requested subdirectory root.
        if *directory == outer {
            continue;
        }
        let parents = evidence.directories.get(directory).ok_or_else(|| {
            LayoutError::RepositoryOwnerMissing {
                directory_id: (*directory).into(),
            }
        })?;
        for owner in parents {
            budget.charge(0)?;
            if let LayoutOwner::Repository(parent) = owner
                && included.contains(parent)
            {
                return Err(LayoutError::UnplacedRepository {
                    directory_id: (*directory).into(),
                });
            }
        }
    }
    Ok(())
}

/// Preserve the whole-outer-root entry point for callers that initialized it.
pub(crate) fn plan<'a>(
    state: &'a State,
    repository: &str,
    extra_name: &str,
) -> Result<LayoutPlan<'a>> {
    plan_root(state, repository, repository, extra_name)
}

/// Plan one initialized root and the children mounted by their stable mains.
/// All retained identities in the outer repository are either planned or backed
/// by explicit exterior evidence. Current stable mains and explicit unmanaged
/// positions in the exact outer namespace Commit take priority over retained
/// history. These are pure facts, not live registry visibility or I/O authority.
pub(crate) fn plan_root<'a>(
    state: &'a State,
    repository: &str,
    root_directory_id: &str,
    extra_name: &str,
) -> Result<LayoutPlan<'a>> {
    let repository =
        state
            .repositories
            .get(repository)
            .ok_or_else(|| LayoutError::RepositoryMissing {
                repository_id: repository.into(),
            })?;
    name_policy::validate_name(extra_name).map_err(LayoutError::ExtraDirectoryName)?;
    let extra = PortablePath::parse(extra_name).map_err(LayoutError::ExtraDirectoryName)?;
    let mut budget = Budget { steps: 0, bytes: 0 };
    let mut registered = BTreeSet::new();
    for git in state.directory_git_repositories.values() {
        budget.charge(0)?;
        if git.repository_id == repository.repository_id {
            registered.insert(git.directory_id.as_str());
        }
    }
    if !registered.contains(root_directory_id) {
        return Err(LayoutError::RootNotInitialized {
            directory_id: root_directory_id.into(),
        });
    }
    let root_directory_id = state.directory_git_repositories[root_directory_id]
        .directory_id
        .as_str();
    let current_mains = current_main_homes(state, &registered, &mut budget)?;
    let current_unmanaged = current_unmanaged(state, repository, &registered, &mut budget)?;
    reject_current_position_conflicts(&current_mains, &current_unmanaged, &mut budget)?;
    let resolved = current_mains
        .keys()
        .chain(current_unmanaged.keys())
        .copied()
        .collect();
    let included = included_repositories(
        state,
        &repository.repository_id,
        root_directory_id,
        &registered,
        &mut budget,
    )?;
    let MainProjection {
        mut repositories,
        files: mut mains,
    } = main_layouts(state, &included, &registered, &extra, &mut budget)?;
    mount_roots(root_directory_id, &mut repositories, &mut budget)?;
    let mut evidence = ownership_evidence(
        state,
        &repository.repository_id,
        &registered,
        resolved,
        &mut budget,
    )?;
    reject_omitted_repositories(
        &evidence,
        &repository.repository_id,
        &registered,
        &included,
        &mut budget,
    )?;
    let mut versions: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for version in state.versions.values() {
        budget.charge(0)?;
        if state.histories[&version.history_tree_id].repository_id == repository.repository_id {
            let key = LayoutFileKey {
                history_tree_id: &version.history_tree_id,
                file_id: &version.file_id,
            };
            versions.entry(key).or_default().push(version);
        }
    }
    let mut files = BTreeMap::new();
    let mut outside_files = BTreeMap::new();
    for (key, versions) in versions {
        budget.charge(0)?;
        let candidates = if let Some(main) = current_mains.get(&key) {
            BTreeSet::from([LayoutOwner::Repository(main.directory_id)])
        } else if current_unmanaged.contains_key(&key) {
            BTreeSet::from([LayoutOwner::Unmanaged])
        } else {
            evidence
                .files
                .remove(&key)
                .ok_or_else(|| LayoutError::FileOwnerMissing {
                    history_tree_id: key.history_tree_id.into(),
                    file_id: key.file_id.into(),
                })?
        };
        let internal = candidates
            .iter()
            .filter_map(|owner| match owner {
                LayoutOwner::Repository(directory) if included.contains(*directory) => {
                    Some(*directory)
                }
                LayoutOwner::Repository(_) | LayoutOwner::Unmanaged => None,
            })
            .collect::<Vec<_>>();
        if internal.is_empty() {
            outside_files.insert(
                key,
                OutsideFile {
                    owners: candidates,
                    versions,
                },
            );
            continue;
        }
        if internal.len() != candidates.len() {
            return Err(LayoutError::FileOwnerCrossesRoot {
                history_tree_id: key.history_tree_id.into(),
                file_id: key.file_id.into(),
                root_directory_id: root_directory_id.into(),
                directory_ids: candidates
                    .iter()
                    .filter_map(|owner| match owner {
                        LayoutOwner::Repository(directory) => Some((*directory).into()),
                        LayoutOwner::Unmanaged => None,
                    })
                    .collect(),
                unmanaged: candidates.contains(&LayoutOwner::Unmanaged),
            });
        }
        if internal.len() != 1 {
            return Err(LayoutError::FileOwnerAmbiguous {
                history_tree_id: key.history_tree_id.into(),
                file_id: key.file_id.into(),
                directory_ids: internal.into_iter().map(str::to_owned).collect(),
            });
        }
        let directory = internal[0];
        let layout = &repositories[directory];
        let (main, path) = match mains.remove(&key) {
            Some(main) => (Some(main.occurrence), main.path),
            None => (
                None,
                physical(
                    format!(
                        "{}/f-{}-{}",
                        extra.as_str(),
                        key.history_tree_id,
                        key.file_id
                    ),
                    &mut budget,
                )?,
            ),
        };
        let full = joined(layout.root.as_ref(), path.as_str(), &mut budget)?;
        joined(Some(&full), "container.json", &mut budget)?;
        for version in &versions {
            joined(
                Some(&full),
                &format!("versions/{}", version.storage_name),
                &mut budget,
            )?;
        }
        files.insert(
            key,
            FileLayout {
                directory_id: directory,
                path,
                main,
                versions,
            },
        );
    }
    for layout in repositories.values() {
        joined(layout.root.as_ref(), extra.as_str(), &mut budget)?;
        joined(
            layout.root.as_ref(),
            crate::container_storage::directory_database_name(),
            &mut budget,
        )?;
        for path in layout.directories.keys() {
            joined(layout.root.as_ref(), path, &mut budget)?;
        }
    }
    Ok(LayoutPlan {
        repository_id: &repository.repository_id,
        root_directory_id,
        state,
        repositories,
        files,
        outside_files,
    })
}

#[cfg(test)]
#[path = "../../../test/engine/unit/directory_layout/planner_tests.rs"]
mod tests;
