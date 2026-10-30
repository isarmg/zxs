//! Logical backup namespace validation stays global so paging cannot hide path
//! conflicts. Only lightweight nodes are retained; wire metadata is page-local.
use super::pagination::{PageRequest, PageSelector};
use super::*;
use crate::backend_listing::SortKey;
use storage_engine::{Entry, FileVersion, PublishedReadView};

fn available_on(view: &PublishedReadView, slot: &str, version: &str) -> bool {
    view.state()
        .content_states
        .get(version)
        .is_none_or(|state| *state == storage_contract::ContentState::Available)
        && view
            .state()
            .placements
            .get(version)
            .is_some_and(|placement| {
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
#[derive(Clone)]
enum Node<'a> {
    Directory {
        display: String,
    },
    File {
        repository: &'a Repository,
        current: Option<&'a Entry>,
        version: &'a FileVersion,
        display: String,
    },
}
impl Node<'_> {
    fn display(&self) -> &str {
        match self {
            Self::Directory { display } | Self::File { display, .. } => display,
        }
    }
    fn directory(&self) -> bool {
        matches!(self, Self::Directory { .. })
    }
    fn identity(&self, slot: &str) -> String {
        match self {
            Self::Directory { display } => format!(
                "backupdir_{}",
                blake3::hash(format!("{slot}/{}", name_policy::path_key(display)).as_bytes())
                    .to_hex()
            ),
            Self::File {
                current, version, ..
            } => current
                .map(|entry| entry.entry_id.clone())
                .unwrap_or_else(|| version.file_id.clone()),
        }
    }
}
fn retain_node<'a>(
    nodes: &mut BTreeMap<String, Node<'a>>,
    value: Node<'a>,
    maximum: usize,
) -> Result<(), ApiError> {
    let key = name_policy::path_key(value.display());
    if let Some(existing) = nodes.get(&key) {
        match (existing, &value) {
            (Node::Directory { .. }, Node::Directory { .. }) => return Ok(()),
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
    view: &PublishedReadView,
) -> String {
    let relative = entry
        .map(|entry| entry.path.as_str())
        .or_else(|| {
            view.state()
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
fn project(view: &PublishedReadView, slot: &str, node: &Node<'_>) -> crate::ListingEntry {
    let display = node.display();
    let name = display
        .rsplit('/')
        .next()
        .expect("invariant: split has at least one element");
    let mut entry = match node {
        Node::Directory { .. } => directory(name, display, &node.identity(slot)),
        Node::File {
            repository: repo,
            current,
            version,
            ..
        } => {
            // Resolve against the current entry before overlaying historical content.
            let mut value = if let Some(current) = current {
                let mut entry = listed_current_entry(view, repo, current, "");
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
                            view.state(),
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
                        view.state()
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
            if current.is_some_and(|entry| entry.version_id != version.version_id) {
                value.node_id = Some(version.version_id.clone());
                value.version = crate::ListingVersion::File {
                    content: super::super::branch::capture_content(
                        view.state(),
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
    entry
}

/// Request-owned publication and its same-lock configuration limit; no authority.
pub(super) struct BackupListing {
    view: PublishedReadView,
    request: PageRequest,
    slot: String,
    path: String,
    maximum: usize,
}

impl BackupListing {
    pub(super) fn capture(
        view: PublishedReadView,
        request: PageRequest,
        slot: String,
        path: String,
        maximum: usize,
    ) -> Self {
        Self {
            view,
            request,
            slot,
            path,
            maximum,
        }
    }

    pub(super) fn read(
        self,
        control: &crate::HistoryReadControl,
    ) -> Result<crate::ListingSnapshot, ApiError> {
        entries(
            &self.view,
            &self.slot,
            &self.path,
            self.request,
            self.maximum,
            control,
        )
    }
}

fn entries(
    view: &PublishedReadView,
    slot: &str,
    path: &str,
    request: PageRequest,
    maximum: usize,
    control: &crate::HistoryReadControl,
) -> Result<crate::ListingSnapshot, ApiError> {
    activity::checkpoint(control)?;
    let observe = || control.checkpoint().map_err(storage_engine::Error::from);
    let repositories = view
        .state()
        .repositories
        .values()
        .map(|repo| {
            activity::checkpoint(control)?;
            Ok((repo.history_tree_id.as_str(), repo))
        })
        .collect::<Result<BTreeMap<_, _>, ApiError>>()?;
    let mut latest = BTreeMap::<&str, (&Repository, &FileVersion)>::new();
    for version in view.state().versions.values() {
        activity::checkpoint(control)?;
        if !available_on(view, slot, &version.version_id) {
            continue;
        }
        let Some(repo) = repositories.get(version.history_tree_id.as_str()) else {
            continue;
        };
        match latest.get(version.file_id.as_str()) {
            Some((_, previous))
                if (previous.version_sequence, &previous.version_id)
                    >= (version.version_sequence, &version.version_id) => {}
            _ => {
                latest.insert(&version.file_id, (repo, version));
            }
        }
    }
    // Stream each main head once, retaining only candidate file entries.
    let mut collected: Vec<Entry> = Vec::new();
    for repo in repositories.values() {
        activity::checkpoint(control)?;
        view.namespace_tree(&repo.main_branch_id, &observe)
            .map_err(activity::read_error)?
            .walk(|entry| {
                control.checkpoint()?;
                if entry.kind == storage_contract::NamespaceEntryKind::File
                    && latest.contains_key(entry.file_id.as_str())
                {
                    collected.push(entry.clone());
                }
                Ok(true)
            })
            .map_err(activity::read_error)?;
    }
    let mut canonical = BTreeMap::<&str, &Entry>::new();
    for entry in &collected {
        activity::checkpoint(control)?;
        let owner = view
            .state()
            .file_locations
            .get(&entry.file_id)
            .is_some_and(|owner| name_policy::path_key(owner) == entry.lookup_key());
        if owner || !canonical.contains_key(entry.file_id.as_str()) {
            canonical.insert(&entry.file_id, entry);
        }
    }
    let mut nodes = BTreeMap::new();
    for (file, (repo, newest)) in latest {
        activity::checkpoint(control)?;
        let current = canonical.get(file).copied();
        let version = current
            .filter(|entry| available_on(view, slot, &entry.version_id))
            .and_then(|entry| view.state().versions.get(&entry.version_id))
            .unwrap_or(newest);
        let display = view
            .state()
            .file_backup_paths
            .get(file)
            .and_then(|paths| paths.get(slot))
            .cloned()
            .unwrap_or_else(|| default_backup_path(repo, current, version, view));
        name_policy::validate_path(&display).map_err(ApiError::invalid_source)?;
        parents(&mut nodes, &display, maximum)?;
        retain_node(
            &mut nodes,
            Node::File {
                repository: repo,
                current,
                version,
                display,
            },
            maximum,
        )?;
    }
    let key = name_policy::path_key(path);
    if !path.is_empty() && !nodes.get(&key).is_some_and(Node::directory) {
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
    let mut file_count = 0;
    let mut page = PageSelector::new(request);
    for (key, node) in &nodes {
        activity::checkpoint(control)?;
        let Some(relative) = key.strip_prefix(&prefix) else {
            continue;
        };
        if !node.directory() {
            file_count += 1;
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
                &node.identity(slot),
            ),
            node,
        );
    }
    let page = page.finish()?;
    let values = page
        .values
        .iter()
        .map(|node| {
            activity::checkpoint(control)?;
            Ok::<_, ApiError>(project(view, slot, node))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let selected_entry = page
        .selected
        .map(|node| {
            activity::checkpoint(control)?;
            Ok::<_, ApiError>(project(view, slot, node))
        })
        .transpose()?;
    Ok(crate::ListingSnapshot {
        slot_id: slot.into(),
        path: path.into(),
        backup: true,
        entries: values,
        selected_entry,
        context: None,
        directory_file_count: file_count,
        directory_branches: Vec::new(),
        directory_branches_supported: false,
        default_branch_scope: false,
        total_entries: page.total,
        snapshot: page.snapshot,
        previous_cursor: page.previous,
        next_cursor: page.next,
    })
}
