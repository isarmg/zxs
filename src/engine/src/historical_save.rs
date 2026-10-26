//! Historical branch records retain lineage; only staged writes gain content.
use crate::{
    business::{
        apply_workspace_namespace, deletion_record, ensure_parents, now, validate_entries,
        validate_workspace_dependencies,
    },
    disk::*,
    *,
};
use data_serialization::{Deserialize, Serialize};
use json_codec::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistoricalSaveOptions {
    pub source_commit_id: String,
    pub name: String,
    pub workspace_id: Option<String>,
    pub primary_slot_id: Option<String>,
    pub target_copies: Option<u32>,
}

fn text<'a>(input: &'a Value, key: &str) -> Result<&'a str> {
    input.get(key).and_then(Value::as_str).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::PlanInvalid,
            format!("missing historical plan field {key}"),
        )
    })
}
fn historical_workspace(
    engine: &Engine,
    id: &str,
    source: &str,
    principal: &str,
) -> Result<Workspace> {
    let workspace = engine.workspace(id)?;
    if workspace.principal != principal {
        return Err(Error::new(
            storage_contract::ErrorCode::PermissionDenied,
            "historical workspace belongs to another principal",
        ));
    }
    if workspace.purpose != WorkspacePurpose::HistoricalBranch || workspace.base_head != source {
        return Err(Error::new(
            storage_contract::ErrorCode::SaveModeUnsupported,
            "historical save requires a workspace explicitly bound to the selected historical commit",
        ));
    }
    if !matches!(
        workspace.state,
        WorkspaceState::Open | WorkspaceState::Sealed
    ) {
        return Err(Error::new(
            storage_contract::ErrorCode::WorkspaceClosed,
            "historical workspace is no longer available",
        ));
    }
    if now().saturating_sub(workspace.created_at) > engine.config.workspace_ttl_seconds {
        return Err(Error::new(
            storage_contract::ErrorCode::FencingTokenExpired,
            "historical workspace lease expired; work data is retained",
        ));
    }
    validate_workspace_dependencies(&workspace)?;
    Ok(workspace)
}

/// Compose a virtual final tree before reading any payload. Removed or fully
/// replaced old bytes are therefore never mistaken for required source data.
fn final_entries(
    source: &Commit,
    workspace: Option<&Workspace>,
    maximum: usize,
) -> Result<(Vec<Entry>, Vec<DeletedPath>)> {
    let mut deleted = Vec::new();
    let mut entries = source.entries.clone();
    if let Some(workspace) = workspace {
        for staged in workspace.files.values() {
            let previous =
                crate::business::find_unordered_key(&entries, staged.path.key()).cloned();
            if previous
                .as_ref()
                .is_some_and(|entry| entry.kind != NamespaceEntryKind::File)
            {
                return Err(Error::new(
                    storage_contract::ErrorCode::IsDirectory,
                    "historical upload cannot replace a directory",
                ));
            }
            ensure_parents(&mut entries, &staged.path)?;
            let key = staged.path.key();
            let entry = Entry {
                entry_id: staged.item_id.clone(),
                path: staged.path.clone(),
                kind: NamespaceEntryKind::File,
                file_id: previous
                    .as_ref()
                    .map(|entry| entry.file_id.clone())
                    .unwrap_or_else(|| format!("new-file:{}", staged.item_id)),
                version_id: format!("staged:{}", staged.item_id),
                size: staged.size,
            };
            entries.retain(|entry| entry.lookup_key() != key);
            entries.push(entry);
        }
        deleted = apply_workspace_namespace(
            &mut entries,
            &source.entries,
            &workspace.namespace_operations,
        )?;
    }
    validate_entries(&mut entries, maximum)?;
    Ok((entries, deleted))
}

impl Engine {
    pub fn create_historical_workspace(
        &mut self,
        branch: &str,
        source_commit: &str,
        principal: &str,
    ) -> Result<Workspace> {
        Self::require_actor(principal)?;
        self.require_write()?;
        let repository = self.repository_for_branch(branch)?;
        if self
            .state
            .branches
            .values()
            .any(|b| b.history_tree_id == repository.history_tree_id && b.file_id.is_some())
        {
            return Err(Error::new(
                storage_contract::ErrorCode::FileBranchScope,
                "historical file versions must be saved as independent file branches",
            ));
        }

        let selected = self.state.commits.get(source_commit).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::CommitNotFound,
                "historical source commit is missing",
            )
        })?;
        let branch_record = self.state.branches.get(branch).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::BranchNotFound,
                "source branch is missing",
            )
        })?;
        if branch_record.state == BranchState::Migrated {
            return Err(Error::new(
                storage_contract::ErrorCode::TreeMoved,
                "source branch has moved to an independent history domain",
            ));
        }
        if selected.history_tree_id != branch_record.history_tree_id {
            return Err(Error::new(
                storage_contract::ErrorCode::HistoryMismatch,
                "historical source is outside the selected branch's history domain",
            ));
        }
        if self
            .workspaces
            .values()
            .filter(|workspace| {
                matches!(
                    workspace.state,
                    WorkspaceState::Open | WorkspaceState::Sealed
                )
            })
            .count()
            >= self.config.max_workspaces
        {
            return Err(Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "workspace count budget exhausted",
            ));
        }
        let workspace = Workspace {
            workspace_id: new_id("ws"),
            purpose: WorkspacePurpose::HistoricalBranch,
            branch_id: branch.into(),
            principal: principal.into(),
            base_head: source_commit.into(),
            fencing_token: self.next_slot,
            files: BTreeMap::new(),
            selected_file_branches: BTreeMap::new(),
            selected_file_contexts: BTreeMap::new(),
            selected_directory_branches: BTreeMap::new(),
            directory_selection_generations: BTreeMap::new(),
            file_selection_generations: BTreeMap::new(),
            namespace_operations: Vec::new(),
            created_at: now(),
            state: WorkspaceState::Open,
        };
        self.persist_workspace(&workspace)?;
        self.workspaces
            .insert(workspace.workspace_id.clone(), workspace.clone());
        Ok(workspace)
    }

    pub fn create_historical_save_plan(
        &mut self,
        history: &str,
        options: HistoricalSaveOptions,
        principal: &str,
    ) -> Result<Plan> {
        Self::require_actor(principal)?;
        self.require_write()?;
        if self
            .state
            .branches
            .values()
            .any(|b| b.history_tree_id == history && b.file_id.is_some())
        {
            return Err(Error::new(
                storage_contract::ErrorCode::FileBranchScope,
                "historical file versions must be saved as independent file branches",
            ));
        }
        crate::business::validate_branch_name(&options.name)?;
        let repository = self.repository_for_history(history)?.clone();

        let source = self
            .state
            .commits
            .get(&options.source_commit_id)
            .filter(|commit| commit.history_tree_id == history)
            .cloned()
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::CommitNotFound,
                    "historical source commit is outside the selected domain",
                )
            })?;
        if self.state.branches.values().any(|branch| {
            branch.history_tree_id == history
                && name_policy::lookup_key(&branch.name) == name_policy::lookup_key(&options.name)
        }) {
            return Err(Error::new(
                storage_contract::ErrorCode::NameCollision,
                "historical branch name already exists",
            ));
        }
        if options.primary_slot_id.is_some() != options.target_copies.is_some() {
            return Err(Error::new(
                storage_contract::ErrorCode::PlacementInvalid,
                "historical placement overrides require both primary_slot_id and target_copies",
            ));
        }
        if let (Some(primary), Some(copies)) = (&options.primary_slot_id, options.target_copies) {
            self.route(primary, copies)?;
        }
        let workspace = if let Some(id) = &options.workspace_id {
            let workspace = historical_workspace(self, id, &options.source_commit_id, principal)?;
            if self
                .repository_for_branch(&workspace.branch_id)?
                .repository_id
                != repository.repository_id
            {
                return Err(Error::new(
                    storage_contract::ErrorCode::HistoryMismatch,
                    "historical workspace is outside the selected repository",
                ));
            }
            self.seal_workspace(id, principal)?;
            Some(self.workspace(id)?)
        } else {
            None
        };
        let (entries, _) = final_entries(&source, workspace.as_ref(), self.config.max_entries)?;
        let mut per_slot = BTreeMap::<String, u64>::new();
        let mut total = 0u64;
        let mut count = 0usize;
        for entry in entries
            .iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::File)
        {
            if let Some(item) = entry.version_id.strip_prefix("staged:") {
                let opened = self.open_workspace_content(
                    &workspace
                        .as_ref()
                        .expect("invariant: staged entries require a workspace")
                        .workspace_id,
                    item,
                    principal,
                )?;
                drop(opened);
            } else {
                // Unwritten files become placeholders, with no source-byte IO.
                continue;
            }
            let (primary, copies) = match (&options.primary_slot_id, options.target_copies) {
                (Some(primary), Some(copies)) => (primary.clone(), copies),
                _ => self.new_file_placement(&repository.repository_id, entry.path())?,
            };
            for slot in self.route(&primary, copies)? {
                let bytes = per_slot.entry(slot).or_default();
                *bytes = bytes.checked_add(entry.size).ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::ResourceLimit,
                        "historical copy budget overflow",
                    )
                })?;
            }
            total = total.checked_add(entry.size).ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::ResourceLimit,
                    "historical copy budget overflow",
                )
            })?;
            count += 1;
        }
        for (slot, bytes) in &per_slot {
            let disk = self.disk(slot)?;
            validate_seal(&self.config, disk)?;
            let capacity = disk
                .capacity_bytes
                .map(|limit| limit.saturating_sub(self.used_bytes(slot)))
                .unwrap_or(u64::MAX);
            if available(&disk.root)? < bytes.saturating_add(self.config.metadata_reserve_bytes)
                || *bytes > capacity
            {
                return Err(Error::new(
                    storage_contract::ErrorCode::NoSpace,
                    "historical full branch exceeds an assigned disk's free-space reserve",
                ));
            }
        }
        let input = json!({"history_tree_id":history,"source_commit_id":source.commit_id,"name":options.name,"workspace_id":options.workspace_id,"sealed_manifest_hash":workspace.as_ref().map(workspace_manifest_hash).transpose()?,"workspace_fencing_token":workspace.as_ref().map(|workspace|workspace.fencing_token.to_string()),"expected_topology_generation":self.state.topology_generation.to_string(),"primary_slot_id":options.primary_slot_id,"target_copies":options.target_copies,"branch_creation":"placeholder-until-write","placeholder_files":entries.iter().filter(|entry|entry.kind == NamespaceEntryKind::File&&!entry.version_id.starts_with("staged:")).count(),"full_copy_bytes":total.to_string(),"full_copy_files":count,"required_bytes_by_slot":per_slot.iter().map(|(slot,bytes)|(slot.clone(),bytes.to_string())).collect::<BTreeMap<_,_>>()});
        let plan = Plan {
            plan_id: new_id("plan"),
            kind: "historical_save".into(),
            principal: principal.into(),
            expected_sequence: self.state.business_generation,
            digest: object_hash("plan", &input)?,
            input,
            state: PlanState::Ready,
        };
        write_json(
            &self
                .root
                .join("plans")
                .join(format!("{}.json", plan.plan_id)),
            &plan,
            false,
        )?;
        std::sync::Arc::make_mut(&mut self.state)
            .plans
            .insert(plan.plan_id.clone(), plan.clone());
        Ok(plan)
    }

    pub fn commit_historical_save_plan(
        &mut self,
        id: &str,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        if let Some(previous) = self.state.operations.get(&operation) {
            if previous.kind == "history.historical_save"
                && previous.result["plan_id"].as_str() == Some(id)
                && self.operation_owner(operation)?.as_deref() == Some(principal)
            {
                return self.operation(operation);
            }
            return Err(Error::new(
                storage_contract::ErrorCode::IdempotencyKeyReused,
                "operation is already bound to another request",
            ));
        }
        let plan = self.plan(id)?;
        if plan.kind != "historical_save" || plan.principal != principal {
            return Err(Error::new(
                storage_contract::ErrorCode::PermissionDenied,
                "historical plan belongs to another operation or principal",
            ));
        }
        if plan.state != PlanState::Ready {
            return Err(Error::new(
                storage_contract::ErrorCode::PlanClosed,
                "historical plan is no longer ready",
            ));
        }
        if plan.expected_sequence != self.state.business_generation {
            return Err(Error::new(
                storage_contract::ErrorCode::PlanStale,
                "business state changed; preview historical save again",
            ));
        }
        if plan.input["branch_creation"].as_str() != Some("placeholder-until-write") {
            return Err(Error::new(
                storage_contract::ErrorCode::PlanStale,
                "preview this plan again under the placeholder branch rules",
            ));
        }
        let history = text(&plan.input, "history_tree_id")?.to_owned();
        let source_id = text(&plan.input, "source_commit_id")?.to_owned();
        let name = text(&plan.input, "name")?.to_owned();
        if text(&plan.input, "expected_topology_generation")?
            != self.state.topology_generation.to_string()
        {
            return Err(Error::new(
                storage_contract::ErrorCode::TopologyGenerationMismatch,
                "topology changed after historical preparation",
            ));
        }
        let workspace = if let Some(id) = plan.input["workspace_id"].as_str() {
            let workspace = historical_workspace(self, id, &source_id, principal)?;
            if workspace.state != WorkspaceState::Sealed
                || workspace_manifest_hash(&workspace)?
                    != text(&plan.input, "sealed_manifest_hash")?
                || workspace.fencing_token.to_string()
                    != text(&plan.input, "workspace_fencing_token")?
            {
                return Err(Error::new(
                    storage_contract::ErrorCode::SealedManifestMismatch,
                    "historical workspace differs from the sealed plan",
                ));
            }
            Some(workspace)
        } else {
            None
        };
        let primary_override = plan.input["primary_slot_id"].as_str().map(str::to_owned);
        let copies_override = plan.input["target_copies"]
            .as_u64()
            .map(|copies| {
                u32::try_from(copies).map_err(|source| {
                    Error::from(source).context(
                        storage_contract::ErrorCode::PlanInvalid,
                        "historical copy count is invalid",
                    )
                })
            })
            .transpose()?;
        let result = self.mutate(
            "history.historical_save",
            principal,
            operation,
            json!({"plan_id":id,"plan_digest":plan.digest}),
            |engine, state, pending| {
                let repository = engine.repository_for_history(&history)?.clone();


                if state.branches.values().any(|branch| {
                    branch.history_tree_id == history
                        && name_policy::lookup_key(&branch.name) == name_policy::lookup_key(&name)
                }) {
                    return Err(Error::new(storage_contract::ErrorCode::NameCollision, "historical branch name already exists"));
                }
                let source = state.commits.get(&source_id)
                    .filter(|commit| commit.history_tree_id == history)
                    .cloned()
                    .ok_or_else(|| Error::new(storage_contract::ErrorCode::CommitNotFound, "historical source no longer exists"))?;
                let (mut entries, deleted) = final_entries(&source, workspace.as_ref(), engine.config.max_entries)?;
                let branch_id = new_id("br");
                let commit_id = new_id("commit");
                let mut copied = 0usize;
                let mut pending_file_versions = BTreeMap::new();
                for entry in &mut entries {
                    entry.entry_id = new_id("entry");
                    if entry.kind != NamespaceEntryKind::File {
                        continue;
                    }
                    let (source_path, source_versions, expected_hash, expected_size) =
                        if let Some(item) = entry.version_id.strip_prefix("staged:") {
                            let staged = workspace.as_ref()
                                .and_then(|workspace| workspace.files.get(item))
                                .expect("invariant: staged entries were composed from this workspace");
                            (staged.source_path.clone(), staged.base_version_id.clone().into_iter().collect::<Vec<_>>(), staged.content_hash, staged.size)
                        } else {
                            pending_file_versions.insert(entry.file_id.clone(), entry.version_id.clone());
                            continue;
                        };
                    if entry.file_id.starts_with("new-file:") {
                        entry.file_id = new_id("file");
                    }
                    let (primary, copies) = match (&primary_override, copies_override) {
                        (Some(primary), Some(copies)) => (primary.clone(), copies),
                        _ => engine.new_file_placement(&repository.repository_id, entry.path())?,
                    };
                    let version = engine.build_version(
                        state, pending, crate::business::VersionBuild { history: &history, file_id: &entry.file_id, path: entry.location(), source: &source_path, primary: &primary, copies, source_ids: source_versions, checkpoint: true },
                    )?;
                    if version.logical_size != expected_size || version.content_hash != expected_hash {
                        return Err(Error::new(storage_contract::ErrorCode::ChecksumMismatch, "historical source changed after sealing"));
                    }
                    entry.version_id = version.version_id;
                    entry.size = version.logical_size;
                    copied += 1;
                }
                validate_entries(&mut entries, engine.config.max_entries)?;
                state.commits.insert(commit_id.clone(), Commit {
                    namespace_deletions: deletion_record(deleted, now()),
                    commit_id: commit_id.clone(), history_tree_id: history.clone(),
                    kind: "historical_save".into(), parent_commit_ids: vec![source_id.clone()],
                    tree_id: new_id("tree"), sequence: state.sequence, operation_id: operation,
                    author: principal.into(), message: format!("Save written content to historical branch {name}"),
                    entries, pending_file_versions: pending_file_versions.clone(), provenance: Some(source_id.clone()),
                });
                state.branches.insert(branch_id.clone(), Branch {file_node_id:None,
                    branch_id: branch_id.clone(), history_tree_id: history.clone(), name: name.clone(),
                    head_commit_id: commit_id.clone(), fork_commit_id: source_id.clone(),
                    branch_kind: storage_contract::BranchKind::Full, file_id: None, pending_file_versions: pending_file_versions.clone(), generation: 1, state: BranchState::Active,
                });
                let mut complete = plan.clone();
                complete.state = PlanState::Completed;
                state.plans.insert(id.into(), complete);
                Ok(json!({"plan_id":id,"history_tree_id":history,"branch_id":branch_id,"commit_id":commit_id,"source_commit_id":source_id,"full_copy_files":copied,"written_files":copied,"placeholder_files":pending_file_versions.len(),"workspace_id":workspace.as_ref().map(|workspace|workspace.workspace_id.clone())}))
            },
        )?;
        if let Some(mut workspace) = workspace {
            workspace.state = WorkspaceState::Committed;
            workspace.fencing_token = workspace.fencing_token.checked_add(1).ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::ResourceLimit,
                    "workspace fencing token exhausted",
                )
            })?;
            self.persist_workspace(&workspace)?;
            self.workspaces
                .insert(workspace.workspace_id.clone(), workspace);
        }
        Ok(result)
    }
}

#[cfg(test)]
#[path = "../../../test/engine/unit/historical_save/tests.rs"]
mod tests;
