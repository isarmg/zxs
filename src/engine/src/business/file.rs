//! Independent file branches: metadata first, complete content on first save.
use super::*;
use json_codec::json;

pub(crate) fn selected_file_branch<'a>(
    state: &'a State,
    branch_id: &str,
    file_id: &str,
) -> Option<&'a Branch> {
    let branch = state.branches.get(branch_id)?;
    if state
        .repositories
        .values()
        .any(|repo| repo.main_branch_id == branch_id)
        && let Some(selected) = state.file_selections.get(file_id)
    {
        return state.branches.get(selected);
    }
    Some(branch)
}

pub(crate) fn pending_file_source<'a>(
    state: &'a State,
    branch_id: &str,
    file_id: &str,
) -> Option<&'a str> {
    selected_file_branch(state, branch_id, file_id)?
        .pending_file_versions
        .get(file_id)
        .map(String::as_str)
}

fn file_snapshot(entries: &[Entry], file: &Entry) -> Vec<Entry> {
    entries
        .iter()
        .filter(|entry| {
            (entry.kind == NamespaceEntryKind::File && entry.lookup_key() == file.lookup_key())
                || (entry.kind == NamespaceEntryKind::Directory
                    && super::namespace::is_descendant(file.lookup_key(), entry.lookup_key()))
        })
        .cloned()
        .collect()
}

pub(crate) fn synchronize_directory_edits(
    state: &mut State,
    commit_id: &str,
    written_paths: &[String],
    principal: &str,
    operation: OperationId,
) -> Result<()> {
    let current = state.commits.get(commit_id).cloned().ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::StateInvalid,
            "directory commit missing",
        )
    })?;
    for file in current.entries.iter().filter(|e| {
        e.kind == NamespaceEntryKind::File && written_paths.iter().any(|path| path == e.path())
    }) {
        let Some(repo) = state
            .repositories
            .values()
            .find(|r| r.history_tree_id == current.history_tree_id)
        else {
            continue;
        };
        let Some(context) =
            directory::context_at_key(state, &repo.repository_id, file.lookup_key())
        else {
            continue;
        };
        let branch_id = context.branch_id;
        if !state
            .branches
            .get(&branch_id)
            .is_some_and(|b| b.file_id.as_deref() == Some(&file.file_id))
        {
            continue;
        }

        let branch = state.branches.get(&branch_id).cloned().ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "selected file branch missing",
            )
        })?;
        if branch.history_tree_id != current.history_tree_id {
            continue;
        }
        if branch.state != BranchState::Active || state.file_branch_aliases.contains_key(&branch_id)
        {
            return Err(Error::new(
                storage_contract::ErrorCode::BranchNotActive,
                "selected file branch is not writable",
            ));
        }
        let previous = state.commits.get(&branch.head_commit_id).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "selected file head missing",
            )
        })?;
        if previous
            .entries
            .iter()
            .any(|e| e.file_id == file.file_id && e.version_id == file.version_id)
        {
            continue;
        }
        let id = new_id("commit");
        state.commits.insert(
            id.clone(),
            Commit {
                namespace_deletions: None,
                commit_id: id.clone(),
                history_tree_id: current.history_tree_id.clone(),
                kind: "file_content".into(),
                parent_commit_ids: vec![branch.head_commit_id],
                tree_id: new_id("tree"),
                sequence: state.sequence,
                operation_id: operation,
                author: principal.into(),
                message: current.message.clone(),
                entries: file_snapshot(&current.entries, file),
                pending_file_versions: Default::default(),
                provenance: None,
            },
        );
        let target = state.branches.get_mut(&branch_id).expect("checked");
        target.head_commit_id = id;
        target.pending_file_versions.remove(&file.file_id);
        target.file_node_id = None;
        target.generation = target.generation.checked_add(1).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "branch generation exhausted",
            )
        })?;
    }
    Ok(())
}

/// Publish one selected file into its directory. No sibling entry or payload is
/// copied, replaced or switched by this operation.
pub(crate) fn publish_selected_file(
    state: &mut State,
    branch_id: &str,
    principal: &str,
    operation: OperationId,
) -> Result<String> {
    publish_selected_file_at(state, branch_id, None, principal, operation)
}
pub(crate) fn publish_selected_file_at(
    state: &mut State,
    branch_id: &str,
    path: Option<&name_policy::PortablePath>,
    principal: &str,
    operation: OperationId,
) -> Result<String> {
    let branch = state.branches.get(branch_id).cloned().ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::BranchNotFound,
            "file branch missing",
        )
    })?;
    let file_id = branch.file_id.as_deref().ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::FileBranchRequired,
            "select an independent file branch",
        )
    })?;
    let source = state
        .commits
        .get(&branch.head_commit_id)
        .and_then(|c| c.entries.iter().find(|e| e.file_id == file_id))
        .cloned()
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "file branch content missing",
            )
        })?;
    let repository = state
        .repositories
        .values()
        .find(|r| r.history_tree_id == branch.history_tree_id)
        .cloned()
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::RepositoryNotFound,
                "file directory missing",
            )
        })?;
    let main = state
        .branches
        .get(&repository.main_branch_id)
        .cloned()
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::BranchNotFound,
                "directory branch missing",
            )
        })?;
    if main.state != BranchState::Active {
        return Err(Error::new(
            storage_contract::ErrorCode::BranchNotActive,
            "directory is not writable",
        ));
    }
    let current = state
        .commits
        .get(&main.head_commit_id)
        .cloned()
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "directory head missing",
            )
        })?;
    let selected_key = path
        .map(name_policy::PortablePath::key)
        .or_else(|| {
            state.file_locations.get(file_id).and_then(|path| {
                let key = name_policy::path_key(path);
                current
                    .entries
                    .iter()
                    .find(|entry| entry.file_id == file_id && entry.lookup_key() == key)
                    .map(|entry| entry.lookup_key())
            })
        })
        .or_else(|| {
            current
                .entries
                .iter()
                .find(|entry| entry.file_id == file_id && entry.kind == NamespaceEntryKind::File)
                .map(|entry| entry.lookup_key())
        })
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::FileNotFound,
                "file is absent from current composition",
            )
        })?;
    let context = directory::context_at_key(state, &repository.repository_id, selected_key);
    let inherited = context.as_ref().is_some_and(|c| c.inherited_from.is_some());
    state.file_write_origins.remove(file_id);
    let already_selected = context.as_ref().is_some_and(|c| c.branch_id == branch_id);
    if !inherited
        && context.as_ref().and_then(|c| c.node_id.as_ref()) == branch.file_node_id.as_ref()
        && already_selected
        && current.entries.iter().any(|e| {
            e.kind == NamespaceEntryKind::File
                && e.lookup_key() == selected_key
                && e.version_id == source.version_id
        })
    {
        return Ok(current.commit_id);
    }
    let mut entries = current.entries.clone();
    let target = entries
        .iter_mut()
        .find(|e| {
            e.file_id == file_id
                && e.lookup_key() == selected_key
                && e.kind == NamespaceEntryKind::File
        })
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::FileNotFound,
                "file was deleted from its directory",
            )
        })?;
    target.version_id = source.version_id;
    target.size = source.size;
    let changed_path = target.path().to_owned();
    let mut pending_file_versions = current.pending_file_versions.clone();
    if let Some(source) = branch.pending_file_versions.get(file_id) {
        pending_file_versions.insert(file_id.into(), source.clone());
    } else {
        pending_file_versions.remove(file_id);
    }
    let id = new_id("commit");
    state.commits.insert(
        id.clone(),
        Commit {
            namespace_deletions: None,
            commit_id: id.clone(),
            history_tree_id: branch.history_tree_id,
            kind: "file_selection".into(),
            parent_commit_ids: vec![current.commit_id],
            tree_id: new_id("tree"),
            sequence: state.sequence,
            operation_id: operation,
            author: principal.into(),
            message: format!("Select file branch {}", branch.name),
            entries,
            pending_file_versions,
            provenance: Some(branch.head_commit_id),
        },
    );
    let main = state
        .branches
        .get_mut(&repository.main_branch_id)
        .expect("main checked");
    main.head_commit_id = id.clone();
    // Selection delegates this file to its independent branch. An older
    // directory placeholder must not mask that branch's first real write.
    main.pending_file_versions.remove(file_id);
    main.generation = main.generation.checked_add(1).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::ResourceLimit,
            "branch generation exhausted",
        )
    })?;
    state
        .file_selections
        .insert(file_id.into(), branch_id.into());
    state.file_contexts.insert(
        directory::context_key_from_key(&repository.repository_id, selected_key),
        FileContext {
            node_id: branch.file_node_id.clone(),
            selection_generation: context
                .as_ref()
                .map(|c| c.selection_generation)
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::ResourceLimit,
                        "file context generation exhausted",
                    )
                })?,
            branch_id: branch_id.into(),
            inherited_from: None,
        },
    );
    super::directory::record_directory_edits(
        state,
        &repository.repository_id,
        &[changed_path],
        None,
    )?;
    Ok(id)
}

pub(crate) fn ensure_shared_file_branch(
    state: &mut State,
    current: &Commit,
    original: &Entry,
) -> Result<String> {
    let file_id = original.file_id.as_str();
    if !state.file_selections.contains_key(file_id) {
        if let Some(id) = state
            .repositories
            .values()
            .find(|r| r.history_tree_id == current.history_tree_id)
            .and_then(|r| {
                state.file_contexts.get(&directory::context_key_from_key(
                    &r.repository_id,
                    original.lookup_key(),
                ))
            })
            .filter(|c| {
                state
                    .branches
                    .get(&c.branch_id)
                    .is_some_and(|b| b.file_id.as_deref() == Some(file_id))
            })
            .map(|c| c.branch_id.clone())
        {
            state.file_selections.insert(file_id.into(), id.clone());
            return Ok(id);
        }
        if let Some(id) = state
            .branches
            .values()
            .find(|b| b.file_id.as_deref() == Some(file_id) && b.name == "main")
            .map(|b| b.branch_id.clone())
        {
            state.file_selections.insert(file_id.into(), id.clone());
            return Ok(id);
        }
        let id = new_id("br");
        let commit_id = new_id("commit");
        state.commits.insert(
            commit_id.clone(),
            Commit {
                namespace_deletions: None,
                commit_id: commit_id.clone(),
                history_tree_id: current.history_tree_id.clone(),
                kind: "file_branch_root".into(),
                parent_commit_ids: vec![current.commit_id.clone()],
                tree_id: new_id("tree"),
                sequence: state.sequence,
                operation_id: current.operation_id,
                author: current.author.clone(),
                message: "Initialize file main branch".into(),
                entries: file_snapshot(&current.entries, original),
                pending_file_versions: current
                    .pending_file_versions
                    .iter()
                    .filter(|(file, _)| file.as_str() == file_id)
                    .map(|(file, source)| (file.clone(), source.clone()))
                    .collect(),
                provenance: Some(current.commit_id.clone()),
            },
        );
        state.branches.insert(
            id.clone(),
            Branch {
                file_node_id: None,
                branch_id: id.clone(),
                history_tree_id: current.history_tree_id.clone(),
                name: "main".into(),
                head_commit_id: commit_id,
                fork_commit_id: current.commit_id.clone(),
                branch_kind: BranchKind::File,
                file_id: Some(file_id.into()),
                pending_file_versions: current
                    .pending_file_versions
                    .get(file_id)
                    .map(|version| [(file_id.into(), version.clone())].into())
                    .unwrap_or_default(),
                generation: 1,
                state: BranchState::Active,
            },
        );
        state.file_selections.insert(file_id.into(), id);
    }
    state.file_selections.get(file_id).cloned().ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::StateInvalid,
            "shared file branch missing",
        )
    })
}

pub(crate) struct FileBranchCreation<'a> {
    pub file_id: &'a str,
    pub source_version: &'a str,
    pub name: &'a str,
    pub name_key: &'a str,
}

pub(crate) fn create_file_branch_change(
    engine: &crate::engine::PlanningContext<'_>,
    state: &mut State,
    source: FileBranchCreation<'_>,
    principal: &str,
    operation: OperationId,
) -> Result<json_codec::Value> {
    let FileBranchCreation {
        file_id,
        source_version,
        name,
        name_key,
    } = source;
    let source = state
        .versions
        .get(source_version)
        .cloned()
        .filter(|v| v.file_id == file_id)
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::VersionNotFound,
                "selected version does not belong to this file",
            )
        })?;
    let repo = engine.repository_for_history(&source.history_tree_id)?;

    let main = state
        .branches
        .get(&repo.main_branch_id)
        .cloned()
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::BranchNotFound,
                "directory branch missing",
            )
        })?;
    if main.state != BranchState::Active {
        return Err(Error::new(
            storage_contract::ErrorCode::BranchNotActive,
            "directory is not writable",
        ));
    }
    let current = state
        .commits
        .get(&main.head_commit_id)
        .cloned()
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "directory head missing",
            )
        })?;
    let original = current
        .entries
        .iter()
        .find(|e| e.file_id == file_id && e.kind == NamespaceEntryKind::File)
        .cloned()
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::FileNotFound,
                "file no longer exists in this directory",
            )
        })?;
    if name_key == "main"
        || state.branches.values().any(|b| {
            b.file_id.as_deref() == Some(file_id) && name_policy::lookup_key(&b.name) == name_key
        })
    {
        return Err(Error::new(
            storage_contract::ErrorCode::NameCollision,
            "file branch name already exists",
        ));
    }
    ensure_shared_file_branch(state, &current, &original)?;
    let id = new_id("br");
    let commit_id = new_id("commit");
    let mut selected = original.clone();
    selected.version_id = source.version_id.clone();
    selected.size = source.logical_size;
    let entries = file_snapshot(&current.entries, &selected)
        .into_iter()
        .map(|entry| {
            if entry.file_id == file_id {
                selected.clone()
            } else {
                entry
            }
        })
        .collect();
    state.commits.insert(
        commit_id.clone(),
        Commit {
            namespace_deletions: None,
            commit_id: commit_id.clone(),
            history_tree_id: source.history_tree_id.clone(),
            kind: "file_branch_pending".into(),
            parent_commit_ids: vec![current.commit_id.clone()],
            tree_id: new_id("tree"),
            sequence: state.sequence,
            operation_id: operation,
            author: principal.into(),
            message: format!("Create file branch {name}"),
            entries,
            pending_file_versions: [(file_id.into(), source_version.into())].into(),
            provenance: Some(source_version.into()),
        },
    );
    state.branches.insert(
        id.clone(),
        Branch {
            file_node_id: None,
            branch_id: id.clone(),
            history_tree_id: source.history_tree_id.clone(),
            name: name.into(),
            head_commit_id: commit_id.clone(),
            fork_commit_id: current.commit_id,
            branch_kind: BranchKind::File,
            file_id: Some(file_id.into()),
            pending_file_versions: [(file_id.into(), source_version.into())].into(),
            generation: 1,
            state: BranchState::Active,
        },
    );
    Ok(
        json!({"file_id":file_id,"branch_id":id,"commit_id":commit_id,"history_tree_id":source.history_tree_id,"copied_files":"0","placeholder_files":"0","materialized":false,"inherited":true,"own_version_id":null,"version_id":source_version,"source_version_id":source_version,"content_state":"PENDING","selected":false}),
    )
}

impl Engine {
    pub fn set_file_note(
        &mut self,
        file_id: &str,
        version_id: Option<&str>,
        note: &storage_contract::Note,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        let note = note.as_str();
        self.mutate(
            if version_id.is_some() {
                "version.metadata"
            } else {
                "file.metadata"
            },
            principal,
            operation,
            json!({"file_id":file_id,"version_id":version_id,"note":note}),
            |engine, state, _| {
                let version = state
                    .versions
                    .values()
                    .find(|v| {
                        v.file_id == file_id && version_id.is_none_or(|id| v.version_id == id)
                    })
                    .ok_or_else(|| {
                        Error::new(
                            storage_contract::ErrorCode::FileNotFound,
                            "file or version not found",
                        )
                    })?;
                engine.repository_for_history(&version.history_tree_id)?;
                let notes = if version_id.is_some() {
                    &mut state.version_notes
                } else {
                    &mut state.file_notes
                };
                let key = version_id.unwrap_or(file_id);
                if note.is_empty() {
                    notes.remove(key);
                } else {
                    notes.insert(key.into(), note.into());
                }
                Ok(json!({"file_id":file_id,"version_id":version_id,"note":note}))
            },
        )
    }
    pub fn file_namespace(
        &mut self,
        repository: &str,
        change: NamespaceOperation,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        self.mutate("file.namespace",principal,operation,json!({"repository_id":repository,"change":change}),|engine,state,_|{
            let repo=engine.state.repositories.get(repository).ok_or_else(||Error::new(storage_contract::ErrorCode::RepositoryNotFound,"directory namespace missing"))?;

            let branch=state.branches.get(&repo.main_branch_id).cloned().ok_or_else(||Error::new(storage_contract::ErrorCode::BranchNotFound,"directory branch missing"))?;
            if branch.state!=BranchState::Active{return Err(Error::new(storage_contract::ErrorCode::BranchNotActive,"directory is not writable"));}
            let previous=state.commits.get(&branch.head_commit_id).cloned().ok_or_else(||Error::new(storage_contract::ErrorCode::StateInvalid,"directory head missing"))?;
            let mut entries=previous.entries.clone();let deleted=super::apply_namespace(&mut entries,&change)?;
            validate_entries(&mut entries,engine.config.max_entries)?;
            let renamed_directories=directory::prepare_renamed_directories(state,repository,&previous.entries,std::slice::from_ref(&change))?;
            super::directory::namespace_locations(state,repository,&previous.entries,&entries,std::slice::from_ref(&change),principal,operation)?;
            let pending_file_versions=previous.pending_file_versions.iter().filter(|(file,_)|entries.iter().any(|entry|entry.kind == NamespaceEntryKind::File&&entry.file_id==file.as_str())).map(|(file,source)|(file.clone(),source.clone())).collect();
            let id=new_id("commit");
            state.commits.insert(id.clone(),Commit{ namespace_deletions: namespace::deletion_record(deleted, now()?),commit_id:id.clone(),history_tree_id:branch.history_tree_id.clone(),kind:"namespace".into(),parent_commit_ids:vec![previous.commit_id],tree_id:new_id("tree"),sequence:state.sequence,operation_id:operation,author:principal.into(),message:format!("{} {}",change.kind().as_str(),change.path()),entries,pending_file_versions,provenance:None});
            let live_files=state.commits[&id].entries.iter().filter(|entry|entry.kind == NamespaceEntryKind::File).map(|entry|entry.file_id.clone()).collect::<std::collections::BTreeSet<_>>();
            let branch=state.branches.get_mut(&repo.main_branch_id).expect("checked");branch.head_commit_id=id.clone();branch.generation = branch.generation.checked_add(1).ok_or_else(|| Error::new(storage_contract::ErrorCode::ResourceLimit, "branch generation exhausted"))?;
            branch.pending_file_versions.retain(|file,_|live_files.contains(file));
            let mut changed_paths=vec![change.path().to_owned()];
            changed_paths.extend(renamed_directories);
            if let Some(destination)=&change.destination() {changed_paths.push(destination.as_str().to_owned());}
            let directories=super::directory::record_directory_edits(state,repository,&changed_paths,None)?;
            Ok(json!({"commit_id":id,"branch_id":repo.main_branch_id,"history_tree_id":repo.history_tree_id,"path":change.path(),"action":change.kind(),"directory_branch_ids":directories,"file_node_ids":file_paths::operation_nodes(state,operation)}))
        })
    }
    pub fn create_file_branch(
        &mut self,
        file_id: &str,
        source_version: &str,
        name: &storage_contract::BranchName,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        self.mutate(
            "file.branch.create",
            principal,
            operation,
            json!({"file_id":file_id,"source_version_id":source_version,"name":name.as_str()}),
            |engine, state, _| {
                create_file_branch_change(
                    &engine.planning_context(),
                    state,
                    FileBranchCreation {
                        file_id,
                        source_version,
                        name: name.as_str(),
                        name_key: name.key(),
                    },
                    principal,
                    operation,
                )
            },
        )
    }

    pub fn select_file_branch_at(
        &mut self,
        repository: &str,
        path: &name_policy::PortablePath,
        file_id: &str,
        branch_id: &str,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        self.mutate(
            "file.branch.select", principal, operation,
            json!({"repository_id":repository,"path":path.as_str(),"file_id":file_id,"branch_id":branch_id}),
            |engine, state, _| {
                let repo = state.repositories.get(repository)
                    .ok_or_else(|| Error::new(storage_contract::ErrorCode::RepositoryNotFound, "repository missing"))?;

                engine.repository_for_history(&repo.history_tree_id)?;
                let main = state.branches.get(&repo.main_branch_id)
                    .ok_or_else(|| Error::new(storage_contract::ErrorCode::StateInvalid, "repository main branch missing"))?;
                let current = state.commits.get(&main.head_commit_id)
                    .ok_or_else(|| Error::new(storage_contract::ErrorCode::StateInvalid, "repository head missing"))?;
                if !find_unordered_key(&current.entries, path.key()).is_some_and(|e| e.kind == NamespaceEntryKind::File && e.file_id == file_id) {
                    return Err(Error::new(storage_contract::ErrorCode::FileNotFound, "file/path identity differs"));
                }
                let branch = state.branches.get(branch_id).cloned()
                    .filter(|b| b.file_id.as_deref() == Some(file_id)
                        && b.history_tree_id == repo.history_tree_id)
                    .ok_or_else(|| Error::new(storage_contract::ErrorCode::FileBranchNotFound, "file branch missing"))?;
                if branch.state != BranchState::Active {
                    return Err(Error::new(storage_contract::ErrorCode::BranchNotActive, "merged file aliases are read-only; select an active branch"));
                }
                let selected = state.commits.get(&branch.head_commit_id)
                    .ok_or_else(|| Error::new(storage_contract::ErrorCode::StateInvalid, "file branch head missing"))?
                    .entries.iter().find(|e| e.kind == NamespaceEntryKind::File && e.file_id == file_id)
                    .ok_or_else(|| Error::new(storage_contract::ErrorCode::StateInvalid, "file branch entry missing"))?;
                // Pending branches retain frozen references rather than requiring
                // a fresh content replica for their metadata-only selection.
                if !branch.pending_file_versions.contains_key(file_id) {
                    engine.valid_replica(&selected.version_id)?;
                }
                let commit = publish_selected_file_at(state, branch_id, Some(path), principal, operation)?;
                Ok(json!({"file_id":file_id,"branch_id":branch_id,"commit_id":commit,"history_tree_id":branch.history_tree_id,"path":path.as_str()}))
            },
        )
    }
    pub fn select_file_branch(
        &mut self,
        file_id: &str,
        branch_id: &str,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        self.mutate(
            "file.branch.select", principal, operation,
            json!({"file_id":file_id,"branch_id":branch_id}),
            |engine, state, _| {
                let branch = state.branches.get(branch_id).cloned()
                    .filter(|b| b.file_id.as_deref() == Some(file_id))
                    .ok_or_else(|| Error::new(storage_contract::ErrorCode::FileBranchNotFound, "select an active branch of this file"))?;
                if branch.state != BranchState::Active {
                    return Err(Error::new(storage_contract::ErrorCode::BranchNotActive, "merged file aliases are read-only; select an active branch"));
                }
                engine.repository_for_history(&branch.history_tree_id)?;
                let selected = state.commits.get(&branch.head_commit_id)
                    .ok_or_else(|| Error::new(storage_contract::ErrorCode::StateInvalid, "file branch head missing"))?
                    .entries.iter().find(|e| e.kind == NamespaceEntryKind::File && e.file_id == file_id)
                    .ok_or_else(|| Error::new(storage_contract::ErrorCode::StateInvalid, "file branch entry missing"))?;
                if !branch.pending_file_versions.contains_key(file_id) {
                    engine.valid_replica(&selected.version_id)?;
                }
                let commit = publish_selected_file(state, branch_id, principal, operation)?;
                Ok(json!({"file_id":file_id,"branch_id":branch_id,"commit_id":commit,"history_tree_id":branch.history_tree_id}))
            },
        )
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/file/pending_regressions.rs"]
mod pending_regressions;
