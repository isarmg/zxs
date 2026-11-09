//! Pure payload authority for current-registry recovery, before any body I/O.
use super::*;
use crate::engine::disk_registry::current::CurrentNamespaces;

pub(super) fn verify(
    view: &CurrentNamespaces,
    advanced: &CurrentNamespaces,
    previous: &State,
    next: &State,
    candidate: &Candidate,
) -> Result<()> {
    verify_fresh_payload_delta(view, previous, next, candidate)?;
    verify_payload_scopes(view, advanced, previous, next, candidate)
}

// The admitted current payload producers (uploads and copied versions) create
// fresh immutable versions through build_version. Bind that complete State
// delta, not a caller-chosen operation label or a list in its result. Existing
// versions/placements cannot acquire I/O authority from a no-op receipt. A future
// producer of historical copies needs its own complete, explicit proof branch.
fn verify_fresh_payload_delta(
    view: &CurrentNamespaces,
    previous: &State,
    next: &State,
    candidate: &Candidate,
) -> Result<()> {
    let invalid = || {
        Error::new(
            ErrorCode::PublicationInvalid,
            "current-registry candidate payloads differ from the exact fresh version and placement delta",
        )
    };
    if candidate.registry() != Some(view.registry())
        || previous
            .versions
            .len()
            .checked_add(candidate.versions.len())
            != Some(next.versions.len())
        || previous
            .placements
            .len()
            .checked_add(candidate.versions.len())
            != Some(next.placements.len())
        || previous
            .versions
            .iter()
            .any(|(id, version)| next.versions.get(id) != Some(version))
        || previous
            .placements
            .iter()
            .any(|(id, placement)| next.placements.get(id) != Some(placement))
    {
        return Err(invalid());
    }
    let mut seen = BTreeSet::new();
    for evidence in &candidate.versions {
        let id = &evidence.version.version_id;
        if !seen.insert(id)
            || previous.versions.contains_key(id)
            || previous.placements.contains_key(id)
            || evidence.version.version_sequence != candidate.sequence
            || evidence.placement.version_id != *id
            || next.versions.get(id) != Some(&evidence.version)
            || next.placements.get(id) != Some(&evidence.placement)
            || next.content_states.get(id) != Some(&storage_contract::ContentState::Available)
        {
            return Err(invalid());
        }
        view.verify_backup_payload_route(previous, next, &evidence.version, &evidence.placement)?;
        for slot in &evidence.placement.route_slot_ids {
            view.require_write_slot(slot).map_err(|source| {
                source.context(
                    ErrorCode::PublicationInvalid,
                    "current candidate payload route is not admitted for writes",
                )
            })?;
        }
    }
    Ok(())
}

// A fresh object alone is not a content write: a receipt-only operation could
// invent an otherwise well-formed orphan version. Each payload must belong to
// the same actor/operation's actual, newly advanced writable namespace. These
// are the two existing current-generation producers; unknown producers fail
// closed until they supply a complete dedicated payload admission.
fn verify_payload_scopes(
    source: &CurrentNamespaces,
    advanced: &CurrentNamespaces,
    previous: &State,
    next: &State,
    candidate: &Candidate,
) -> Result<()> {
    if candidate.versions.is_empty() {
        return Ok(());
    }
    let invalid = || {
        Error::new(
            ErrorCode::PublicationInvalid,
            "current-registry payload has no matching native content publication scope",
        )
    };
    let result = next
        .operations
        .get(&candidate.operation_id)
        .ok_or_else(invalid)?;
    let principal = next
        .operation_principals
        .get(&candidate.operation_id)
        .ok_or_else(invalid)?;
    Engine::require_actor(principal)?;
    if previous.operations.contains_key(&candidate.operation_id)
        || previous
            .operation_principals
            .contains_key(&candidate.operation_id)
        || !next.events.last().is_some_and(|event| {
            event.operation_id == candidate.operation_id
                && event.sequence == candidate.sequence
                && event.kind == result.kind
        })
    {
        return Err(invalid());
    }
    let workspace = result.kind == "workspace.commit";
    let copied = result.kind == "directory.namespace.batch"
        && (result.result["action"] == json_codec::json!(DirectoryBatchKind::Copy)
            || result.result["action"] == json_codec::json!(DirectoryBatchKind::Separate));
    if !workspace && !copied {
        return Err(invalid());
    }
    let copy_plan = if copied {
        let plan = verify_copied_payload_request(source, previous, candidate, result, principal)?;
        if let Some(retained) = &plan.retained {
            if previous.directory_git_repositories != next.directory_git_repositories {
                return Err(invalid_copy());
            }
            let branch = next
                .directory_branches
                .get(&retained.directory_branch_id)
                .ok_or_else(invalid_copy)?;
            let entries =
                advanced.directory_entries(&retained.directory_branch_id, &branch.head_tree_id)?;
            verify_retained_file(branch, entries, retained)?;
        }
        verify_copy_targets(source, advanced, next, candidate, &plan)?;
        Some(plan)
    } else {
        None
    };
    let native_commit = |commit: &Commit| {
        !previous.commits.contains_key(&commit.commit_id)
            && commit.operation_id == candidate.operation_id
            && commit.sequence == candidate.sequence
            && &commit.author == principal
            && result.history_tree_id.as_ref() == Some(&commit.history_tree_id)
    };
    let versions = candidate
        .versions
        .iter()
        .map(|evidence| (evidence.version.version_id.as_str(), &evidence.version))
        .collect::<BTreeMap<_, _>>();
    let mut covered = BTreeSet::new();
    for (id, branch) in next.branches.iter() {
        if previous
            .branches
            .get(id)
            .is_some_and(|old| old.head_commit_id == branch.head_commit_id)
        {
            continue;
        }
        let commit = next
            .commits
            .get(&branch.head_commit_id)
            .ok_or_else(invalid)?;
        if !native_commit(commit) || !advanced.branch_writable(next, id)? {
            continue;
        }
        if workspace
            && (result.branch_id.as_ref() != Some(id)
                || result.commit_id.as_ref() != Some(&commit.commit_id)
                || commit.kind != "content")
        {
            continue;
        }
        if workspace {
            let before = previous.branches.get(id).ok_or_else(invalid)?;
            if commit.parent_commit_ids.as_slice() != std::slice::from_ref(&before.head_commit_id) {
                return Err(invalid());
            }
        }
        for entry in advanced.branch_entries(id, &branch.head_commit_id)? {
            if entry.kind == NamespaceEntryKind::File
                && versions
                    .get(entry.version_id.as_str())
                    .is_some_and(|version| {
                        version.file_id == entry.file_id
                            && version.history_tree_id == commit.history_tree_id
                            && version.logical_size == entry.size
                    })
            {
                if copy_plan.as_ref().is_some_and(|plan| {
                    !plan.payloads[&entry.version_id]
                        .paths
                        .contains(entry.lookup_key())
                }) {
                    return Err(invalid());
                }
                covered.insert(entry.version_id.as_str());
            }
        }
    }
    if copied {
        for (id, branch) in next.directory_branches.iter() {
            if previous
                .directory_branches
                .get(id)
                .is_some_and(|old| old.head_tree_id == branch.head_tree_id)
            {
                continue;
            }
            let commit = next.commits.get(&branch.commit_id).ok_or_else(invalid)?;
            if !native_commit(commit) || !advanced.directory_writable(next, id)? {
                continue;
            }
            for entry in advanced.directory_entries(id, &branch.head_tree_id)? {
                if entry.kind == NamespaceEntryKind::File
                    && versions
                        .get(entry.version_id.as_str())
                        .is_some_and(|version| {
                            version.file_id == entry.file_id
                                && version.history_tree_id == commit.history_tree_id
                                && version.logical_size == entry.size
                        })
                {
                    if copy_plan.as_ref().is_some_and(|plan| {
                        !plan.payloads[&entry.version_id]
                            .paths
                            .contains(entry.lookup_key())
                    }) {
                        return Err(invalid());
                    }
                    covered.insert(entry.version_id.as_str());
                }
            }
        }
    }
    if covered.len() != candidate.versions.len() {
        return Err(invalid());
    }
    Ok(())
}

// Copy retains its original source/target request in the permanent transfer
// result. Recompute that request and resolve sources through the current batch
// admission, never through a raw historical tree or an arbitrary old body.
fn verify_copied_payload_request(
    view: &CurrentNamespaces,
    previous: &State,
    candidate: &Candidate,
    result: &OperationResult,
    principal: &str,
) -> Result<CopyPayloadPlan> {
    let invalid = || {
        Error::new(
            ErrorCode::PublicationInvalid,
            "copied payload differs from its native current namespace request",
        )
    };
    let transfers = result.result["transfers"].as_array().ok_or_else(invalid)?;
    let items = transfers
        .iter()
        .map(|transfer| {
            json_codec::from_value::<DirectoryNamespaceItem>(json_codec::json!({
                "source": transfer["source"], "target": transfer["target"],
            }))
            .map_err(|source| {
                Error::from(source).context(
                    ErrorCode::PublicationInvalid,
                    "invalid persisted namespace copy request",
                )
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let kind = if result.result["action"] == json_codec::json!(DirectoryBatchKind::Copy) {
        DirectoryBatchKind::Copy
    } else if result.result["action"] == json_codec::json!(DirectoryBatchKind::Separate) {
        DirectoryBatchKind::Separate
    } else {
        return Err(invalid());
    };
    let request = DirectoryNamespaceBatch { kind, items };
    let history = result.history_tree_id.as_ref().ok_or_else(invalid)?;
    let repository = &previous
        .histories
        .get(history)
        .ok_or_else(invalid)?
        .repository_id;
    if request.items.is_empty()
        || (kind == DirectoryBatchKind::Separate && request.items.len() != 1)
        || result.result["count"].as_str() != Some(request.items.len().to_string().as_str())
        || Engine::native_request_digest(
            "directory.namespace.batch",
            principal,
            &json_codec::json!({"repository_id": repository, "request": request}),
        )? != candidate.request_digest
    {
        return Err(invalid());
    }
    let (targets, retained) = copy_target_projections(view, previous, repository, &request)?;
    let sources = targets
        .values()
        .flat_map(|target| target.copies.values())
        .filter(|entry| entry.kind == NamespaceEntryKind::File)
        .map(|entry| {
            (
                entry.file_id.clone(),
                entry.version_id.clone(),
                entry.lookup_key().to_owned(),
            )
        })
        .collect::<BTreeSet<_>>();
    let copies = result.result["copies"].as_array().ok_or_else(invalid)?;
    if copies.len() != candidate.versions.len() {
        return Err(invalid());
    }
    let mut payloads = BTreeMap::new();
    for (copy, evidence) in copies.iter().zip(&candidate.versions) {
        let source_file = copy["source_file_id"].as_str().ok_or_else(invalid)?;
        let source_version = copy["source_version_id"].as_str().ok_or_else(invalid)?;
        let path = copy["path"].as_str().ok_or_else(invalid)?;
        let original = previous.versions.get(source_version).ok_or_else(invalid)?;
        if !sources.contains(&(
            source_file.to_owned(),
            source_version.to_owned(),
            name_policy::path_key(path),
        )) || copy["file_id"].as_str() != Some(evidence.version.file_id.as_str())
            || copy["version_id"].as_str() != Some(evidence.version.version_id.as_str())
            || previous.content_states.get(source_version)
                != Some(&storage_contract::ContentState::Available)
            || original.file_id != source_file
            || original.content_hash != evidence.version.content_hash
            || original.logical_size != evidence.version.logical_size
            || !evidence.version.created_as_full_checkpoint
            || !evidence.version.source_version_ids.is_empty()
        {
            return Err(invalid());
        }
        payloads.insert(
            evidence.version.version_id.clone(),
            CopiedPayload {
                source_file_id: source_file.into(),
                source_version_id: source_version.into(),
                paths: sources
                    .iter()
                    .filter(|(file, version, _)| file == source_file && version == source_version)
                    .map(|(_, _, path)| path.clone())
                    .collect(),
            },
        );
    }
    Ok(CopyPayloadPlan {
        payloads,
        targets,
        retained,
    })
}

struct CopiedPayload {
    source_file_id: String,
    source_version_id: String,
    paths: BTreeSet<String>,
}

struct CopyTarget {
    original: BTreeMap<String, Entry>,
    // Source identity/body at its requested destination, before allocating the
    // new copy identities. This is only a pure comparison projection.
    copies: BTreeMap<String, Entry>,
}

struct CopyPayloadPlan {
    payloads: BTreeMap<String, CopiedPayload>,
    targets: BTreeMap<String, CopyTarget>,
    retained: Option<RetainedFile>,
}

struct RetainedFile {
    directory_branch_id: String,
    entry: Entry,
    context: FileContext,
}

fn verify_retained_file(
    branch: &DirectoryBranch,
    entries: &[Entry],
    retained: &RetainedFile,
) -> Result<()> {
    // A child replacement may advance the ancestor's cached subtree. Preserve
    // the exact owned original occurrence/context, not the whole ancestor tree.
    if branch.directory_branch_id != retained.directory_branch_id
        || entries
            .iter()
            .find(|entry| entry.lookup_key() == retained.entry.lookup_key())
            != Some(&retained.entry)
        || branch.file_contexts.get(retained.entry.lookup_key()) != Some(&retained.context)
    {
        return Err(invalid_copy());
    }
    Ok(())
}

fn invalid_copy() -> Error {
    Error::new(
        ErrorCode::PublicationInvalid,
        "copied payload differs from its exact destination and fresh identity projection",
    )
}

fn descendant(path: &str, ancestor: &str) -> bool {
    path.strip_prefix(ancestor)
        .is_some_and(|suffix| suffix.starts_with('/'))
}

fn inside_scope(path: &str, scope: &str) -> bool {
    scope.is_empty() || descendant(path, scope)
}

// Reproduce the native batch destination preflight without opening a body or
// inventing new IDs. Requests consume original sources, but destinations see
// earlier additions in the same batch, including newly copied parent paths.
fn copy_target_projections(
    view: &CurrentNamespaces,
    previous: &State,
    repository: &str,
    request: &DirectoryNamespaceBatch,
) -> Result<(BTreeMap<String, CopyTarget>, Option<RetainedFile>)> {
    let mut scopes = BTreeMap::new();
    for item in &request.items {
        let target = item.target.as_ref().ok_or_else(invalid_copy)?;
        for (id, expected) in [
            (
                &item.source.directory_branch_id,
                &item.source.expected_tree_id,
            ),
            (&target.directory_branch_id, &target.expected_tree_id),
        ] {
            let branch = previous
                .directory_branches
                .get(id)
                .ok_or_else(invalid_copy)?;
            if branch.repository_id != repository || &branch.head_tree_id != expected {
                return Err(invalid_copy());
            }
            if !scopes.contains_key(id) {
                scopes.insert(id.clone(), view.batch_scope(previous, id)?);
            }
        }
    }
    let mut targets = BTreeMap::<String, CopyTarget>::new();
    let mut retained = None;
    let mut ranges = BTreeMap::<String, Vec<String>>::new();
    let mut mappings = BTreeSet::new();
    let mut target_scopes = BTreeSet::<String>::new();
    for item in &request.items {
        let target = item
            .target
            .as_ref()
            .expect("invariant: Copy requires a target");
        let source = scopes[&item.source.directory_branch_id].branch();
        let destination = scopes[&target.directory_branch_id].branch();
        let source_path = name_policy::PortablePath::parse(&item.source.path)?;
        let target_path = name_policy::PortablePath::parse(&target.path)?;
        if !inside_scope(source_path.key(), &name_policy::path_key(&source.path))
            || !inside_scope(target_path.key(), &name_policy::path_key(&destination.path))
        {
            return Err(invalid_copy());
        }
        let original = source
            .entries
            .iter()
            .find(|entry| {
                entry.entry_id == item.source.entry_id && entry.lookup_key() == source_path.key()
            })
            .ok_or_else(invalid_copy)?;
        if original.kind == NamespaceEntryKind::Directory
            && descendant(target_path.key(), original.lookup_key())
        {
            return Err(invalid_copy());
        }
        let source_ranges = ranges
            .entry(source.directory_branch_id.clone())
            .or_default();
        if source_ranges.iter().any(|path| {
            path != original.lookup_key()
                && (descendant(path, original.lookup_key())
                    || descendant(original.lookup_key(), path))
        }) || !mappings.insert((
            source.directory_branch_id.clone(),
            original.entry_id.clone(),
            original.lookup_key().to_owned(),
            target.directory_branch_id.clone(),
            target_path.key().to_owned(),
        )) {
            return Err(invalid_copy());
        }
        source_ranges.push(original.lookup_key().to_owned());
        if !targets.contains_key(&target.directory_branch_id) {
            let path = name_policy::path_key(&destination.path);
            if target_scopes.iter().any(|other| {
                other == &path || inside_scope(&path, other) || inside_scope(other, &path)
            }) {
                return Err(invalid_copy());
            }
            target_scopes.insert(path);
            targets.insert(
                target.directory_branch_id.clone(),
                CopyTarget {
                    original: destination
                        .entries
                        .iter()
                        .map(|entry| (entry.lookup_key().to_owned(), entry.clone()))
                        .collect(),
                    copies: BTreeMap::new(),
                },
            );
        }
        let projection = targets
            .get_mut(&target.directory_branch_id)
            .expect("invariant: target projection exists");
        let (content_scope, content_source) = if request.kind == DirectoryBatchKind::Separate {
            let secondary = crate::business::separated_file_source(
                previous,
                source,
                original,
                destination,
                &target_path,
            )?;
            if projection.original.remove(target_path.key()).as_ref() != Some(secondary) {
                return Err(invalid_copy());
            }
            retained = Some(RetainedFile {
                directory_branch_id: source.directory_branch_id.clone(),
                entry: original.clone(),
                context: source
                    .file_contexts
                    .get(original.lookup_key())
                    .cloned()
                    .ok_or_else(invalid_copy)?,
            });
            (destination, secondary)
        } else {
            (source, original)
        };
        if projection.original.contains_key(target_path.key())
            || projection.copies.contains_key(target_path.key())
        {
            return Err(invalid_copy());
        }
        if let Some((parent, _)) = target_path.key().rsplit_once('/')
            && parent != name_policy::path_key(&destination.path)
            && !projection
                .original
                .get(parent)
                .or_else(|| projection.copies.get(parent))
                .is_some_and(|entry| entry.kind == NamespaceEntryKind::Directory)
        {
            return Err(invalid_copy());
        }
        let depth = content_source.path().split('/').count();
        for entry in content_scope.entries.iter().filter(|entry| {
            entry.lookup_key() == content_source.lookup_key()
                || descendant(entry.lookup_key(), content_source.lookup_key())
        }) {
            let path = if entry.path().split('/').count() == depth {
                target_path.clone()
            } else {
                entry.location().rebase(depth, &target_path)?
            };
            let mut expected = entry.clone();
            expected.relocate(path);
            let key = expected.lookup_key().to_owned();
            if projection.original.contains_key(&key)
                || projection.copies.insert(key, expected).is_some()
            {
                return Err(invalid_copy());
            }
        }
    }
    Ok((targets, retained))
}

fn verify_copy_targets(
    source: &CurrentNamespaces,
    advanced: &CurrentNamespaces,
    next: &State,
    candidate: &Candidate,
    plan: &CopyPayloadPlan,
) -> Result<()> {
    let versions = candidate
        .versions
        .iter()
        .map(|evidence| (evidence.version.version_id.as_str(), &evidence.version))
        .collect::<BTreeMap<_, _>>();
    let mut entry_ids = BTreeSet::new();
    let mut file_ids = BTreeSet::new();
    let mut covered = BTreeSet::new();
    for (id, projection) in &plan.targets {
        let branch = next.directory_branches.get(id).ok_or_else(invalid_copy)?;
        let entries = advanced.directory_entries(id, &branch.head_tree_id)?;
        let actual = entries
            .iter()
            .map(|entry| (entry.lookup_key(), entry))
            .collect::<BTreeMap<_, _>>();
        if actual.len() != entries.len()
            || projection
                .original
                .len()
                .checked_add(projection.copies.len())
                != Some(entries.len())
            || projection
                .original
                .values()
                .any(|entry| actual.get(entry.lookup_key()).copied() != Some(entry))
        {
            return Err(invalid_copy());
        }
        for (path, expected) in &projection.copies {
            let entry = actual
                .get(path.as_str())
                .copied()
                .ok_or_else(invalid_copy)?;
            if entry.kind != expected.kind
                || entry.path() != expected.path()
                || entry.size != expected.size
                || !source.copy_identity_is_fresh(entry)
                || !entry_ids.insert(entry.entry_id.as_str())
            {
                return Err(invalid_copy());
            }
            match entry.kind {
                NamespaceEntryKind::Directory => {
                    if entry.file_id != expected.file_id || entry.version_id != expected.version_id
                    {
                        return Err(invalid_copy());
                    }
                }
                NamespaceEntryKind::File => {
                    let payload = plan
                        .payloads
                        .get(&entry.version_id)
                        .ok_or_else(invalid_copy)?;
                    let version = versions
                        .get(entry.version_id.as_str())
                        .ok_or_else(invalid_copy)?;
                    if payload.source_file_id != expected.file_id
                        || payload.source_version_id != expected.version_id
                        || !payload.paths.contains(path)
                        || version.file_id != entry.file_id
                        || !file_ids.insert(entry.file_id.as_str())
                        || !covered.insert(entry.version_id.as_str())
                    {
                        return Err(invalid_copy());
                    }
                }
            }
        }
    }
    if covered.len() != candidate.versions.len() {
        return Err(invalid_copy());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/recovery/file_separation_tests.rs"]
mod file_separation_tests;
