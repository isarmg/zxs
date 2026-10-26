//! Captured scope reads and publication with session/generation fences.
use super::*;
pub(crate) fn start(state: RwSignal<State>, kind: Kind, entry: Option<Value>) {
    let draft = state.with_untracked(|snapshot| {
        if !file_actions::write_allowed(snapshot) {
            return Ok(None);
        }
        let entries = if let Some(entry) = entry {
            vec![entry]
        } else {
            let checked = snapshot
                .files
                .entries
                .iter()
                .filter(|entry| snapshot.files.checked_entries.contains(&entry_key(entry)))
                .cloned()
                .collect::<Vec<_>>();
            if checked.is_empty() {
                snapshot.files.selected.iter().cloned().collect()
            } else {
                checked
            }
        };
        if entries.is_empty() || snapshot.files.repository_id.is_empty() {
            return Ok(None);
        }
        let sources = source_branches(&snapshot.files);
        let selected_sources = sources
            .iter()
            .filter(|branch| branch["current"] == true)
            .map(branch_id)
            .collect::<BTreeSet<_>>();
        let id = api::operation_id()?;
        let repository = snapshot.files.repository_id.clone();
        let repository_path = snapshot.files.repository_path.clone();
        let disk_prefix = if repository_path.is_empty() {
            snapshot.files.directory.clone()
        } else {
            snapshot
                .files
                .directory
                .strip_suffix(&repository_path)
                .unwrap_or_default()
                .trim_end_matches('/')
                .to_owned()
        };
        let draft = Draft {
            id,
            kind,
            repository,
            repository_path,
            disk_prefix,
            names: entries
                .iter()
                .map(|entry| (entry_key(entry), text(entry, &["name"])))
                .collect(),
            entries,
            mappings: selected_sources
                .iter()
                .map(|source| Mapping {
                    source: source.clone(),
                    target: String::new(),
                })
                .collect(),
            source_resolved: sources.is_empty(),
            scope_read: None,
            sources,
            selected_sources,
            target_path: String::new(),
            targets: vec![],
            target_resolved: false,
            loading: false,
            submitted_operation: None,
            registration: None,
            generation: 0,
            error: String::new(),
        };
        Ok(Some(draft))
    });
    let draft = match draft {
        Ok(Some(draft)) => draft,
        Ok(None) => return,
        Err(error) => {
            fail(state, error);
            return;
        }
    };
    open_draft(state, draft);
}
pub(super) fn open_draft(state: RwSignal<State>, mut draft: Draft) {
    // Empty scopes in the identity-aware listing remain authoritative. A path
    // query must never resurrect branches belonging to a replaced directory.
    draft.source_resolved = draft.sources.is_empty();
    draft.loading = false;
    state.update(|s| {
        s.files.batch = Some(draft);
        s.page = crate::ui::Page::Files;
    });
    load_sources(state);
}
pub(super) fn selected_scope_body(draft: &Draft, sources: &[Value], path: &str) -> Value {
    let sources = sources
        .iter()
        .map(|source| {
            json!({
                "directory_branch_id":branch_id(source),
                "expected_tree_id":text(source, &["head_tree_id"])
            })
        })
        .collect::<Vec<_>>();
    let entries = draft
        .entries
        .iter()
        .map(|entry| {
            json!({
                "entry_id":entry_id(entry), "path":selected_path(entry)
            })
        })
        .collect::<Vec<_>>();
    json!({"path":path,"sources":sources,"entries":entries})
}
fn selected_scope_endpoint(repository: &str) -> String {
    format!(
        "/api/repositories/{}/directory-branches",
        api::segment(repository)
    )
}
pub(super) fn load_sources(state: RwSignal<State>) {
    let Some((epoch, draft)) = state
        .try_update(|s| {
            if !s.session_active() {
                return None;
            }
            let draft = s.files.batch.as_mut()?;
            if draft.source_resolved || draft.loading || draft.registration.is_some() {
                return None;
            }
            draft.loading = true;
            draft.error.clear();
            draft.generation += 1;
            Some((s.session_epoch, draft.clone()))
        })
        .flatten()
    else {
        return;
    };
    spawn_local(async move {
        let result = match start_scope_read(state, epoch, &draft.id, draft.generation) {
            Ok(Some(cancellation)) => {
                api::cancellable_request(
                    "POST",
                    &selected_scope_endpoint(&draft.repository),
                    Some(selected_scope_body(
                        &draft,
                        &draft.sources,
                        &draft.repository_path,
                    )),
                    cancellation,
                )
                .await
            }
            Ok(None) => return,
            Err(error) => Err(error),
        };
        complete_source_scopes(state, epoch, &draft.id, draft.generation, result);
    });
}
fn start_scope_read(
    state: RwSignal<State>,
    epoch: u64,
    id: &str,
    generation: u64,
) -> Result<Option<api::ReadCancellation>, api::Error> {
    if !current_session(state, epoch) || !current_target(state, id, generation) {
        return Ok(None);
    }
    let cancellation = api::ReadCancellation::new()?;
    update(state, |draft| {
        draft.scope_read = Some(std::sync::Arc::new(ScopeRead(cancellation.clone())))
    });
    Ok(Some(cancellation))
}
pub(super) fn complete_source_scopes(
    state: RwSignal<State>,
    epoch: u64,
    id: &str,
    generation: u64,
    result: Result<Value, api::Error>,
) {
    if !accept_read_response(state, epoch, &result) || !current_target(state, id, generation) {
        return;
    }
    update(state, |draft| {
        draft.loading = false;
        draft.scope_read = None;
    });
    match result {
        Ok(value) => update(state, |draft| {
            let sources = list(&value, "directory_branches");
            if value["repository_id"] != draft.repository
                || !same_source_heads(&draft.sources, &sources)
            {
                draft.error = t(
                    "源目录分支已改变，请取消并刷新后重试。",
                    "Source folder branches changed. Cancel and refresh before retrying.",
                );
                return;
            }
            draft.sources = sources;
            draft.source_resolved = true;
            draft.error.clear();
        }),
        Err(error) => {
            update(state, |draft| draft.error = error.to_string());
            fail(state, error);
        }
    }
}
fn same_source_heads(expected: &[Value], actual: &[Value]) -> bool {
    let heads = |values: &[Value]| {
        values
            .iter()
            .map(|branch| (branch_id(branch), text(branch, &["head_tree_id"])))
            .collect::<BTreeSet<_>>()
    };
    expected.len() == actual.len() && heads(expected) == heads(actual)
}
fn same_draft(state: RwSignal<State>, id: &str) -> bool {
    state.with_untracked(|s| s.files.batch.as_ref().is_some_and(|draft| draft.id == id))
}
pub(super) fn update(state: RwSignal<State>, f: impl FnOnce(&mut Draft)) {
    state.update(|s| {
        if let Some(draft) = s.files.batch.as_mut() {
            f(draft)
        }
    });
}
pub(super) fn toggle_source(state: RwSignal<State>, id: String, checked: bool) {
    update(state, |draft| {
        if checked {
            draft.selected_sources.insert(id.clone());
            if draft.kind.has_destination() {
                draft.mappings.push(Mapping {
                    source: id,
                    target: String::new(),
                });
            }
        } else {
            draft.selected_sources.remove(&id);
            draft.mappings.retain(|mapping| mapping.source != id);
        }
        draft.error.clear();
    });
}
fn scope_endpoint(repository: &str, path: &str) -> String {
    format!(
        "/api/repositories/{}/directory-branches?path={}&summary=true",
        api::segment(repository),
        api::segment(path)
    )
}
fn current_target(state: RwSignal<State>, id: &str, generation: u64) -> bool {
    state.with_untracked(|s| {
        s.files
            .batch
            .as_ref()
            .is_some_and(|draft| draft.id == id && draft.generation == generation)
    })
}
/// Return true only when a current, authoritative scope read found no records.
/// A path in an unscoped catalogue can belong to a different, moved directory.
pub(super) fn complete_target_scopes(
    state: RwSignal<State>,
    epoch: u64,
    id: &str,
    generation: u64,
    result: Result<Value, api::Error>,
) -> bool {
    if !accept_read_response(state, epoch, &result) || !current_target(state, id, generation) {
        return false;
    }
    match result {
        Ok(value) => {
            let mut needs_directory = false;
            update(state, |draft| {
                draft.scope_read = None;
                if value["repository_id"] != draft.repository {
                    draft.loading = false;
                    draft.error = t("目标目录范围无效。", "Invalid destination folder scope.");
                } else {
                    let targets = list(&value, "directory_branches");
                    if targets.is_empty() {
                        needs_directory = true;
                    } else {
                        draft.targets = targets;
                        draft.target_resolved = true;
                        draft.loading = false;
                    }
                }
            });
            needs_directory
        }
        Err(error) => {
            update(state, |draft| {
                draft.loading = false;
                draft.scope_read = None;
                draft.error = error.to_string();
            });
            fail(state, error);
            false
        }
    }
}
pub(super) fn load_target(state: RwSignal<State>) {
    if !state.with_untracked(State::session_active) {
        return;
    }
    let Some((target, generation, repository, disk_prefix, id, slot, epoch)) = state
        .with_untracked(|s| {
            let draft = s.files.batch.as_ref()?;
            if draft.registration.is_some() || draft.loading || !draft.source_resolved {
                return None;
            }
            Some((
                draft.target_path.trim_matches('/').to_owned(),
                draft.generation + 1,
                draft.repository.clone(),
                draft.disk_prefix.clone(),
                draft.id.clone(),
                s.files.slot.clone(),
                s.session_epoch,
            ))
        })
    else {
        return;
    };
    if !valid_path(&target) {
        update(state, |draft| {
            draft.error = t("目录路径无效。", "Invalid directory path.")
        });
        return;
    }
    update(state, |draft| {
        draft.target_path = target.clone();
        draft.targets.clear();
        draft.target_resolved = false;
        draft.error.clear();
        draft.generation += 1;
        draft.loading = true;
        for mapping in &mut draft.mappings {
            mapping.target.clear();
        }
    });
    let scoped_endpoint = scope_endpoint(&repository, &target);
    let entries_endpoint = format!(
        "/api/file-manager/disks/{}/entries?path={}&limit=1",
        api::segment(&slot),
        api::segment(&join_path(&disk_prefix, &target))
    );
    spawn_local(async move {
        let cancellation = match start_scope_read(state, epoch, &id, generation) {
            Ok(Some(value)) => value,
            Ok(None) => return,
            Err(error) => {
                complete_target_scopes(state, epoch, &id, generation, Err(error));
                return;
            }
        };
        let result = api::cancellable_get(&scoped_endpoint, cancellation.clone()).await;
        if !complete_target_scopes(state, epoch, &id, generation, result) {
            return;
        }
        // No registered scope exists. Verify the live writable directory before
        // offering registration, while leaving historical-only scopes readable.
        let cancellation = match start_scope_read(state, epoch, &id, generation) {
            Ok(Some(value)) => value,
            Ok(None) => return,
            Err(error) => {
                complete_target_scopes(state, epoch, &id, generation, Err(error));
                return;
            }
        };
        let result = api::cancellable_get(&entries_endpoint, cancellation).await;
        if !accept_read_response(state, epoch, &result) || !current_target(state, &id, generation) {
            return;
        }
        match result {
            Ok(value) => update(state, |draft| {
                draft.scope_read = None;
                draft.loading = false;
                if value["context"]["repository_id"] != draft.repository
                    || value["context"]["repository_path"] != target
                    || value["writable"] != true
                {
                    draft.error = t(
                        "目标必须是同一文件目录中的可写目录。",
                        "Choose a writable destination in the same file namespace.",
                    );
                } else {
                    draft.targets = list(&value, "directory_branches");
                    draft.target_resolved = true;
                    if draft.targets.is_empty() {
                        draft.error = t(
                            "目标尚无可操作的目录分支，请先登记目录分支。",
                            "Register a destination folder branch before continuing.",
                        );
                    }
                }
            }),
            Err(error) => {
                update(state, |draft| {
                    draft.loading = false;
                    draft.scope_read = None;
                    draft.error = error.to_string();
                });
                fail(state, error);
            }
        }
    });
}
pub(super) fn begin_scope_registration(
    draft: &mut Draft,
    operation_id: String,
    destination: bool,
) -> Option<ScopeRegistration> {
    if draft.loading
        || !draft.source_resolved
        || draft.registration.is_some()
        || draft.submitted_operation.is_some()
        || (destination && (!draft.target_resolved || !draft.targets.is_empty()))
        || (!destination && !draft.sources.is_empty())
    {
        return None;
    }
    let registration = ScopeRegistration {
        operation_id,
        destination,
        path: if destination {
            draft.target_path.clone()
        } else {
            draft.repository_path.clone()
        },
        generation: draft.generation,
        published: false,
    };
    draft.registration = Some(registration.clone());
    draft.loading = true;
    Some(registration)
}

pub(super) fn initialize_scope(state: RwSignal<State>, destination: bool) {
    let Some((repository, id, epoch)) = state.with_untracked(|s| {
        if !file_actions::write_allowed(s) {
            return None;
        }
        let draft = s.files.batch.as_ref()?;
        if draft.registration.is_some() || draft.loading {
            return None;
        }
        Some((draft.repository.clone(), draft.id.clone(), s.session_epoch))
    }) else {
        return;
    };
    let Some(operation_id) = crate::ui::operation_id(state) else {
        return;
    };
    let Some(registration) = state
        .try_update(|s| {
            let registration = begin_scope_registration(
                s.files.batch.as_mut()?,
                operation_id.clone(),
                destination,
            )?;
            s.files.mutation_pending = true;
            Some(registration)
        })
        .flatten()
    else {
        return;
    };
    let endpoint = format!(
        "/api/repositories/{}/directory-scopes",
        api::segment(&repository)
    );
    spawn_local(async move {
        let result = api::request(
            "POST",
            &endpoint,
            Some(json!({"path":registration.path,"operation_id":operation_id})),
        )
        .await;
        if !current_session(state, epoch) {
            return;
        }
        match result {
            Ok(value) => {
                // The same settlement hook resumes reads for direct and polled receipts.
                file_actions::record_receipt(state, &operation_id, value);
            }
            Err(error) => {
                if error.mutation_outcome_unknown() {
                    file_actions::record_operation(
                        state,
                        json!({"operation_id":operation_id,"state":"in_doubt"}),
                    );
                } else {
                    state.update(|s| {
                        settle_operation(
                            &mut s.files,
                            &operation_id,
                            super::super::model::SaveStatus::Rejected,
                        );
                    });
                }
                if same_draft(state, &id) {
                    update(state, |draft| draft.error = error.to_string());
                }
                fail(state, error);
            }
        }
        // Receipt settlement owns draft.loading: a confirmed receipt may already
        // have started a follow-on read that this POST must not release.
        state.update(|s| s.files.mutation_pending = false);
    });
}

pub(crate) fn reload_registered_scope(state: RwSignal<State>) {
    if !state.with_untracked(State::session_active) {
        return;
    }
    let Some((epoch, draft, registration)) = state
        .try_update(|s| {
            let draft = s.files.batch.as_mut()?;
            let registration = draft
                .registration
                .as_ref()
                .filter(|registration| registration.published)?;
            if draft.loading {
                return None;
            }
            let registration = registration.clone();
            draft.loading = true;
            draft.error.clear();
            Some((s.session_epoch, draft.clone(), registration))
        })
        .flatten()
    else {
        return;
    };
    spawn_local(async move {
        let result = async {
            let cancellation = start_scope_read(state, epoch, &draft.id, registration.generation)?
                .ok_or_else(api::session_changed)?;
            let value = api::cancellable_get(
                &scope_endpoint(&draft.repository, &registration.path),
                cancellation,
            )
            .await?;
            if !current_session(state, epoch)
                || !current_target(state, &draft.id, registration.generation)
            {
                return Err(api::session_changed());
            }
            if registration.destination || value["repository_id"] != draft.repository {
                return Ok(value);
            }
            let branches = list(&value, "directory_branches");
            if branches.is_empty() {
                return Ok(value);
            }
            let cancellation = start_scope_read(state, epoch, &draft.id, registration.generation)?
                .ok_or_else(api::session_changed)?;
            api::cancellable_request(
                "POST",
                &selected_scope_endpoint(&draft.repository),
                Some(selected_scope_body(&draft, &branches, &registration.path)),
                cancellation,
            )
            .await
        }
        .await;
        complete_registered_scope(
            state,
            epoch,
            &draft.id,
            &registration.operation_id,
            registration.generation,
            result,
        );
    });
}

pub(super) fn complete_registered_scope(
    state: RwSignal<State>,
    epoch: u64,
    id: &str,
    operation_id: &str,
    generation: u64,
    result: Result<Value, api::Error>,
) {
    if !accept_read_response(state, epoch, &result)
        || !state.with_untracked(|s| {
            s.files.batch.as_ref().is_some_and(|draft| {
                draft.id == id
                    && draft.generation == generation
                    && draft.registration.as_ref().is_some_and(|registration| {
                        registration.operation_id == operation_id && registration.published
                    })
            })
        })
    {
        return;
    }
    match result {
        Ok(value) => update(state, |draft| {
            draft.loading = false;
            draft.scope_read = None;
            let branches = list(&value, "directory_branches");
            if value["repository_id"] != draft.repository || branches.is_empty() {
                draft.error = t(
                    "目录分支记录尚未读取成功，请重试读取。",
                    "Folder branch records could not be loaded. Retry the read.",
                );
                return;
            }
            if draft
                .registration
                .as_ref()
                .expect("invariant: matching published registration was checked")
                .destination
            {
                draft.targets = branches;
                draft.target_resolved = true;
            } else {
                draft.sources = branches;
                draft.source_resolved = true;
                draft.selected_sources = draft
                    .sources
                    .iter()
                    .filter(|branch| branch["current"] == true)
                    .map(branch_id)
                    .collect();
                draft.mappings = draft
                    .selected_sources
                    .iter()
                    .map(|source| Mapping {
                        source: source.clone(),
                        target: String::new(),
                    })
                    .collect();
            }
            draft.registration = None;
            draft.error.clear();
        }),
        Err(error) => {
            update(state, |draft| {
                draft.loading = false;
                draft.scope_read = None;
                draft.error = error.to_string();
            });
            fail(state, error);
        }
    }
}
pub(super) fn submit(state: RwSignal<State>) {
    let Some((request, slot, epoch, draft_id)) = state.with_untracked(|s| {
        if !file_actions::write_allowed(s) {
            return None;
        }
        let draft = s.files.batch.as_ref()?;
        if draft.loading || draft.submitted_operation.is_some() || draft.registration.is_some() {
            return None;
        }
        Some((
            request(draft),
            s.files.slot.clone(),
            s.session_epoch,
            draft.id.clone(),
        ))
    }) else {
        return;
    };
    let mut body = match request {
        Ok(body) => body,
        Err(error) => {
            update(state, |draft| draft.error = error);
            return;
        }
    };
    let Some(id) = crate::ui::operation_id(state) else {
        return;
    };
    body["operation_id"] = id.clone().into();
    let endpoint = format!(
        "/api/file-manager/disks/{}/namespace-batch",
        api::segment(&slot)
    );
    state.update(|s| {
        s.files.mutation_pending = true;
        if let Some(draft) = s.files.batch.as_mut() {
            draft.submitted_operation = Some(id.clone());
        }
    });
    spawn_local(async move {
        let result = api::request("POST", &endpoint, Some(body)).await;
        if !current_session(state, epoch) {
            return;
        }
        match result {
            Ok(value) => {
                file_actions::record_receipt(state, &id, value);
            }
            Err(error) => {
                if error.mutation_outcome_unknown() {
                    file_actions::record_operation(
                        state,
                        json!({"operation_id":id,"state":"in_doubt"}),
                    );
                }
                if same_draft(state, &draft_id) {
                    update(state, |draft| {
                        draft.error = error.to_string();
                        if !error.mutation_outcome_unknown() {
                            draft.submitted_operation = None;
                        }
                    });
                }
                fail(state, error);
            }
        }
        state.update(|s| s.files.mutation_pending = false);
    });
}
