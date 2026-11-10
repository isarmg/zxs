//! One owner-aware membership projection from the current saved namespace.
//! The storage executor publishes this complete unit, never individual members.
use super::*;
use data_serialization::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::directory_git::DirectoryGitRepository;
use storage_contract::file_relocation::ObjectMoveRequest;
mod projection;
mod evidence;
pub(crate) use evidence::{admit_result, deletion_for, retained_for, verify_publication};
pub(crate) use projection::{plan, read_result, verify_transition, MovePlan, ResultBody};

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub(crate) struct NamespaceBasis {
    source_head: String,
    source_entries: Vec<Entry>,
    target_head: Option<String>,
    target_entries: Vec<Entry>,
}


#[derive(Serialize)]
pub(crate) struct MoveEntry {
    pub(crate) source: Entry,
    pub(crate) destination: name_policy::PortablePath,
    pub(crate) owner_directory_id: Option<String>,
    pub(crate) context: Option<FileContext>,
}

#[derive(Serialize)]
pub(crate) struct MoveDirectory {
    pub(crate) repository: DirectoryGitRepository,
    pub(crate) branch_ids: Vec<String>,
    pub(crate) tree_ids: Vec<String>,
    pub(crate) selected_branch: Option<String>,
    pub(crate) selected_tree: Option<String>,
    pub(crate) selection_generation: Option<u64>,
}

#[derive(Default, Serialize)]
pub(crate) struct MoveFile {
    pub(crate) version_ids: Vec<String>,
    pub(crate) branch_ids: Vec<String>,
    /// Saved inherited contents have no owned version until their first write.
    pub(crate) inherited_version_ids: BTreeSet<String>,
}

pub(crate) enum MoveTarget {
    Existing {
        repository: Repository,
        namespace: Vec<Entry>,
        owner_directory_id: Option<String>,
        displaced: Option<Entry>,
        displaced_entries: Vec<Entry>,
    },
    /// Create the root in the same candidate; this is not a separate mutation.
    EmptyDisk { slot_id: String },
}

pub(crate) struct MoveUnit {
    pub(crate) request: ObjectMoveRequest,
    pub(crate) source_repository: Repository,
    pub(crate) source_namespace: Vec<Entry>,
    pub(crate) target: MoveTarget,
    pub(crate) entries: Vec<MoveEntry>,
    pub(crate) directories: BTreeMap<String, MoveDirectory>,
    pub(crate) files: BTreeMap<String, MoveFile>,
    /// Current destination composition before final operation-bound ID mapping.
    pub(crate) target_entries: Vec<Entry>,
    copy_history: bool,
}

impl MoveUnit {
    /// Add an empty target root to the same unpublished State as the movement.
    /// No nested mutate, root-creation receipt, or separate visible publication.
    pub(crate) fn project_target_root(
        &self,
        state: &mut State,
        principal: &str,
        operation: OperationId,
    ) -> Result<Repository> {
        match &self.target {
            MoveTarget::Existing { repository, .. } => Ok(repository.clone()),
            MoveTarget::EmptyDisk { slot_id, .. } => {
                let name = storage_contract::RepositoryName::parse(&format!("ZXS disk {slot_id}"))?;
                repository::project_creation(state, repository::RepositoryCreation {
                    name: &name, primary: slot_id, copies: 1, file_manager_root: true,
                    principal, operation,
                }, repository::RepositoryIds::move_target(operation)?)
            }
        }
    }

    pub(crate) fn retained_version_ids(&self) -> BTreeSet<&str> {
        self.files.values().flat_map(|file| {
            file.version_ids.iter().chain(file.inherited_version_ids.iter()).map(String::as_str)
        }).collect()
    }
}

fn changed() -> Error {
    Error::new(ErrorCode::WorkspaceChanged, "the original move source or destination changed")
}

fn in_scope(entry: &Entry, root: &Entry) -> bool {
    entry.lookup_key() == root.lookup_key()
        || (root.kind == NamespaceEntryKind::Directory
            && namespace::is_descendant(entry.lookup_key(), root.lookup_key()))
}

fn target_scope(state: &State, request: &ObjectMoveRequest, source: &Entry, current: &[Entry]) -> Result<MoveTarget> {
    let Some(id) = request.target_repository_id.as_ref() else {
        if state.repositories.values().any(|repository| {
            repository.file_manager_root && repository.primary_slot_id == request.target_slot_id
        }) {
            return Err(changed());
        }
        if request.target_path.parent().is_some() {
            return Err(Error::new(ErrorCode::ParentNotDirectory,
                "an empty disk has no destination parent directory"));
        }
        return Ok(MoveTarget::EmptyDisk { slot_id: request.target_slot_id.clone() });
    };
    let repository = state.repositories.get(id.as_str()).ok_or_else(|| {
        Error::new(ErrorCode::RepositoryNotFound, "move destination repository missing")
    })?;
    if repository.primary_slot_id != request.target_slot_id {
        return Err(changed());
    }
    let namespace = current.to_vec();
    let tree = namespace::NamespaceTree::from_current_state(state, &repository.repository_id, &namespace);
    if tree.get_key(request.target_path.key()).is_none()
        && current.iter().any(|entry| entry.lookup_key()==request.target_path.key())
    {
        return Err(Error::new(ErrorCode::NameCollision,
            "move destination is reserved by a deleted object; restore or choose another name"));
    }
    let occupied = tree.get_key(request.target_path.key());
    let same_source = occupied.is_some_and(|entry| repository.repository_id == request.source_repository_id.as_str()
        && entry.entry_id == source.entry_id);
    let displaced = if same_source {
        // A case-only rename names the same object, even if the transport sent
        // its captured target identity. It must not delete its own history.
        if request.target_entry_id.as_ref().is_some_and(|id| id.as_str() != source.entry_id) {
            return Err(changed());
        }
        None
    } else {
        match (request.replace, request.target_entry_id.as_ref(), occupied) {
            (_, None, None) => {}
            (true, Some(expected), Some(actual)) if expected.as_str() == actual.entry_id => {}
            (false, _, Some(_)) => return Err(Error::new(ErrorCode::NameCollision,
                "move destination exists without replacement intent")),
            _ => return Err(changed()),
        }
        occupied
    };
    if let Some(actual) = displaced {
        if actual.kind != source.kind {
            return Err(Error::new(ErrorCode::InvalidMove,
                "replacement requires the same file or directory kind"));
        }
        if actual.kind == NamespaceEntryKind::Directory && current.iter().any(|entry|
            namespace::is_descendant(entry.lookup_key(), actual.lookup_key())) {
            return Err(Error::new(ErrorCode::DirectoryNotEmpty,
                "replacement requires an empty directory, including retained deleted children"));
        }
    }
    let parent_key = request.target_path.key().rsplit_once('/').map(|(parent, _)| parent);
    if parent_key.is_some_and(|key| {
        tree.get_key(key).is_none_or(|entry| entry.kind != NamespaceEntryKind::Directory)
    }) {
        return Err(Error::new(ErrorCode::ParentNotDirectory, "move destination parent missing"));
    }
    let owner_directory_id = tree.initialized_repository_id(parent_key.unwrap_or("")).map(str::to_owned);
    let displaced_entries = displaced.map(|root| {
        tree.visible_entries().filter(|entry| in_scope(entry, root)).cloned().collect()
    }).unwrap_or_default();
    let displaced = displaced.cloned();
    Ok(MoveTarget::Existing {
        repository: repository.clone(), namespace, owner_directory_id, displaced, displaced_entries,
    })
}

fn require_writable(engine: &Engine, repository: &Repository) -> Result<()> {
    if let Some(view) = engine.admitted_current_namespaces()? {
        view.require_branch_write(engine.state(), &repository.main_branch_id)?;
    } else if engine.state.branches[&repository.main_branch_id].state != BranchState::Active {
        return Err(Error::new(ErrorCode::BranchNotActive, "move namespace is not writable"));
    }
    Ok(())
}

fn current_entries(
    state: &State,
    request: &ObjectMoveRequest,
    snapshot: &[Entry],
) -> Result<Vec<MoveEntry>> {
    let tree = namespace::NamespaceTree::from_current_state(state, request.source_repository_id.as_str(), snapshot);
    let root = tree.get_key(request.source_path.key())
        .filter(|entry| entry.entry_id == request.source_entry_id.as_str())
        .ok_or_else(changed)?;
    if request.target_repository_id.as_ref() == Some(&request.source_repository_id)
        && (request.target_path.as_str() == root.path()
            || (root.kind == NamespaceEntryKind::Directory
                && namespace::is_descendant(request.target_path.key(), root.lookup_key())))
    {
        return Err(Error::new(ErrorCode::InvalidMove, "move destination is inside the source"));
    }
    let depth = root.path().split('/').count();
    snapshot.iter().filter(|entry| in_scope(entry, root)).map(|entry| {
        let destination = if entry.entry_id == root.entry_id {
            request.target_path.clone()
        } else {
            entry.location().rebase(depth, &request.target_path)?
        };
        let context = if entry.kind == NamespaceEntryKind::File {
            Some(state.file_contexts.get(&directory::context_key_from_key(
                request.source_repository_id.as_str(), entry.lookup_key())).cloned()
                .unwrap_or_else(|| directory::context_for_entry(state,
                    &state.repositories[request.source_repository_id.as_str()], entry)))
        } else { None };
        Ok(MoveEntry {
            source: entry.clone(), destination, context,
            owner_directory_id: tree.initialized_repository_id(entry.lookup_key()).map(str::to_owned),
        })
    }).collect()
}

fn arrival_entries(
    state: &State,
    request: &ObjectMoveRequest,
    target: &MoveTarget,
    entries: &[MoveEntry],
    maximum: usize,
) -> Result<Vec<Entry>> {
    let source_ids = entries.iter().map(|entry| entry.source.entry_id.as_str()).collect::<BTreeSet<_>>();
    let mut arrival = match target {
        MoveTarget::Existing { repository, namespace, displaced_entries, .. } => {
            let displaced = displaced_entries.iter().map(|entry| entry.entry_id.as_str())
                .collect::<BTreeSet<_>>();
            namespace::NamespaceTree::from_current_state(state, &repository.repository_id, namespace).visible_entries().filter(|entry| {
                !displaced.contains(entry.entry_id.as_str())
                    && (repository.repository_id != request.source_repository_id.as_str()
                        || !source_ids.contains(entry.entry_id.as_str()))
            }).cloned().collect::<Vec<_>>()
        }
        MoveTarget::EmptyDisk { .. } => Vec::new(),
    };
    if entries.len() > maximum.saturating_sub(arrival.len()) {
        return Err(Error::new(ErrorCode::ResourceLimit, "move exceeds destination entry budget"));
    }
    arrival.extend(entries.iter().map(|entry| {
        let mut arrived = entry.source.clone();
        arrived.relocate(entry.destination.clone());
        arrived
    }));
    namespace::validate_entries(&mut arrival, maximum)?;
    Ok(arrival)
}

fn directory_members(state: &State, entries: &[MoveEntry]) -> BTreeMap<String, MoveDirectory> {
    let mut branches = BTreeMap::<&str, Vec<&DirectoryBranch>>::new();
    let mut trees = BTreeMap::<&str, Vec<&DirectoryTree>>::new();
    for branch in state.directory_branches.values() {
        branches.entry(&branch.directory_id).or_default().push(branch);
    }
    for tree in state.directory_trees.values() {
        trees.entry(&tree.directory_branch_id).or_default().push(tree);
    }
    let mut pending = entries.iter().filter(|entry| entry.source.kind == NamespaceEntryKind::Directory)
        .map(|entry| entry.source.entry_id.clone()).collect::<BTreeSet<_>>();
    let mut directories = BTreeMap::new();
    while let Some(id) = pending.pop_first() {
        if directories.contains_key(&id) { continue; }
        let Some(repository) = state.directory_git_repositories.get(&id) else { continue; };
        let member_branches = branches.get(id.as_str()).cloned().unwrap_or_default();
        let member_trees = member_branches.iter().flat_map(|branch| {
            trees.get(branch.directory_branch_id.as_str()).into_iter().flatten().copied()
        }).collect::<Vec<_>>();
        // Frozen child pins can outlive the current child path. Their exact
        // histories belong to the parent, even when that child is not visible.
        for child in member_trees.iter().flat_map(|tree| tree.directory_refs.values()) {
            pending.insert(state.directory_branches[&child.directory_branch_id].directory_id.clone());
        }
        for child in repository.branches.values().flat_map(|branch| branch.index_children.values()) {
            pending.insert(state.directory_branches[&child.directory_branch_id].directory_id.clone());
        }
        directories.insert(id.clone(), MoveDirectory {
            repository:repository.clone(),
            branch_ids:member_branches.iter().map(|branch| branch.directory_branch_id.clone()).collect(),
            tree_ids:member_trees.iter().map(|tree| tree.tree_id.clone()).collect(),
            selected_branch:state.directory_selections.get(&id).cloned(),
            selected_tree:state.directory_tree_selections.get(&id).cloned(),
            selection_generation:state.directory_selection_generations.get(&id).copied(),
        });
    }
    directories
}

fn file_members(state: &State, entries: &[MoveEntry], directories: &BTreeMap<String, MoveDirectory>) -> BTreeMap<String, MoveFile> {
    let mut versions = BTreeMap::<&str, Vec<&FileVersion>>::new();
    let mut branches = BTreeMap::<&str, Vec<&Branch>>::new();
    for version in state.versions.values() { versions.entry(&version.file_id).or_default().push(version); }
    for branch in state.branches.values() {
        if let Some(file) = &branch.file_id { branches.entry(file).or_default().push(branch); }
    }
    let mut files = BTreeMap::<String, MoveFile>::new();
    let mut pending = BTreeSet::new();
    let mut inherited = BTreeMap::<String,BTreeSet<String>>::new();
    for entry in entries.iter().map(|entry| &entry.source).chain(
        directories.values().flat_map(|directory| directory.tree_ids.iter())
            .flat_map(|tree| state.directory_trees[tree].entries.iter()),
    ).filter(|entry| entry.kind == NamespaceEntryKind::File) {
        pending.insert(entry.file_id.clone());
        if state.versions[&entry.version_id].file_id != entry.file_id {
            inherited.entry(entry.file_id.clone()).or_default().insert(entry.version_id.clone());
        }
    }
    for member in directories.values() {
        for deletion in member.repository.deletions.values() {
            pending.extend(deletion.files.keys().cloned());
        }
    }
    while let Some(file) = pending.pop_first() {
        if files.contains_key(&file) { continue; }
        let owned = versions.get(file.as_str()).cloned().unwrap_or_default();
        let selected = branches.get(file.as_str()).cloned().unwrap_or_default();
        let mut dependencies = inherited.remove(&file).unwrap_or_default();
        dependencies.extend(selected.iter().flat_map(|branch| branch.pending_file_versions.values()).cloned());
        dependencies.extend(owned.iter().flat_map(|version| version.source_version_ids.iter()).cloned());
        for version in &dependencies {
            pending.insert(state.versions[version].file_id.clone());
        }
        files.insert(file, MoveFile {
            version_ids:owned.iter().map(|version| version.version_id.clone()).collect(),
            branch_ids:selected.iter().map(|branch| branch.branch_id.clone()).collect(),
            inherited_version_ids:dependencies,
        });
    }
    files
}

/// The same capture serves a new request and validation of its exact journal
/// candidate. It receives admitted current rows, never substitutes an old HEAD.
fn capture(state: &State, request: &ObjectMoveRequest, basis: &NamespaceBasis, maximum: usize) -> Result<MoveUnit> {
    request.require_target_intent()?;
    let source_repository = state.repositories.get(request.source_repository_id.as_str())
        .ok_or_else(|| Error::new(ErrorCode::RepositoryNotFound, "move source repository missing"))?;
    let entries = current_entries(state, request, &basis.source_entries)?;
    let source = entries.iter().find(|entry| entry.source.entry_id == request.source_entry_id.as_str())
        .expect("invariant: captured move includes its source identity");
    let target = target_scope(state, request, &source.source, &basis.target_entries)?;
    let target_entries = arrival_entries(state, request, &target, &entries, maximum)?;
    let directories = directory_members(state, &entries);
    let files = file_members(state, &entries, &directories);
    let same_repository = request.target_repository_id.as_ref() == Some(&request.source_repository_id);
    let copy_history = if same_repository {
        let mut ownership = directory_git::RenameOwnership::new(state,
            request.source_repository_id.as_str(), &basis.source_entries, &target_entries);
        for member in &entries {
            if member.source.kind == NamespaceEntryKind::File {
                let mut arrived = member.source.clone();
                arrived.relocate(member.destination.clone());
                ownership.track(&member.source, &arrived);
            }
        }
        ownership.requires_history_copy(state)?
    } else { true };
    // A deletion belongs to its initialized owner, not merely its old path.
    // Whole-owner moves carry the marker and its retained tree together. A
    // partial export must not expose the hidden rows under a different owner.
    let moved_ids=entries.iter().map(|entry|entry.source.entry_id.as_str()).collect::<BTreeSet<_>>();
    for owner in state.directory_git_repositories.values() {
        if owner.repository_id!=source_repository.repository_id || directories.contains_key(&owner.directory_id) {continue;}
        for node in owner.deletions.values().filter(|node|moved_ids.contains(node.directory_id.as_str())) {
            if copy_history || entries.iter().find(|entry|entry.source.entry_id==node.directory_id)
                .is_some_and(|entry| {
                    let tree=namespace::NamespaceTree::from_current_state(state,&source_repository.repository_id,&target_entries);
                    tree.initialized_repository_id(entry.destination.key())!=Some(owner.directory_id.as_str())
                }) {
                return Err(Error::new(ErrorCode::UnsupportedOperation,
                    "restore the deleted child before exporting it separately from its initialized owner"));
            }
        }
    }
    history_copy::retained::bounded(&(request, &entries, &directories, &files))?;
    Ok(MoveUnit {
        request: request.clone(), source_repository: source_repository.clone(),
        source_namespace: basis.source_entries.clone(), target, entries,
        directories, files, target_entries, copy_history,
    })
}

fn basis(state: &State, view: Option<&crate::engine::CurrentNamespaces>, request: &ObjectMoveRequest) -> Result<NamespaceBasis> {
    let source = state.repositories.get(request.source_repository_id.as_str())
        .ok_or_else(|| Error::new(ErrorCode::RepositoryNotFound, "move source repository missing"))?;
    let source_head = state.branches[&source.main_branch_id].head_commit_id.clone();
    let source_entries = directory_git::current_entries(state, view, &source.repository_id)?.to_vec();
    let (target_head, target_entries) = match &request.target_repository_id {
        Some(id) => {
            let target = state.repositories.get(id.as_str())
                .ok_or_else(|| Error::new(ErrorCode::RepositoryNotFound, "move destination repository missing"))?;
            (Some(state.branches[&target.main_branch_id].head_commit_id.clone()),
                directory_git::current_entries(state, view, id.as_str())?.to_vec())
        }
        None => (None, Vec::new()),
    };
    Ok(NamespaceBasis { source_head, source_entries, target_head, target_entries })
}
