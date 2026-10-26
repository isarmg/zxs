//! Durable workspace manifests and restart reconciliation.
use super::Engine;
use crate::journal::RecordType;
use crate::{disk::*, *};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

impl Engine {
    pub(crate) fn persist_workspace(&mut self, w: &Workspace) -> Result<()> {
        let admission = self.check_workspace_budget(w)?;
        let primary = self.workspace_data_directory(w)?;
        let result = (|| {
            write_json(&primary.join("workspace.json"), w, true)?;
            write_json(
                &self
                    .root
                    .join("workspaces")
                    .join(&w.workspace_id)
                    .join("workspace.json"),
                w,
                true,
            )
        })();
        // Either mirror may have landed. Never admit more mutations against
        // an in-memory view that no longer accounts for all retained evidence.
        if result.is_err() {
            self.blocked = true;
        } else {
            self.account_workspace(admission);
        }
        result
    }

    fn saved_request_digest_matches(
        &self,
        workspace: &Workspace,
        request: &CommitRequest,
        directories: &[PathBuf],
    ) -> Result<bool> {
        let Some(operation) = self.state.operations.get(&request.operation_id) else {
            return Ok(false);
        };
        if crate::business::native_commit_digest(workspace, request)
            .is_ok_and(|digest| digest == operation.request_digest)
        {
            return Ok(true);
        }
        // Completion advances the live fence, but the wire manifest commits
        // to the original request fence. Only restore that fence for hashing;
        // files, namespace operations, branch and base must remain unchanged.
        let mut frozen = workspace.clone();
        frozen.fencing_token = request.fencing_token;
        let Ok(manifest) = crate::wire::manifest_hash(&frozen) else {
            return Ok(false);
        };
        // Wire requests use a different envelope. Verify the original body,
        // not merely its caller-supplied digest string. Missing legacy body
        // copies leave the workspace untouched rather than guessing its state.
        for directory in directories {
            let body = match read_json::<json_codec::Value>(&directory.join("wire-request.json")) {
                Ok(body) => body,
                Err(error) if error.code() == storage_contract::ErrorCode::UnsafePath => {
                    return Err(error);
                }
                Err(_) => continue,
            };
            if body["operation_id"].as_str() == Some(request.operation_id.as_str())
                && body["workspace_id"].as_str() == Some(workspace.workspace_id.as_str())
                && body["branch_id"].as_str() == Some(workspace.branch_id.as_str())
                && body["expected_head"].as_str() == Some(request.expected_head.as_str())
                && body["fencing_token"].as_str()
                    == Some(request.fencing_token.to_string().as_str())
                && body["message"].as_str() == Some(request.message.as_str())
                && body["request_digest"].as_str() == Some(operation.request_digest.as_str())
                && body["sealed_manifest_hash"].as_str() == Some(manifest.as_str())
                && crate::wire::commit_request_digest(&workspace.workspace_id, &body)
                    .is_ok_and(|digest| digest == operation.request_digest)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) fn workspace_data_directory(&self, workspace: &Workspace) -> Result<PathBuf> {
        self.planning_context().workspace_data_directory(workspace)
    }
    /// Read-only recovery may retain system-side evidence while the primary
    /// is physically absent. A present disk must still have its expected seal.
    fn workspace_recovery_directory(&self, workspace: &Workspace) -> Result<PathBuf> {
        let repository = self.repository_for_branch(&workspace.branch_id)?;
        let disk = self.disk(&repository.primary_slot_id)?;
        if !disk_unavailable(disk)? {
            validate_seal(&self.config, disk)?;
        }
        Ok(disk
            .root
            .join("staging/workspaces")
            .join(&workspace.workspace_id))
    }

    /// Before journal replay these records are only discovery hints for payload
    /// repair (which independently verifies candidate-bound bytes). No caller can
    /// observe them until final reconciliation with the published history.
    pub(crate) fn seed_workspace_recovery(&mut self) -> Result<()> {
        self.load_workspace_mirrors(false)
    }

    pub(crate) fn load_workspaces(&mut self) -> Result<()> {
        self.load_workspace_mirrors(true)
    }

    fn load_workspace_mirrors(&mut self, authoritative: bool) -> Result<()> {
        let mut roots = vec![self.root.join("workspaces")];
        roots.extend(
            self.config
                .disks
                .iter()
                .filter(|d| d.level == 1)
                .map(|d| d.root.join("staging/workspaces")),
        );
        // Collect paths, not decoded manifests. At most two bounded metadata
        // inputs may represent an ID; extra copies are a conflict, not votes.
        let mut candidates = BTreeMap::<String, Vec<PathBuf>>::new();
        for root in roots {
            match fs::symlink_metadata(&root) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
                Ok(_) => {
                    if let Some(disk) = self
                        .config
                        .disks
                        .iter()
                        .find(|disk| root == disk.root.join("staging/workspaces"))
                    {
                        validate_seal(&self.config, disk)?;
                    }
                    existing_private_dir(&root)?;
                }
            }
            for entry in fs::read_dir(&root)? {
                let entry = entry?;
                if entry.file_type()?.is_symlink() {
                    return Err(Error::new(
                        storage_contract::ErrorCode::UnsafePath,
                        "linked workspace directory",
                    ));
                }
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let path = entry.path().join("workspace.json");
                match fs::symlink_metadata(&path) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error.into()),
                    Ok(_) => {}
                }
                let id = entry.file_name().into_string().map_err(|name| {
                    Error::PathEncoding(name).context(
                        ErrorCode::UnsafePath,
                        "invalid workspace directory identity",
                    )
                })?;
                validate_id(&id, "ws")?;
                if !candidates.contains_key(&id)
                    && candidates.len() >= self.effective_workspace_limits().max_retained_workspaces
                {
                    return Err(Error::new(
                        storage_contract::ErrorCode::ResourceLimit,
                        "retained workspace scan budget exhausted",
                    ));
                }
                let paths = candidates.entry(id).or_default();
                if paths.len() == 2 {
                    return Err(recovery_required("too many workspace mirrors"));
                }
                paths.push(path);
            }
        }
        self.workspaces.clear();
        self.reset_workspace_budget();
        for (id, paths) in candidates {
            let mut healthy = Vec::new();
            let mut damaged = false;
            for path in &paths {
                match read_json::<Workspace>(path) {
                    Ok(workspace) => {
                        self.validate_workspace_mirror(&id, path, &workspace, authoritative)?;
                        healthy.push(workspace);
                    }
                    Err(error) if error.code() == storage_contract::ErrorCode::UnsafePath => {
                        return Err(error);
                    }
                    Err(_) => damaged = true,
                }
            }
            if healthy.is_empty() {
                return Err(recovery_required("no healthy workspace manifest mirror"));
            }
            if authoritative && healthy.len() == 2 {
                self.reconcile_completion_contexts(&mut healthy)?;
            }
            if authoritative {
                for workspace in &mut healthy {
                    let published = self.reconcile_workspace_receipt(workspace)?;
                    if damaged && workspace.state == WorkspaceState::Committed && !published {
                        return Err(recovery_required(
                            "surviving committed manifest lacks a bound receipt",
                        ));
                    }
                }
            }
            let selected = healthy.remove(0);
            for other in &healthy {
                let mut left = selected.clone();
                let mut right = other.clone();
                if !authoritative {
                    // Only a possible commit completion may await journal
                    // evidence. An abort or arbitrary newer fence is not a
                    // reason to let recovery publish a pending decision.
                    if (left.state != right.state || left.fencing_token != right.fencing_token)
                        && !possible_commit_transition(&left, &right)
                        && !possible_commit_transition(&right, &left)
                    {
                        return Err(recovery_required(
                            "workspace state or fence mirrors conflict",
                        ));
                    }
                    if possible_commit_transition(&left, &right)
                        || possible_commit_transition(&right, &left)
                    {
                        // These fields are refreshed at completion. Only the
                        // exact publication snapshot may validate them later.
                        left.selected_file_contexts.clear();
                        right.selected_file_contexts.clear();
                    }
                    left.state = WorkspaceState::Open;
                    right.state = WorkspaceState::Open;
                    left.fencing_token = 0;
                    right.fencing_token = 0;
                }
                if canonical_json(&left)? != canonical_json(&right)? {
                    return Err(recovery_required("workspace manifest mirrors conflict"));
                }
            }
            if damaged {
                // A surviving manifest alone is not evidence that the promised
                // staging bytes survived. Verify them without changing either
                // mirror, so all damaged evidence remains available for repair.
                for file in selected.files.values() {
                    let mut source = open_regular(&file.source_path)?;
                    let digest = blake3::Hash::from_hex(&file.content_hash).map_err(|source| {
                        Error::from(source)
                            .context(ErrorCode::RecoveryRequired, "invalid staged content digest")
                    })?;
                    platform_io::payload::verify_full(
                        &mut source,
                        platform_io::payload::Expectation {
                            bytes: file.size,
                            blake3: Some(digest),
                        },
                    )
                    .map_err(|source| {
                        Error::from(source).context(
                            ErrorCode::RecoveryRequired,
                            "workspace staging bytes do not match surviving mirror",
                        )
                    })?;
                }
            }
            let admission = self.check_workspace_budget(&selected)?;
            self.account_workspace(admission);
            self.workspaces.insert(id, selected);
        }
        Ok(())
    }

    fn validate_workspace_mirror(
        &self,
        id: &str,
        path: &Path,
        workspace: &Workspace,
        authoritative: bool,
    ) -> Result<()> {
        if workspace.workspace_id != id {
            return Err(recovery_required(
                "workspace manifest identity differs from directory",
            ));
        }
        if workspace.fencing_token == 0 {
            return Err(recovery_required(
                "invalid workspace state or fencing token",
            ));
        }
        let local = self.root.join("workspaces").join(id);
        let primary = if authoritative {
            let branch = self
                .state
                .branches
                .get(&workspace.branch_id)
                .ok_or_else(|| {
                    recovery_required("workspace branch is absent from verified history")
                })?;
            if let Some(base) = self.state.commits.get(&workspace.base_head) {
                if base.history_tree_id != branch.history_tree_id {
                    return Err(recovery_required(
                        "workspace base belongs to another history",
                    ));
                }
            } else if matches!(
                workspace.state,
                WorkspaceState::Open | WorkspaceState::Sealed
            ) {
                return Err(recovery_required(
                    "active workspace base is absent from verified history",
                ));
            }
            Some(self.workspace_recovery_directory(workspace)?)
        } else {
            None
        };
        if let Some(primary) = &primary
            && path.parent() != Some(local.as_path())
            && path.parent() != Some(primary.as_path())
        {
            return Err(recovery_required(
                "workspace mirror is not on its authoritative primary",
            ));
        }
        for file in workspace.files.values() {
            let owned = if let Some(primary) = &primary {
                file.source_path.starts_with(primary) || file.source_path.starts_with(&local)
            } else {
                file.source_path.starts_with(&local)
                    || self.config.disks.iter().any(|disk| {
                        file.source_path
                            .starts_with(disk.root.join("staging/workspaces").join(id))
                    })
            };
            if !owned
                || file
                    .source_path
                    .components()
                    .any(|part| matches!(part, Component::ParentDir))
            {
                return Err(Error::new(
                    storage_contract::ErrorCode::UnsafePath,
                    "workspace staging path escapes owned directory",
                ));
            }
            // Missing bytes can be an existing unavailable-content condition,
            // but an unsafe source must not be hidden by a healthy JSON mirror.
            if let Err(error) = open_regular(&file.source_path)
                && error.code() == storage_contract::ErrorCode::UnsafePath
            {
                return Err(error);
            }
        }
        Ok(())
    }

    fn reconcile_completion_contexts(&self, mirrors: &mut [Workspace]) -> Result<()> {
        if mirrors[0].selected_file_contexts == mirrors[1].selected_file_contexts {
            return Ok(());
        }
        let before = if possible_commit_transition(&mirrors[0], &mirrors[1]) {
            0
        } else if possible_commit_transition(&mirrors[1], &mirrors[0]) {
            1
        } else {
            return Err(recovery_required("workspace context mirrors conflict"));
        };
        let mut expected = mirrors[before].clone();
        // Historical-save completion does not update selection contexts.
        if expected.purpose != WorkspacePurpose::Normal {
            return Err(recovery_required(
                "historical workspace context mirrors conflict",
            ));
        }
        let published = self.workspace_publication_snapshot(&expected)?;
        Self::refresh_workspace_contexts(&published, &mut expected);
        expected.state = WorkspaceState::Committed;
        expected.fencing_token = expected
            .fencing_token
            .checked_add(1)
            .ok_or_else(|| recovery_required("workspace fencing generation exhausted"))?;
        if canonical_json(&expected)? != canonical_json(&mirrors[1 - before])? {
            return Err(recovery_required(
                "workspace completion differs from published context",
            ));
        }
        mirrors[before] = expected;
        Ok(())
    }

    /// Reconstruct incomplete completion contexts from their original decision.
    /// Current selections may have changed since the save, so they cannot prove
    /// the receipt. Bind the original snapshot to the verified operation/slot.
    fn workspace_publication_snapshot(&self, workspace: &Workspace) -> Result<Arc<State>> {
        let directories = [
            self.root.join("workspaces").join(&workspace.workspace_id),
            self.workspace_recovery_directory(workspace)?,
        ];
        for directory in &directories {
            let request = match read_json::<CommitRequest>(&directory.join("commit-request.json")) {
                Ok(request) => request,
                Err(error) if error.code() == storage_contract::ErrorCode::UnsafePath => {
                    return Err(error);
                }
                Err(_) => continue,
            };
            if !self.workspace_request_matches_operation(workspace, &request)
                || workspace.fencing_token != request.fencing_token
                || !self.saved_request_digest_matches(workspace, &request, &directories)?
            {
                continue;
            }
            let operation = &self.state.operations[&request.operation_id];
            let members = self.trusted_evidence_members();
            for member in &members {
                for (record, _) in crate::journal::scan(member)?.records {
                    if record.sequence != operation.sequence
                        || record.record_type != RecordType::Publication
                    {
                        continue;
                    }
                    let candidate: crate::journal::Candidate =
                        crate::publication_evidence::verified_object(
                            &members,
                            "candidate",
                            &record.candidate_hash,
                        )?;
                    if candidate.operation_id != request.operation_id
                        || candidate.sequence != operation.sequence
                    {
                        continue;
                    }
                    // Recovery already verified the current root. Reuse it only
                    // for this exact publication, never for a later selection.
                    // Degraded recovery overlays its operation result, not the
                    // snapshot's file contexts or operation ownership.
                    let state =
                        if self.snapshot.as_deref() == Some(candidate.snapshot_hash.as_str()) {
                            Arc::clone(&self.state)
                        } else {
                            Arc::new(self.load_bound_snapshot(&candidate.snapshot_hash)?)
                        };
                    if state
                        .operations
                        .get(&request.operation_id)
                        .is_some_and(|bound| {
                            bound.sequence == operation.sequence
                                && bound.request_digest == operation.request_digest
                        })
                        && state.operation_principals.get(&request.operation_id)
                            == Some(&workspace.principal)
                    {
                        return Ok(state);
                    }
                }
            }
        }
        Err(recovery_required(
            "workspace completion has no bound publication snapshot",
        ))
    }

    fn reconcile_workspace_receipt(&self, workspace: &mut Workspace) -> Result<bool> {
        let directories = [
            self.root.join("workspaces").join(&workspace.workspace_id),
            self.workspace_recovery_directory(workspace)?,
        ];
        let mut completion = None;
        for directory in &directories {
            let request = match read_json::<CommitRequest>(&directory.join("commit-request.json")) {
                Ok(request) => request,
                Err(error) if error.code() == storage_contract::ErrorCode::UnsafePath => {
                    return Err(error);
                }
                Err(_) => continue,
            };
            if self.workspace_request_matches_operation(workspace, &request)
                && self.saved_request_digest_matches(workspace, &request, &directories)?
            {
                // Closing an already published workspace is permitted. A later
                // abort is terminal, not a request to resurrect the old fence.
                if workspace.state == WorkspaceState::Aborted {
                    continue;
                }
                let next = request
                    .fencing_token
                    .checked_add(1)
                    .ok_or_else(|| recovery_required("workspace fencing generation exhausted"))?;
                if !matches!(
                    workspace.state,
                    WorkspaceState::Open | WorkspaceState::Sealed | WorkspaceState::Committed
                ) || !(workspace.fencing_token == request.fencing_token
                    || workspace.fencing_token == next)
                    || completion.is_some_and(|fence| fence != next)
                {
                    return Err(recovery_required(
                        "workspace receipt conflicts with retained fence or state",
                    ));
                }
                completion = Some(next);
            }
        }
        if workspace.purpose == WorkspacePurpose::HistoricalBranch
            && workspace.state != WorkspaceState::Aborted
        {
            for plan in self.state.plans.values().filter(|plan| {
                plan.kind == "historical_save"
                    && plan.state == PlanState::Completed
                    && plan.principal == workspace.principal
                    && plan.input["workspace_id"].as_str() == Some(workspace.workspace_id.as_str())
                    && plan.input["source_commit_id"].as_str() == Some(workspace.base_head.as_str())
            }) {
                let Some(fence) = plan.input["workspace_fencing_token"]
                    .as_str()
                    .and_then(|fence| fence.parse::<u64>().ok())
                else {
                    continue;
                };
                let Some(next) = fence.checked_add(1) else {
                    continue;
                };
                let mut frozen = workspace.clone();
                frozen.fencing_token = fence;
                if plan.input["sealed_manifest_hash"].as_str()
                    != Some(crate::wire::manifest_hash(&frozen)?.as_str())
                {
                    continue;
                }
                if !matches!(
                    workspace.state,
                    WorkspaceState::Sealed | WorkspaceState::Committed
                ) || !(workspace.fencing_token == fence || workspace.fencing_token == next)
                    || completion.is_some_and(|other| other != next)
                {
                    return Err(recovery_required(
                        "historical workspace plan conflicts with retained fence or state",
                    ));
                }
                completion = Some(next);
            }
        }
        if let Some(fence) = completion {
            if workspace.purpose == WorkspacePurpose::Normal
                && matches!(
                    workspace.state,
                    WorkspaceState::Open | WorkspaceState::Sealed
                )
            {
                // Both mirrors may predate completion. Later selections cannot
                // replace the destination bound by this workspace's publication.
                let published = self.workspace_publication_snapshot(workspace)?;
                Self::refresh_workspace_contexts(&published, workspace);
            }
            workspace.state = WorkspaceState::Committed;
            workspace.fencing_token = fence;
        }
        Ok(completion.is_some())
    }
}

fn recovery_required(message: &str) -> Error {
    Error::new(storage_contract::ErrorCode::RecoveryRequired, message)
}

fn possible_commit_transition(before: &Workspace, after: &Workspace) -> bool {
    matches!(before.state, WorkspaceState::Open | WorkspaceState::Sealed)
        && after.state == WorkspaceState::Committed
        && before.fencing_token.checked_add(1) == Some(after.fencing_token)
}

impl crate::engine::PlanningContext<'_> {
    pub(crate) fn require_saved_workspace_request(
        &self,
        workspace: &Workspace,
        request: &CommitRequest,
    ) -> Result<()> {
        let expected = canonical_json(request)?;
        let local = self
            .root
            .join("workspaces")
            .join(&workspace.workspace_id)
            .join("commit-request.json");
        let matches = |path: &std::path::Path| -> Result<bool> {
            match read_json::<CommitRequest>(path) {
                Ok(saved) => Ok(canonical_json(&saved)? == expected),
                Err(error) if error.code() == storage_contract::ErrorCode::UnsafePath => Err(error),
                Err(_) => Ok(false),
            }
        };
        if matches(&local)? {
            return Ok(());
        }
        let data = self
            .workspace_data_directory(workspace)?
            .join("commit-request.json");
        if matches(&data)? {
            return Ok(());
        }
        Err(Error::new(
            storage_contract::ErrorCode::RecoveryRequired,
            "no exact saved workspace commit request is available",
        ))
    }
}

impl crate::engine::PlanningContext<'_> {
    pub(crate) fn workspace_data_directory(&self, w: &Workspace) -> Result<PathBuf> {
        let repo = self.repository_for_branch(&w.branch_id)?;
        let disk = self.disk(&repo.primary_slot_id)?;
        validate_seal(self.config, disk)?;
        Ok(disk.root.join("staging/workspaces").join(&w.workspace_id))
    }
}
