//! Logical backup namespace validation stays global so paging cannot hide path
//! conflicts. Immutable metadata owns the scan; wire metadata is page-local.
use super::pagination::{Page, PageRequest, PageSelector};
use super::*;
use crate::backend_listing::SortKey;
use storage_contract::State;
use storage_engine::{Entry, FileVersion, PublishedBackupDirectory};

fn available_on(state: &BackupListingMetadata, slot: &str, version: &str) -> bool {
    state
        .content_states
        .get(version)
        .is_none_or(|state| *state == storage_contract::ContentState::Available)
        && state.placements.get(version).is_some_and(|placement| {
            placement
                .route_slot_ids
                .iter()
                .any(|candidate| candidate == slot)
        })
}
fn conflict() -> ApiError {
    ApiError::new(
        ::http::StatusCode::CONFLICT,
        storage_contract::ErrorCode::BackupPathConflict,
        "authorized backup containers have conflicting logical paths; choose distinct backup prefixes",
    )
}
fn directory_identity(slot: &str, key: &str) -> String {
    format!(
        "backupdir_{}",
        blake3::hash(format!("{slot}/{key}").as_bytes()).to_hex()
    )
}

enum Node<'a> {
    Directory {
        display: String,
        published: Option<&'a PublishedBackupDirectory>,
    },
    File {
        repository: &'a Repository,
        current: Option<(usize, &'a Entry)>,
        version: &'a FileVersion,
        display: String,
    },
}
impl Node<'_> {
    fn display(&self) -> &str {
        match self {
            Self::Directory { display, .. } | Self::File { display, .. } => display,
        }
    }
    fn directory(&self) -> bool {
        matches!(self, Self::Directory { .. })
    }
    fn identity(&self, slot: &str, key: &str) -> String {
        match self {
            Self::Directory { published, .. } => published
                .map(|row| row.directory_id().to_owned())
                .unwrap_or_else(|| directory_identity(slot, key)),
            Self::File {
                current, version, ..
            } => current
                .map(|(_, entry)| entry.entry_id.clone())
                .unwrap_or_else(|| version.file_id.clone()),
        }
    }
    fn candidate(&self, slot: &str) -> Candidate {
        match self {
            Self::Directory { display, .. } => Candidate::Directory {
                display: display.clone(),
                identity: self.identity(slot, &name_policy::path_key(display)),
            },
            Self::File {
                repository,
                current,
                version,
                display,
            } => Candidate::File {
                repository_id: repository.repository_id.clone(),
                current: current.map(|(index, _)| index),
                version_id: version.version_id.clone(),
                display: display.clone(),
            },
        }
    }
}

enum Candidate {
    Directory {
        display: String,
        identity: String,
    },
    File {
        repository_id: String,
        current: Option<usize>,
        version_id: String,
        display: String,
    },
}
impl Candidate {
    fn display(&self) -> &str {
        match self {
            Self::Directory { display, .. } | Self::File { display, .. } => display,
        }
    }
}
fn retain_node<'a>(
    nodes: &mut BTreeMap<String, Node<'a>>,
    value: Node<'a>,
    maximum: usize,
) -> Result<(), ApiError> {
    let key = name_policy::path_key(value.display());
    if let Some(existing) = nodes.get_mut(&key) {
        match (&mut *existing, &value) {
            (Node::Directory { published: old, .. }, Node::Directory { published: new, .. }) => {
                if let (Some(a), Some(b)) = (*old, *new)
                    && (a.repository_id(), a.history_tree_id(), a.directory_id())
                        != (b.repository_id(), b.history_tree_id(), b.directory_id())
                {
                    return Err(conflict());
                }
                if old.is_none() && new.is_some() {
                    *existing = value;
                }
                return Ok(());
            }
            (
                Node::File {
                    repository: a,
                    version: av,
                    ..
                },
                Node::File {
                    repository: b,
                    version: bv,
                    ..
                },
            ) if a.repository_id == b.repository_id && av.file_id == bv.file_id => return Ok(()),
            _ => return Err(conflict()),
        }
    }
    if nodes.len() >= maximum {
        return Err(ApiError::new(
            ::http::StatusCode::TOO_MANY_REQUESTS,
            storage_contract::ErrorCode::ResourceLimitExceeded,
            "backup namespace exceeds the bounded entry limit",
        ));
    }
    nodes.insert(key, value);
    Ok(())
}
fn parents(
    nodes: &mut BTreeMap<String, Node<'_>>,
    path: &str,
    maximum: usize,
) -> Result<(), ApiError> {
    for (index, _) in path.match_indices('/') {
        retain_node(
            nodes,
            Node::Directory {
                display: path[..index].into(),
                published: None,
            },
            maximum,
        )?;
    }
    Ok(())
}
fn default_backup_path(
    repo: &Repository,
    entry: Option<&Entry>,
    version: &FileVersion,
    state: &BackupListingMetadata,
) -> String {
    let relative = entry
        .map(|entry| entry.path.as_str())
        .or_else(|| {
            state
                .file_locations
                .get(&version.file_id)
                .map(String::as_str)
        })
        .unwrap_or(&version.creation_name);
    if repo.file_manager_root {
        format!("{}/{relative}", repo.primary_slot_id)
    } else {
        format!("{}/{}/{relative}", repo.primary_slot_id, repo.name)
    }
}
fn project(
    engine: &Engine,
    scope: &SelectedScope,
    node: &Candidate,
) -> Result<crate::ListingEntry, ApiError> {
    let display = node.display();
    let name = display
        .rsplit('/')
        .next()
        .expect("invariant: split has at least one element");
    let mut entry = match node {
        Candidate::Directory { identity, .. } => directory(name, display, identity),
        Candidate::File {
            repository_id,
            current,
            version_id,
            ..
        } => {
            let state = scope;
            let repo = &state.repositories[repository_id];
            let version = &state.versions[version_id];
            let namespace = engine.namespace_tree(&repo.main_branch_id).map_err(error)?;
            let current = current.map(|index| (index, &namespace.entries()[index]));
            // Resolve against the current entry before overlaying historical content.
            let mut value = if let Some((index, _)) = current {
                let mut entry = listed_current_entry(engine, repo, index, "")?;
                entry.path = display.into();
                entry.name = name.into();
                entry
            } else {
                crate::ListingEntry {
                    entry_id: version.file_id.clone(),
                    name: name.into(),
                    path: display.into(),
                    kind: storage_contract::NamespaceEntryKind::File,
                    size: version.logical_size,
                    file_id: version.file_id.clone(),
                    version: crate::ListingVersion::File {
                        content: super::super::branch::capture_content(
                            engine.state(),
                            Some(&version.version_id),
                            None,
                        ),
                        backup_version: None,
                    },
                    location: Some(crate::ListingLocation {
                        repository_id: repo.repository_id.clone(),
                        repository_path: None,
                        container: true,
                        selected_branch_id: None,
                    }),
                    note: Some(
                        engine
                            .state()
                            .file_notes
                            .get(&version.file_id)
                            .cloned()
                            .unwrap_or_default(),
                    ),
                    node_id: Some(version.version_id.clone()),
                    read_only: None,
                    backup: false,
                    historical_only: None,
                }
            };
            if current.is_some_and(|(_, entry)| entry.version_id != version.version_id) {
                value.node_id = Some(version.version_id.clone());
                value.version = crate::ListingVersion::File {
                    content: super::super::branch::capture_content(
                        engine.state(),
                        Some(&version.version_id),
                        None,
                    ),
                    backup_version: None,
                };
            }
            let crate::ListingVersion::File { backup_version, .. } = &mut value.version else {
                unreachable!("invariant: backup file candidates project file content");
            };
            *backup_version = Some(version.version_id.clone());
            value.size = version.logical_size;
            value.historical_only = Some(current.is_none());
            value
        }
    };
    entry.read_only = Some(true);
    entry.backup = true;
    Ok(entry)
}

struct NamespaceCapture {
    repository_id: String,
    namespace: storage_engine::NamespaceSnapshot,
}

/// Exactly the tables used by backup namespace validation and page selection.
/// Current namespace heads are held separately; unrelated historical commits,
/// directory trees and authority records are never captured here.
struct BackupListingMetadata {
    #[cfg(test)]
    sequence: u64,
    repositories: storage_contract::TrackedMap<Repository>,
    versions: storage_contract::TrackedRecords<FileVersion>,
    placements: storage_contract::TrackedMap<storage_contract::Placement>,
    content_states: storage_contract::TrackedMap<storage_contract::ContentState>,
    file_locations: storage_contract::TrackedMap<String>,
    file_backup_paths: storage_contract::TrackedMap<BTreeMap<String, String>>,
}

impl BackupListingMetadata {
    fn capture(state: &State) -> Self {
        Self {
            #[cfg(test)]
            sequence: state.sequence,
            repositories: state.repositories.clone(),
            versions: state.versions.clone(),
            placements: state.placements.clone(),
            content_states: state.content_states.clone(),
            file_locations: state.file_locations.clone(),
            file_backup_paths: state.file_backup_paths.clone(),
        }
    }
}

struct ListingScope {
    metadata: BackupListingMetadata,
    slot_id: String,
    path: String,
    maximum: usize,
    namespaces: BTreeMap<String, NamespaceCapture>,
    directories: Vec<PublishedBackupDirectory>,
}

pub(in crate::engine_backend) struct BackupListingRead {
    scope: ListingScope,
    request: PageRequest,
}

struct NamespaceWitness {
    repository_id: String,
    namespace: storage_engine::NamespaceSnapshotWitness,
}

// Global identity evidence stays small per repository; only selected versions
// remain strongly owned after the full namespace conflict scan has completed.
struct SelectedScope {
    repositories: storage_contract::TrackedMap<Repository>,
    versions: BTreeMap<String, Arc<FileVersion>>,
    namespaces: BTreeMap<String, NamespaceWitness>,
    slot_id: String,
    path: String,
}

pub(in crate::engine_backend) struct SelectedListing {
    scope: SelectedScope,
    page: Page<Candidate>,
    directory_file_count: usize,
}

pub(in crate::engine_backend) struct ProjectedEntries {
    entries: Vec<crate::ListingEntry>,
    selected_entry: Option<crate::ListingEntry>,
}

impl BackupListingRead {
    pub(super) fn capture(
        engine: &AdmissionEngine,
        slot: &str,
        path: &str,
        request: PageRequest,
    ) -> Result<Self, ApiError> {
        let state = engine.state();
        let repositories = state
            .repositories
            .values()
            .map(|repo| (repo.history_tree_id.as_str(), repo))
            .collect::<BTreeMap<_, _>>();
        let mut namespaces = BTreeMap::new();
        for (history, repo) in repositories {
            let namespace = engine
                .namespace_snapshot(&repo.main_branch_id)
                .map_err(error)?;
            namespaces.insert(
                history.into(),
                NamespaceCapture {
                    repository_id: repo.repository_id.clone(),
                    namespace,
                },
            );
        }
        Ok(Self {
            scope: ListingScope {
                metadata: BackupListingMetadata::capture(engine.state()),
                slot_id: slot.into(),
                path: path.into(),
                maximum: engine.config().max_entries,
                namespaces,
                directories: engine.published_backup_directories(slot).map_err(error)?,
            },
            request,
        })
    }

    pub(in crate::engine_backend) fn select(self) -> Result<SelectedListing, ApiError> {
        #[cfg(test)]
        lifecycle_tests::before_scan(&self.scope.metadata);
        let Self { scope, request } = self;
        let nodes = scope.nodes()?;
        let key = name_policy::path_key(&scope.path);
        if !scope.path.is_empty() && !nodes.get(&key).is_some_and(Node::directory) {
            return Err(ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::DirectoryNotFound,
                "backup directory not found",
            ));
        }
        let prefix = if key.is_empty() {
            String::new()
        } else {
            format!("{key}/")
        };
        let mut directory_file_count = 0;
        let mut page = PageSelector::new(request);
        for (key, node) in &nodes {
            let Some(relative) = key.strip_prefix(&prefix) else {
                continue;
            };
            if !node.directory() {
                directory_file_count += 1;
            }
            if relative.contains('/') {
                continue;
            }
            let display = node.display();
            page.consider(
                SortKey::new(
                    node.directory(),
                    display.rsplit('/').next().unwrap_or(display),
                    display,
                    &node.identity(&scope.slot_id, key),
                ),
                node,
            );
        }
        let page = page.finish()?;
        let page = Page {
            values: page
                .values
                .into_iter()
                .map(|node| node.candidate(&scope.slot_id))
                .collect(),
            selected: page.selected.map(|node| node.candidate(&scope.slot_id)),
            total: page.total,
            snapshot: page.snapshot,
            previous: page.previous,
            next: page.next,
        };
        // The bounded page owns only identities, indices and display paths.
        // Release the global graph and all scan-only tables before reacquiring
        // Engine for current identity and namespace-pointer checks.
        drop(nodes);
        let scope = scope.retain_selected(&page);
        Ok(SelectedListing {
            scope,
            page,
            directory_file_count,
        })
    }
}

impl ListingScope {
    fn retain_selected(self, page: &Page<Candidate>) -> SelectedScope {
        let versions = page
            .values
            .iter()
            .chain(page.selected.iter())
            .filter_map(|candidate| match candidate {
                Candidate::Directory { .. } => None,
                Candidate::File { version_id, .. } => Some((
                    version_id.clone(),
                    self.metadata
                        .versions
                        .get_shared(version_id)
                        .expect("invariant: selected backup version was validated"),
                )),
            })
            .collect();
        let namespaces = self
            .namespaces
            .into_iter()
            .map(|(history, namespace)| {
                (
                    history,
                    NamespaceWitness {
                        repository_id: namespace.repository_id,
                        namespace: namespace.namespace.witness(),
                    },
                )
            })
            .collect();
        SelectedScope {
            repositories: self.metadata.repositories,
            versions,
            namespaces,
            slot_id: self.slot_id,
            path: self.path,
        }
    }

    fn latest(&self) -> BTreeMap<&str, (&Repository, &FileVersion)> {
        let state = &self.metadata;
        let mut latest = BTreeMap::<&str, (&Repository, &FileVersion)>::new();
        for version in state
            .versions
            .values()
            .filter(|version| available_on(state, &self.slot_id, &version.version_id))
        {
            let Some(namespace) = self.namespaces.get(&version.history_tree_id) else {
                continue;
            };
            let repo = &state.repositories[&namespace.repository_id];
            match latest.get(version.file_id.as_str()) {
                Some((_, previous))
                    if (previous.version_sequence, &previous.version_id)
                        >= (version.version_sequence, &version.version_id) => {}
                _ => {
                    latest.insert(&version.file_id, (repo, version));
                }
            }
        }
        latest
    }

    fn canonical<'a>(
        &'a self,
        latest: &BTreeMap<&str, (&Repository, &FileVersion)>,
    ) -> BTreeMap<&'a str, (usize, &'a Entry)> {
        let state = &self.metadata;
        let mut canonical = BTreeMap::new();
        for namespace in self.namespaces.values() {
            for (index, entry) in namespace.namespace.entries().iter().enumerate() {
                if entry.kind != storage_contract::NamespaceEntryKind::File
                    || !latest.contains_key(entry.file_id.as_str())
                {
                    continue;
                }
                let owner = state
                    .file_locations
                    .get(&entry.file_id)
                    .is_some_and(|owner| name_policy::path_key(owner) == entry.lookup_key());
                if owner || !canonical.contains_key(entry.file_id.as_str()) {
                    canonical.insert(entry.file_id.as_str(), (index, entry));
                }
            }
        }
        canonical
    }

    fn nodes(&self) -> Result<BTreeMap<String, Node<'_>>, ApiError> {
        let state = &self.metadata;
        let latest = self.latest();
        let canonical = self.canonical(&latest);
        let mut nodes = BTreeMap::new();
        for row in &self.directories {
            if !self
                .namespaces
                .get(row.history_tree_id())
                .is_some_and(|scope| scope.repository_id == row.repository_id())
            {
                continue;
            }
            let path = row.path().as_str();
            parents(&mut nodes, path, self.maximum)?;
            retain_node(
                &mut nodes,
                Node::Directory {
                    display: path.to_owned(),
                    published: Some(row),
                },
                self.maximum,
            )?;
        }
        for (file, (repo, newest)) in latest {
            let current = canonical.get(file).copied();
            let version = current
                .filter(|(_, entry)| available_on(state, &self.slot_id, &entry.version_id))
                .and_then(|(_, entry)| state.versions.get(&entry.version_id))
                .unwrap_or(newest);
            let display = state
                .file_backup_paths
                .get(file)
                .and_then(|paths| paths.get(&self.slot_id))
                .cloned()
                .unwrap_or_else(|| {
                    default_backup_path(repo, current.map(|(_, entry)| entry), version, state)
                });
            name_policy::validate_path(&display).map_err(ApiError::invalid_source)?;
            parents(&mut nodes, &display, self.maximum)?;
            retain_node(
                &mut nodes,
                Node::File {
                    repository: repo,
                    current,
                    version,
                    display,
                },
                self.maximum,
            )?;
        }
        Ok(nodes)
    }
}

fn changed() -> ApiError {
    ApiError::conflict(
        storage_contract::ErrorCode::ListingChanged,
        "directory listing changed; restart from the first page",
    )
}

impl SelectedListing {
    fn authorize_current(&self, engine: &AdmissionEngine) -> Result<(), ApiError> {
        let captured = &self.scope;
        let current = engine.state();
        if engine.listing_snapshot() != self.page.snapshot
            || current.repositories.len() != captured.repositories.len()
        {
            return Err(changed());
        }
        disk(engine, &captured.slot_id)?;
        // Every namespace participated in the global conflict proof, even if it
        // contributed no visible row to this page or the requested directory.
        for namespace in self.scope.namespaces.values() {
            let repo = &captured.repositories[&namespace.repository_id];
            if current.repositories.get(&namespace.repository_id) != Some(repo) {
                return Err(changed());
            }
            if !engine.namespace_snapshot_matches(&namespace.namespace) {
                return Err(changed());
            }
        }
        Ok(())
    }

    /// The caller has refreshed current identity under this same engine lock.
    pub(in crate::engine_backend) fn project(
        &self,
        engine: &AdmissionEngine,
    ) -> Result<ProjectedEntries, ApiError> {
        self.authorize_current(engine)?;
        let entries = self
            .page
            .values
            .iter()
            .map(|node| project(engine, &self.scope, node))
            .collect::<Result<Vec<_>, _>>()?;
        let selected_entry = self
            .page
            .selected
            .as_ref()
            .map(|node| project(engine, &self.scope, node))
            .transpose()?;
        Ok(ProjectedEntries {
            entries,
            selected_entry,
        })
    }

    /// Consume the page and release selected records and identity evidence after unlocking.
    pub(in crate::engine_backend) fn finish(
        self,
        page: ProjectedEntries,
    ) -> crate::ListingSnapshot {
        crate::ListingSnapshot {
            slot_id: self.scope.slot_id,
            path: self.scope.path,
            backup: true,
            preview: false,
            entries: page.entries,
            selected_entry: page.selected_entry,
            context: None,
            directory_file_count: self.directory_file_count,
            directory_branches: Vec::new(),
            directory_branches_supported: false,
            default_branch_scope: false,
            total_entries: self.page.total,
            snapshot: self.page.snapshot,
            previous_cursor: self.page.previous,
            next_cursor: self.page.next,
        }
    }
}

#[cfg(test)]
#[path = "../../../../../test/server/unit/engine_backend/file_manager/backup_namespace_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../../test/server/unit/engine_backend/file_manager/backup_listing_lifecycle_tests.rs"]
mod lifecycle_tests;

#[cfg(test)]
#[path = "../../../../../test/server/unit/engine_backend/file_manager/backup_listing_entry_tests.rs"]
mod entry_tests;
