//! Logical backup namespace validation stays global so paging cannot hide path
//! conflicts. Only lightweight nodes are retained; wire metadata is page-local.
use super::pagination::{PageRequest, PageSelector, SortKey};
use super::*;
use storage_engine::{Entry, FileVersion};

fn available_on(engine: &Engine, slot: &str, version: &str) -> bool {
    engine
        .state()
        .content_states
        .get(version)
        .is_none_or(|state| *state == storage_contract::ContentState::Available)
        && engine
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
        409,
        "BACKUP_PATH_CONFLICT",
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
        current: Option<(usize, &'a Entry)>,
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
                .map(|(_, entry)| entry.entry_id.clone())
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
            429,
            "RESOURCE_LIMIT_EXCEEDED",
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
    engine: &Engine,
) -> String {
    let relative = entry
        .map(|entry| entry.path.as_str())
        .or_else(|| {
            engine
                .state()
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
fn project(engine: &Engine, slot: &str, node: &Node<'_>) -> Result<Value, ApiError> {
    let display = node.display();
    let name = display.rsplit('/').next().unwrap_or(display);
    let mut entry = match node {
        Node::Directory { .. } => directory(name, display, &node.identity(slot)),
        Node::File {
            repository: repo,
            current,
            version,
            ..
        } => {
            // Resolve context against the actual current entry, never a frozen
            // copy with a substituted version. Historical content is overlaid
            // only after that validated projection.
            let mut value = if let Some((index, _)) = current {
                listed_current_entry(engine, repo, *index, "")?
            } else {
                json!({"entry_id":version.file_id,"kind":"file","repository_id":repo.repository_id,"repository_path":null,"container":true,"file_id":version.file_id,"selected_branch_id":null})
            };
            if !current.is_some_and(|(_, entry)| entry.version_id == version.version_id) {
                value["node_id"] = version.version_id.clone().into();
                value.as_object_mut().expect("file wire object").extend(
                    content_wire(engine, Some(&version.version_id), None)
                        .as_object()
                        .expect("content wire object")
                        .clone(),
                );
            }
            value["size"] = version.logical_size.to_string().into();
            value["version_id"] = version.version_id.clone().into();
            value["note"] = engine
                .state()
                .file_notes
                .get(&version.file_id)
                .cloned()
                .unwrap_or_default()
                .into();
            value["historical_only"] = current.is_none().into();
            value
        }
    };
    entry["path"] = display.into();
    entry["name"] = name.into();
    entry["read_only"] = true.into();
    entry["backup"] = true.into();
    Ok(entry)
}

pub(super) fn entries(
    engine: &Engine,
    slot: &str,
    path: &str,
    request: PageRequest,
) -> Result<Value, ApiError> {
    let repositories = engine
        .state()
        .repositories
        .values()
        .map(|repo| (repo.history_tree_id.as_str(), repo))
        .collect::<BTreeMap<_, _>>();
    let mut latest = BTreeMap::<&str, (&Repository, &FileVersion)>::new();
    for version in engine
        .state()
        .versions
        .values()
        .filter(|version| available_on(engine, slot, &version.version_id))
    {
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
    let mut canonical = BTreeMap::<&str, (usize, &Entry)>::new();
    for repo in repositories.values() {
        for (index, entry) in engine
            .list_tree_ref(&repo.main_branch_id)
            .map_err(error)?
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.kind == "file" && latest.contains_key(entry.file_id.as_str())
            })
        {
            let owner = engine
                .state()
                .file_locations
                .get(&entry.file_id)
                .is_some_and(|owner| name_policy::path_key(owner) == entry.lookup_key);
            if owner || !canonical.contains_key(entry.file_id.as_str()) {
                canonical.insert(&entry.file_id, (index, entry));
            }
        }
    }
    let maximum = engine.config().max_entries;
    let mut nodes = BTreeMap::new();
    for (file, (repo, newest)) in latest {
        let current = canonical.get(file).copied();
        let version = current
            .filter(|(_, entry)| available_on(engine, slot, &entry.version_id))
            .and_then(|(_, entry)| engine.state().versions.get(&entry.version_id))
            .unwrap_or(newest);
        let display = engine
            .state()
            .file_backup_paths
            .get(file)
            .and_then(|paths| paths.get(slot))
            .cloned()
            .unwrap_or_else(|| {
                default_backup_path(repo, current.map(|(_, entry)| entry), version, engine)
            });
        name_policy::validate_path(&display).map_err(ApiError::invalid)?;
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
            404,
            "DIRECTORY_NOT_FOUND",
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
        .map(|node| project(engine, slot, node))
        .collect::<Result<Vec<_>, _>>()?;
    let selected_entry = page
        .selected
        .map(|node| project(engine, slot, node))
        .transpose()?;
    let mut response = json!({"slot_id":slot,"path":path,"backup":true,"writable":false,"entries":values,"selected_entry":selected_entry,"context":null,"directory_file_count":file_count.to_string(),"directory_branches":[],"directory_branches_supported":false,"default_branch_scope":false});
    page.metadata(&mut response);
    Ok(response)
}
