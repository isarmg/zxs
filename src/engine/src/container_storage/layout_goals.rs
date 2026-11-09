//! Owned, pure primary and explicitly admitted Backup goals over a State.
//! Current homes are resolved by the pure planner before retained version routing.
//! Bindings describe exact metadata choices; they grant no registry or filesystem authority.
use super::layout_locator::{DirectoryId, FileAddress, HistoryRelative, LayoutBinding};
use crate::{
    Error, ErrorCode, Result,
    directory_layout::{self, FileLayout, LayoutFileKey, LayoutOwner, LayoutPlan},
};
use data_serialization::Serialize;
use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};
use storage_contract::{FileContext, FileVersion, Placement, Repository, State};

mod backup;

pub(super) const EXTRA_NAME: &str = ".zxs-retained-files";
const MAX_GOALS: usize = 1_000_000;
const MAX_GOAL_BYTES: usize = 16 * 1024 * 1024;
type HistoryKey = (String, String);
type FileKey = (String, String);

#[derive(Debug)]
pub(super) struct LayoutGoals {
    pub(super) state_sequence: u64,
    pub(super) histories: BTreeMap<HistoryKey, HistoryGoals>,
    /// Positive Unmanaged witnesses from the planned forests, never missing
    /// ownership. Other initialized forests are accounted for as managed goals.
    pub(super) outside_files: BTreeMap<FileKey, BTreeSet<String>>,
}

#[derive(Debug)]
pub(super) struct HistoryGoals {
    pub(super) slot_id: String,
    pub(super) history_id: String,
    pub(super) outer_repository_ids: BTreeSet<String>,
    pub(super) root_directory_ids: BTreeSet<DirectoryId>,
    pub(super) directories: BTreeMap<DirectoryId, DirectoryGoal>,
    pub(super) files: BTreeMap<String, FileGoal>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DirectoryGoal {
    pub(super) id: DirectoryId,
    pub(super) parent: Option<DirectoryId>,
    pub(super) name: String,
    pub(super) to: HistoryRelative,
    pub(super) binding: LayoutBinding,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FileGoal {
    pub(super) file_id: String,
    pub(super) root_directory_id: DirectoryId,
    pub(super) logical_owner_directory_id: DirectoryId,
    pub(super) to: FileAddress,
    pub(super) cache_name: String,
    /// Relative to this forest's fixed root, without directory-repositories/ID.
    pub(super) root_relative_path: HistoryRelative,
    pub(super) binding: LayoutBinding,
    pub(super) version_ids: BTreeSet<String>,
}

fn invalid(reason: &str, detail: String) -> Error {
    Error::new(
        ErrorCode::ContainerLayoutInvalid,
        format!("{reason}: {detail}"),
    )
}

#[derive(Default)]
struct Budget {
    goals: usize,
    bytes: usize,
}
impl Budget {
    fn charge(&mut self, bytes: usize) -> Result<()> {
        if self.goals == MAX_GOALS || bytes > MAX_GOAL_BYTES - self.bytes {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "directory layout goals exceed their metadata budget",
            ));
        }
        self.goals += 1;
        self.bytes += bytes;
        Ok(())
    }
}

fn join(parent: &HistoryRelative, path: &str) -> Result<HistoryRelative> {
    HistoryRelative::parse(&format!("{}/{path}", parent.as_str()))
}
fn basename(path: &HistoryRelative) -> &str {
    path.as_str()
        .rsplit('/')
        .next()
        .expect("invariant: a history-relative path has a leaf")
}
fn parent_path(path: &HistoryRelative) -> &str {
    path.as_str()
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .expect("invariant: a managed target is below directory-repositories")
}
#[derive(Clone, Copy)]
enum DirectoryRole {
    Repository,
    Ordinary,
    Retained,
}
fn directory_binding(
    role: DirectoryRole,
    main: &str,
    to: &HistoryRelative,
) -> Result<LayoutBinding> {
    let role = match role {
        DirectoryRole::Repository => "repository",
        DirectoryRole::Ordinary => "ordinary",
        DirectoryRole::Retained => "retained",
    };
    LayoutBinding::parse(&storage_contract::object_hash(
        "directory-layout-directory",
        &(role, main, to.as_str()),
    )?)
}

struct Skeleton {
    root: DirectoryId,
    prefix: HistoryRelative,
    directories: BTreeMap<DirectoryId, DirectoryGoal>,
    paths: BTreeMap<String, DirectoryId>,
}
impl Skeleton {
    fn new(plan: &LayoutPlan<'_>) -> Result<Self> {
        let root = DirectoryId::parse(plan.root_directory_id())?;
        let prefix = HistoryRelative::parse(&format!("directory-repositories/{}", root.as_str()))?;
        let mut skeleton = Self {
            root,
            prefix,
            directories: BTreeMap::new(),
            paths: BTreeMap::new(),
        };
        for layout in plan.repositories().values() {
            let id = DirectoryId::parse(layout.directory_id)?;
            let to = match &layout.root {
                Some(path) => join(&skeleton.prefix, path.as_str())?,
                None => skeleton.prefix.clone(),
            };
            skeleton.insert(
                id.clone(),
                to.clone(),
                DirectoryRole::Repository,
                &layout.main.directory_branch_id,
            )?;
            for (path, entry) in &layout.directories {
                skeleton.insert(
                    DirectoryId::parse(&entry.entry_id)?,
                    join(&to, path)?,
                    DirectoryRole::Ordinary,
                    &layout.main.directory_branch_id,
                )?;
            }
            skeleton.insert(
                DirectoryId::retained(&id),
                join(&to, layout.extra_directory.as_str())?,
                DirectoryRole::Retained,
                &layout.main.directory_branch_id,
            )?;
        }
        skeleton.attach_parents()?;
        Ok(skeleton)
    }

    fn insert(
        &mut self,
        id: DirectoryId,
        to: HistoryRelative,
        role: DirectoryRole,
        main: &str,
    ) -> Result<()> {
        let goal = DirectoryGoal {
            id: id.clone(),
            parent: None,
            name: basename(&to).to_owned(),
            binding: directory_binding(role, main, &to)?,
            to,
        };
        if let Some(previous) = self.directories.get(&id) {
            if previous != &goal {
                return Err(invalid(
                    "DIRECTORY_GOAL_ID_CONFLICT",
                    format!(
                        "{} targets both {} and {}",
                        id.as_str(),
                        previous.to.as_str(),
                        goal.to.as_str()
                    ),
                ));
            }
            return Ok(());
        }
        let key = name_policy::path_key(goal.to.as_str());
        if let Some(previous) = self.paths.get(&key) {
            return Err(invalid(
                "DIRECTORY_GOAL_TARGET_CONFLICT",
                format!(
                    "{} and {} target {}",
                    previous.as_str(),
                    id.as_str(),
                    goal.to.as_str()
                ),
            ));
        }
        self.paths.insert(key, id.clone());
        self.directories.insert(id, goal);
        Ok(())
    }

    fn parent_at(&self, path: &str) -> Result<DirectoryId> {
        let id = self
            .paths
            .get(&name_policy::path_key(path))
            .ok_or_else(|| {
                invalid(
                    "DIRECTORY_GOAL_PARENT_MISSING",
                    format!("no directory entity at {path}"),
                )
            })?;
        if self.directories[id].to.as_str() != path {
            return Err(invalid(
                "DIRECTORY_GOAL_PARENT_SPELLING_CONFLICT",
                path.to_owned(),
            ));
        }
        Ok(id.clone())
    }

    fn attach_parents(&mut self) -> Result<()> {
        let mut parents = BTreeMap::new();
        for (id, goal) in &self.directories {
            if id != &self.root {
                parents.insert(id.clone(), self.parent_at(parent_path(&goal.to))?);
            }
        }
        for (id, parent) in parents {
            self.directories
                .get_mut(&id)
                .expect("invariant: skeleton parent belongs to its directory")
                .parent = Some(parent);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Entity {
    Directory(DirectoryId),
    File(String),
}
struct HistoryBuild {
    goals: HistoryGoals,
    positions: BTreeMap<String, Entity>,
}
#[derive(Default)]
struct Builder {
    state_sequence: u64,
    histories: BTreeMap<HistoryKey, HistoryBuild>,
    outside_files: BTreeMap<FileKey, BTreeSet<String>>,
    budget: Budget,
}
impl Builder {
    fn history(&mut self, slot: &str, history: &str, outer: &str) -> Result<HistoryKey> {
        let key = (slot.to_owned(), history.to_owned());
        let group = match self.histories.entry(key.clone()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                name_policy::validate_slot(slot)?;
                storage_contract::validate_id(history, "history")?;
                self.budget.charge(slot.len() + history.len())?;
                entry.insert(HistoryBuild {
                    goals: HistoryGoals {
                        slot_id: slot.to_owned(),
                        history_id: history.to_owned(),
                        outer_repository_ids: BTreeSet::new(),
                        root_directory_ids: BTreeSet::new(),
                        directories: BTreeMap::new(),
                        files: BTreeMap::new(),
                    },
                    positions: BTreeMap::new(),
                })
            }
        };
        group.goals.outer_repository_ids.insert(outer.to_owned());
        Ok(key)
    }

    fn skeleton(
        &mut self,
        slot: &str,
        history: &str,
        outer: &str,
        skeleton: &Skeleton,
    ) -> Result<()> {
        let key = self.history(slot, history, outer)?;
        let group = self
            .histories
            .get_mut(&key)
            .expect("invariant: goal history was inserted");
        group.goals.root_directory_ids.insert(skeleton.root.clone());
        for (id, goal) in &skeleton.directories {
            if let Some(previous) = group.goals.directories.get(id) {
                if previous != goal {
                    return Err(invalid(
                        "DIRECTORY_GOAL_ID_CONFLICT",
                        format!(
                            "{slot}/{history}/{} targets {} and {}",
                            id.as_str(),
                            previous.to.as_str(),
                            goal.to.as_str()
                        ),
                    ));
                }
                continue;
            }
            let position = name_policy::path_key(goal.to.as_str());
            if let Some(previous) = group.positions.get(&position) {
                return Err(invalid(
                    "DIRECTORY_GOAL_TARGET_CONFLICT",
                    format!(
                        "{previous:?} and {} target {slot}/{history}/{}",
                        id.as_str(),
                        goal.to.as_str()
                    ),
                ));
            }
            self.budget
                .charge(id.as_str().len() + goal.to.as_str().len() + goal.name.len())?;
            group
                .positions
                .insert(position, Entity::Directory(id.clone()));
            group.goals.directories.insert(id.clone(), goal.clone());
        }
        Ok(())
    }

    fn file(&mut self, slot: &str, history: &str, outer: &str, goal: FileGoal) -> Result<()> {
        let key = self.history(slot, history, outer)?;
        let group = self
            .histories
            .get_mut(&key)
            .expect("invariant: goal history was inserted");
        if let Some(previous) = group.goals.files.get(&goal.file_id) {
            if previous != &goal {
                return Err(invalid(
                    "FILE_GOAL_ID_CONFLICT",
                    format!("{slot}/{history}/{} has different goals", goal.file_id),
                ));
            }
            return Ok(());
        }
        let FileAddress::Directory {
            directory_id,
            relative_path,
        } = &goal.to
        else {
            return Err(invalid("FILE_GOAL_ADDRESS_INVALID", goal.file_id));
        };
        let parent = group
            .goals
            .directories
            .get(directory_id)
            .expect("invariant: a file goal follows its complete skeleton");
        let to = join(&parent.to, relative_path.as_str())?;
        let position = name_policy::path_key(to.as_str());
        if let Some(previous) = group.positions.get(&position) {
            return Err(invalid(
                "FILE_GOAL_TARGET_CONFLICT",
                format!(
                    "{previous:?} and {} target {slot}/{history}/{}",
                    goal.file_id,
                    to.as_str()
                ),
            ));
        }
        self.budget.charge(goal.file_id.len() + to.as_str().len())?;
        for version in &goal.version_ids {
            self.budget.charge(version.len())?;
        }
        group
            .positions
            .insert(position, Entity::File(goal.file_id.clone()));
        group.goals.files.insert(goal.file_id.clone(), goal);
        Ok(())
    }

    fn outside(&mut self, key: LayoutFileKey<'_>, versions: &[&FileVersion]) -> Result<()> {
        let ids = self
            .outside_files
            .entry((key.history_tree_id.to_owned(), key.file_id.to_owned()))
            .or_default();
        for version in versions {
            if ids.insert(version.version_id.clone()) {
                self.budget.charge(version.version_id.len())?;
            }
        }
        Ok(())
    }

    fn finish(self) -> LayoutGoals {
        LayoutGoals {
            state_sequence: self.state_sequence,
            histories: self
                .histories
                .into_iter()
                .map(|(key, group)| (key, group.goals))
                .collect(),
            outside_files: self.outside_files,
        }
    }
}

#[derive(Serialize)]
struct MainProof<'a> {
    directory_branch_id: &'a str,
    entry: &'a storage_contract::Entry,
    context: Option<&'a FileContext>,
}
#[derive(Serialize)]
struct VersionProof<'a> {
    version: &'a FileVersion,
    placement: &'a Placement,
}
#[derive(Serialize)]
struct FileProof<'a> {
    slot_id: &'a str,
    history_id: &'a str,
    file_id: &'a str,
    root_directory_id: &'a str,
    logical_owner_directory_id: &'a str,
    parent_directory_id: &'a str,
    basename: &'a str,
    root_relative_path: &'a str,
    main: Option<MainProof<'a>>,
    versions: &'a [VersionProof<'a>],
}

fn primary_routes<'a>(
    state: &'a State,
    file: &FileLayout<'a>,
) -> Result<BTreeMap<&'a str, Vec<VersionProof<'a>>>> {
    let mut primary = BTreeMap::<&str, Vec<VersionProof<'_>>>::new();
    let mut backups = BTreeMap::<&str, &str>::new();
    for version in &file.versions {
        let placement = &state.placements[&version.version_id];
        primary
            .entry(&placement.primary_slot_id)
            .or_default()
            .push(VersionProof { version, placement });
        for slot in &placement.route_slot_ids {
            if slot != &placement.primary_slot_id {
                backups.entry(slot).or_insert(&version.version_id);
            }
        }
    }
    for slot in primary.keys() {
        let file_id = file.versions[0].file_id.as_str();
        if state
            .file_backup_paths
            .get(file_id)
            .is_some_and(|paths| paths.contains_key(*slot))
        {
            return Err(invalid(
                "DIRECTORY_LAYOUT_ROUTE_CONFLICT",
                format!("file {file_id} primary slot {slot} has an explicit backup path"),
            ));
        }
        if let Some(version) = backups.get(slot) {
            return Err(invalid(
                "DIRECTORY_LAYOUT_ROUTE_CONFLICT",
                format!("file {file_id} primary slot {slot} is also backup for version {version}"),
            ));
        }
    }
    Ok(primary)
}

fn file_goal(
    plan: &LayoutPlan<'_>,
    skeleton: &Skeleton,
    key: LayoutFileKey<'_>,
    file: &FileLayout<'_>,
    slot: &str,
    versions: &[VersionProof<'_>],
) -> Result<FileGoal> {
    let repository = &plan.repositories()[file.directory_id];
    let root_relative_path = match &repository.root {
        Some(root) => HistoryRelative::parse(&format!("{}/{}", root.as_str(), file.path.as_str()))?,
        None => HistoryRelative::parse(file.path.as_str())?,
    };
    let to = join(&skeleton.prefix, root_relative_path.as_str())?;
    let parent = skeleton.parent_at(parent_path(&to))?;
    let cache_name = basename(&to).to_owned();
    let owner = DirectoryId::parse(file.directory_id)?;
    let proof = FileProof {
        slot_id: slot,
        history_id: key.history_tree_id,
        file_id: key.file_id,
        root_directory_id: skeleton.root.as_str(),
        logical_owner_directory_id: owner.as_str(),
        parent_directory_id: parent.as_str(),
        basename: &cache_name,
        root_relative_path: root_relative_path.as_str(),
        main: file.main.as_ref().map(|occurrence| MainProof {
            directory_branch_id: occurrence.directory_branch_id,
            entry: occurrence.entry,
            context: occurrence.context,
        }),
        versions,
    };
    let binding = LayoutBinding::parse(&storage_contract::object_hash(
        "directory-layout-file",
        &proof,
    )?)?;
    Ok(FileGoal {
        file_id: key.file_id.to_owned(),
        root_directory_id: skeleton.root.clone(),
        logical_owner_directory_id: owner,
        to: FileAddress::Directory {
            directory_id: parent,
            relative_path: HistoryRelative::parse(&cache_name)?,
        },
        cache_name,
        root_relative_path,
        binding,
        version_ids: versions
            .iter()
            .map(|proof| proof.version.version_id.clone())
            .collect(),
    })
}

fn forest_roots<'a>(state: &'a State, outer: &str) -> Result<(BTreeSet<&'a str>, Vec<&'a str>)> {
    directory_layout::forest_roots(state, outer).map_err(Into::into)
}

#[derive(Default)]
struct Coverage {
    directories: BTreeSet<String>,
    managed: BTreeSet<FileKey>,
    expected_managed: BTreeSet<FileKey>,
}

fn add_tree(
    builder: &mut Builder,
    state: &State,
    repository: &Repository,
    plan: &LayoutPlan<'_>,
    coverage: &mut Coverage,
    targets: &[backup::Target<'_>],
) -> Result<()> {
    builder.state_sequence = plan.state_sequence();
    let skeleton = Skeleton::new(plan)?;
    builder.skeleton(
        &repository.primary_slot_id,
        &repository.history_tree_id,
        plan.repository_id(),
        &skeleton,
    )?;
    coverage
        .directories
        .extend(plan.repositories().keys().map(|id| (*id).to_owned()));
    for (key, outside) in plan.outside_files() {
        if outside
            .owners
            .iter()
            .any(|owner| matches!(owner, LayoutOwner::Repository(_)))
        {
            coverage
                .expected_managed
                .insert((key.history_tree_id.to_owned(), key.file_id.to_owned()));
        } else {
            builder.outside(*key, &outside.versions)?;
        }
    }
    for (key, file) in plan.files() {
        coverage
            .managed
            .insert((key.history_tree_id.to_owned(), key.file_id.to_owned()));
        for (slot, versions) in primary_routes(state, file)? {
            builder.skeleton(slot, key.history_tree_id, plan.repository_id(), &skeleton)?;
            builder.file(
                slot,
                key.history_tree_id,
                plan.repository_id(),
                file_goal(plan, &skeleton, *key, file, slot, &versions)?,
            )?;
        }
    }
    backup::add_tree(builder, state, repository, plan, targets)
}

fn add_outer(
    builder: &mut Builder,
    state: &State,
    repository: &Repository,
    targets: &[backup::Target<'_>],
) -> Result<()> {
    builder.history(
        &repository.primary_slot_id,
        &repository.history_tree_id,
        &repository.repository_id,
    )?;
    let (registered, roots) = forest_roots(state, &repository.repository_id)?;
    let mut coverage = Coverage::default();
    for root in roots {
        let plan = if root == repository.repository_id {
            directory_layout::plan(state, &repository.repository_id, EXTRA_NAME)?
        } else {
            directory_layout::plan_root(state, &repository.repository_id, root, EXTRA_NAME)?
        };
        builder.budget.charge(0)?;
        add_tree(builder, state, repository, &plan, &mut coverage, targets)?;
    }
    let unplaced = registered
        .iter()
        .filter(|id| !coverage.directories.contains(**id))
        .map(|id| (*id).to_owned())
        .collect::<Vec<_>>();
    if !unplaced.is_empty() {
        return Err(crate::DirectoryLayoutError::RepositoryCycle {
            directory_ids: unplaced,
        }
        .into());
    }
    if let Some(key) = coverage
        .expected_managed
        .difference(&coverage.managed)
        .next()
    {
        return Err(invalid(
            "INITIALIZED_FILE_GOAL_MISSING",
            format!("{}/{} has no forest goal", key.0, key.1),
        ));
    }
    for key in &coverage.managed {
        if builder.outside_files.contains_key(key) {
            return Err(invalid(
                "FILE_GOAL_SCOPE_CONFLICT",
                format!("{}/{} is both managed and unmanaged", key.0, key.1),
            ));
        }
    }
    Ok(())
}

/// Construct metadata goals only. None selects every outer repository for a
/// cold-final repair; source inode, availability, authorization, and publication
/// fences remain the filesystem producer's responsibilities.
pub(super) fn build(
    state: &State,
    selected_outer_ids: Option<&BTreeSet<String>>,
) -> Result<LayoutGoals> {
    build_with_targets(state, selected_outer_ids, &[])
}

/// Ordinary active-generation maintenance only. The generation supplies the
/// original native Backup target selection; State supplies the current logical
/// forest and each retained version's actual route. No other backup is inferred.
pub(super) fn build_for_config(
    config: &crate::ValidatedConfig,
    state: &State,
    selected_outer_ids: Option<&BTreeSet<String>>,
) -> Result<LayoutGoals> {
    let targets = backup::selected(config)?;
    build_with_targets(state, selected_outer_ids, &targets)
}

fn build_with_targets(
    state: &State,
    selected_outer_ids: Option<&BTreeSet<String>>,
    targets: &[backup::Target<'_>],
) -> Result<LayoutGoals> {
    if let Some(selected) = selected_outer_ids {
        for outer in selected {
            storage_contract::validate_id(outer, "repo")?;
            if !state.repositories.contains_key(outer) {
                return Err(crate::DirectoryLayoutError::RepositoryMissing {
                    repository_id: outer.clone(),
                }
                .into());
            }
        }
    }
    let mut builder = Builder {
        state_sequence: state.sequence,
        ..Builder::default()
    };
    for repository in state.repositories.values() {
        if selected_outer_ids.is_none_or(|selected| selected.contains(&repository.repository_id)) {
            add_outer(&mut builder, state, repository, targets)?;
        }
    }
    Ok(builder.finish())
}
