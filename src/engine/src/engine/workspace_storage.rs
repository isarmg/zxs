//! Durable workspace manifests and restart reconciliation.
use super::{
    Engine, ReadSource,
    workspace_budget::{WorkspaceColdKind, WorkspaceMirrors, WorkspaceRetention},
};
use crate::journal::RecordType;
use crate::{disk::*, *};
use std::{
    borrow::Cow,
    fs,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

enum WorkspaceLocation {
    System(DiskRoot),
    Primary(DiskRoot),
}

impl WorkspaceLocation {
    fn root(&self) -> &DiskRoot {
        match self {
            Self::System(root) | Self::Primary(root) => root,
        }
    }

    fn directory(&self) -> PathBuf {
        self.root().path().to_owned()
    }

    fn source(&self, path: PathBuf) -> WorkspaceSource<'_> {
        WorkspaceSource {
            location: self,
            path,
        }
    }
}

/// A manifest address is bound to the same admitted private root used for
/// discovery, not re-resolved through the member root after path replacement.
struct WorkspaceSource<'a> {
    location: &'a WorkspaceLocation,
    path: PathBuf,
}

impl WorkspaceSource<'_> {
    fn path(&self) -> &Path {
        &self.path
    }

    fn open(&self) -> Result<fs::File> {
        self.location.root().open_regular(&self.path)
    }
}

struct LoadedWorkspace {
    workspace: Workspace,
    canonical: Vec<u8>,
    consistent: bool,
    terminal: bool,
}

mod inventory;
pub use inventory::{WorkspaceInventory, WorkspaceInventoryRecord};

fn workspace_locations(
    root: &Path,
    config: &ValidatedConfig,
    disks: &DiskResources,
) -> Result<Vec<WorkspaceLocation>> {
    let mut locations = Vec::new();
    match DiskRoot::open(&root.join("workspaces")) {
        Ok(root) => locations.push(WorkspaceLocation::System(root)),
        Err(error) if workspace_absent(&error) => {}
        Err(error) => return Err(error),
    }
    for disk in config.disks.iter().filter(|disk| disk.level == 1) {
        if let Some(held) = disks.available(disk) {
            // Restore the private base-directory boundary for every consumer,
            // including cold point reads that do not enumerate the base. Keep
            // this admitted FD for all relative probes and enumeration.
            let directory = held.root.path().join("staging/workspaces");
            let root = match held.root.private_child(&directory) {
                Ok(root) => root,
                Err(error) if workspace_absent(&error) => continue,
                Err(error) => return Err(error),
            };
            locations.push(WorkspaceLocation::Primary(root));
        }
    }
    Ok(locations)
}

impl Engine {
    pub(crate) fn persist_workspace(&mut self, w: &Workspace) -> Result<()> {
        let canonical = encode_metadata(w, MAX_METADATA)?;
        self.persist_workspace_as(w, &canonical, WorkspaceRetention::Resident)
    }

    fn persist_workspace_as(
        &mut self,
        w: &Workspace,
        canonical: &[u8],
        retention: WorkspaceRetention,
    ) -> Result<()> {
        let admission = self.check_workspace_budget(w, canonical, retention)?;
        let primary = self.workspace_data_directory(w)?;
        let result = (|| {
            self.planning_context()
                .workspace_data_root(w)?
                .write_atomic(&primary.join("workspace.json"), canonical, true)?;
            write_atomic(
                &self
                    .root
                    .join("workspaces")
                    .join(&w.workspace_id)
                    .join("workspace.json"),
                canonical,
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
        directories: &[ReadSource],
    ) -> Result<bool> {
        let Some(operation) = self.state.operations.get(&request.operation_id) else {
            return Ok(false);
        };
        if crate::business::native_commit_digest(workspace, request)? == operation.request_digest {
            return Ok(true);
        }
        // Completion advances the live fence, but the wire manifest commits
        // to the original request fence. Only restore that fence for hashing;
        // files, namespace operations, branch and base must remain unchanged.
        let mut frozen = workspace.clone();
        frozen.fencing_token = request.fencing_token;
        let manifest = crate::wire::manifest_hash(&frozen)?;
        // Wire requests use a different envelope. Verify the original body,
        // not merely its caller-supplied digest string. Absent body copies do
        // not establish completion; an unreadable copy is not absence.
        let mut damage = None;
        for directory in directories {
            let Some(body) = read_workspace_evidence::<json_codec::Value>(
                directory.join("wire-request.json").open(),
                &mut damage,
            )?
            else {
                continue;
            };
            if body["operation_id"].as_str() == Some(request.operation_id.to_string().as_str())
                && body["workspace_id"].as_str() == Some(workspace.workspace_id.as_str())
                && body["branch_id"].as_str() == Some(workspace.branch_id.as_str())
                && body["expected_head"].as_str() == Some(request.expected_head.as_str())
                && body["fencing_token"].as_str()
                    == Some(request.fencing_token.to_string().as_str())
                && body["message"].as_str() == Some(request.message.as_str())
                && body["request_digest"].as_str() == Some(operation.request_digest.as_str())
                && body["sealed_manifest_hash"].as_str() == Some(manifest.as_str())
                && crate::wire::commit_request_digest(&workspace.workspace_id, &body)?
                    == operation.request_digest
            {
                return Ok(true);
            }
        }
        if let Some(error) = damage {
            return Err(error.context(
                ErrorCode::RecoveryRequired,
                "no healthy wire request evidence",
            ));
        }
        Ok(false)
    }

    pub(crate) fn workspace_data_directory(&self, workspace: &Workspace) -> Result<PathBuf> {
        self.planning_context().workspace_data_directory(workspace)
    }
    /// Read-only recovery may retain system-side evidence while the admitted
    /// resource set has no primary handle. A present resource still needs its seal.
    fn workspace_recovery_directory(&self, workspace: &Workspace) -> Result<PathBuf> {
        let repository = self.repository_for_branch(&workspace.branch_id)?;
        let disk = self.disk(&repository.primary_slot_id)?;
        if self.disk_resources.available(disk).is_some() {
            super::provenance::validate_owned_seal(
                &self.disk_resources,
                disk,
                &self.metadata_integrity,
            )?;
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

    fn workspace_locations(&self) -> Result<Vec<WorkspaceLocation>> {
        workspace_locations(&self.root, &self.config, &self.disk_resources)
    }

    fn load_workspace_mirrors(&mut self, authoritative: bool) -> Result<()> {
        let locations = self.workspace_locations()?;
        self.workspaces.clear();
        self.reset_workspace_budget();
        // The accounting index is bounded by all admitted metadata bytes, not
        // active record count. No decoded cold manifests or complete path list
        // survive this iteration; each ID is reconciled before admitting another.
        for location in &locations {
            let directory = location.directory();
            let entries = match location.root().entries(&directory) {
                Ok(entries) => entries,
                Err(error) if workspace_absent(&error) => continue,
                Err(error) => return Err(error),
            };
            for entry in entries {
                let entry = entry?;
                let path = directory.join(entry.as_path());
                if !workspace_directory(location.root(), &path)? {
                    continue;
                }
                if location
                    .root()
                    .optional_metadata(&path.join("workspace.json"))?
                    .is_none()
                {
                    continue;
                }
                let id = entry
                    .as_os_str()
                    .to_str()
                    .ok_or_else(|| recovery_required("invalid workspace directory identity"))?;
                validate_id(id, "ws")?;
                if self.accounted_workspace(id) {
                    continue;
                }
                let paths = workspace_paths(&locations, id)?;
                if paths.is_empty() {
                    continue;
                }
                let loaded = self.read_workspace_mirrors(id, &paths, authoritative)?;
                let retention = self.workspace_retention(&loaded, &paths, &locations)?;
                let admission =
                    self.check_workspace_budget(&loaded.workspace, &loaded.canonical, retention)?;
                self.account_workspace(admission);
                if !retention.is_cold() {
                    self.workspaces.insert(id.into(), loaded.workspace);
                }
            }
        }
        Ok(())
    }

    fn read_workspace_mirrors(
        &self,
        id: &str,
        paths: &[WorkspaceSource<'_>],
        authoritative: bool,
    ) -> Result<LoadedWorkspace> {
        let mut healthy = Vec::new();
        let mut damage = None;
        for path in paths {
            if let Some(workspace) = read_workspace_evidence::<Workspace>(path.open(), &mut damage)?
            {
                self.validate_workspace_mirror(id, path.path(), &workspace, authoritative)?;
                healthy.push(workspace);
            }
        }
        if healthy.is_empty() {
            return Err(missing_evidence(
                damage,
                "no healthy workspace manifest mirror",
            ));
        }
        let damaged = healthy.len() != paths.len();
        let terminal = healthy.iter().all(|workspace| {
            matches!(
                workspace.state,
                WorkspaceState::Committed | WorkspaceState::Aborted
            )
        });
        let first = canonical_json(&healthy[0])?;
        let mut consistent = true;
        for workspace in &healthy[1..] {
            consistent &= first == canonical_json(workspace)?;
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
        let canonical = canonical_json(&selected)?;
        for other in &healthy {
            if authoritative
                || (selected.state == other.state && selected.fencing_token == other.fencing_token)
            {
                if canonical != canonical_json(other)? {
                    return Err(recovery_required("workspace manifest mirrors conflict"));
                }
                continue;
            }
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
                let mut source = self
                    .planning_context()
                    .admit_staged_source(&selected.workspace_id, file)?
                    .ok_or_else(|| recovery_required("workspace staging resource is unavailable"))?
                    .open()?;
                let digest = file.content_hash.digest();
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
        Ok(LoadedWorkspace {
            workspace: selected,
            canonical,
            consistent: consistent && !damaged,
            terminal,
        })
    }

    fn workspace_retention(
        &self,
        loaded: &LoadedWorkspace,
        paths: &[WorkspaceSource<'_>],
        locations: &[WorkspaceLocation],
    ) -> Result<WorkspaceRetention> {
        let workspace = &loaded.workspace;
        let id = &workspace.workspace_id;
        let kind = if !loaded.consistent {
            None
        } else if self.empty_aborted_workspace(workspace)
            && workspace_directories_clear(locations, id, false)?
        {
            Some(WorkspaceColdKind::EmptyAbort)
        } else if loaded.terminal
            && self.published_normal_workspace(workspace)?
            && workspace_directories_clear(locations, id, true)?
        {
            Some(WorkspaceColdKind::Published)
        } else {
            None
        };
        Ok(match (workspace_mirrors(paths), kind) {
            (Some(mirrors), Some(kind)) => WorkspaceRetention::Cold {
                fingerprint: blake3::hash(&loaded.canonical),
                mirrors,
                kind,
            },
            _ => WorkspaceRetention::Resident,
        })
    }

    fn empty_aborted_workspace(&self, workspace: &Workspace) -> bool {
        workspace.state == WorkspaceState::Aborted
            && workspace.purpose == WorkspacePurpose::Normal
            && workspace.files.is_empty()
            && workspace.namespace_operations.is_empty()
    }

    pub(super) fn read_cold_workspace(
        &self,
        id: &str,
        admitted: WorkspaceRetention,
    ) -> Result<Workspace> {
        self.read_admitted_cold_workspace(id, admitted, true)
    }

    fn read_admitted_cold_workspace(
        &self,
        id: &str,
        admitted: WorkspaceRetention,
        authoritative: bool,
    ) -> Result<Workspace> {
        let WorkspaceRetention::Cold {
            fingerprint,
            mirrors,
            kind,
        } = admitted
        else {
            unreachable!("invariant: cold point reads require cold admission");
        };
        let locations = self.workspace_locations()?;
        let paths = workspace_paths(&locations, id)?;
        let loaded = self.read_workspace_mirrors(id, &paths, authoritative)?;
        // Admission already bound the publication. A point read must preserve
        // its exact manifest/mirrors, not repeat retirement against damaged
        // request copies. Replay still requires an exact healthy saved request.
        if !loaded.consistent
            || workspace_mirrors(&paths) != Some(mirrors)
            || blake3::hash(&loaded.canonical) != fingerprint
            || !workspace_directories_clear(&locations, id, kind == WorkspaceColdKind::Published)?
        {
            return Err(recovery_required(
                "cold workspace evidence changed; reopen for reconciliation",
            ));
        }
        Ok(loaded.workspace)
    }

    /// The complete published prefix admitted cold hints before replay. At an
    /// earlier replay slot, verify those same bytes/mirrors without requiring
    /// its later operation to have been installed in the live State already.
    pub(crate) fn workspace_recovery_hint(&self, id: &str) -> Result<Cow<'_, Workspace>> {
        if let Some(workspace) = self.workspaces.get(id) {
            return Ok(Cow::Borrowed(workspace));
        }
        let admitted = self
            .cold_workspace_retention(id)
            .expect("invariant: retained ID is charged");
        Ok(Cow::Owned(
            self.read_admitted_cold_workspace(id, admitted, false)?,
        ))
    }

    fn workspace_has_preparation(&self, id: &str) -> bool {
        self.commit_preparation
            .load(std::sync::atomic::Ordering::Acquire)
            || self
                .upload_preparations
                .lock()
                .expect("invariant: upload preparation lock poisoned")
                .contains(id)
    }

    fn published_normal_workspace(&self, workspace: &Workspace) -> Result<bool> {
        if workspace.purpose != WorkspacePurpose::Normal
            || !matches!(
                workspace.state,
                WorkspaceState::Committed | WorkspaceState::Aborted
            )
            || !self.state.branches.contains_key(&workspace.branch_id)
        {
            return Ok(false);
        }
        // A terminal label is not a publication proof. Reuse the existing
        // actor/workspace/action binding and native/wire digest verification.
        let directories = self
            .planning_context()
            .workspace_evidence_directories(workspace)?;
        let mut damage = None;
        let mut bound = false;
        for directory in &directories {
            let Some(request) = read_workspace_evidence::<CommitRequest>(
                directory.join("commit-request.json").open(),
                &mut damage,
            )?
            else {
                continue;
            };
            if !self.workspace_request_matches_operation(workspace, &request)
                || !self.saved_request_digest_matches(workspace, &request, &directories)?
            {
                return Ok(false);
            }
            let operation = &self.state.operations[&request.operation_id];
            let completed = request.fencing_token.checked_add(1);
            let terminal = match workspace.state {
                WorkspaceState::Committed => completed == Some(workspace.fencing_token),
                WorkspaceState::Aborted => completed
                    .and_then(|fence| fence.checked_add(1))
                    .is_some_and(|closed| workspace.fencing_token >= closed),
                WorkspaceState::Open | WorkspaceState::Sealed => false,
            };
            if !terminal
                || !matches!(
                    operation.commit_state,
                    CommitState::Published | CommitState::PublishedDegraded
                )
            {
                return Ok(false);
            }
            bound = true;
        }
        Ok(bound && damage.is_none())
    }

    pub(crate) fn persist_terminal_workspace(&mut self, workspace: &Workspace) -> Result<()> {
        let canonical = encode_metadata(workspace, MAX_METADATA)?;
        let id = &workspace.workspace_id;
        let owned = self.workspace_has_preparation(id);
        let retention = if !owned
            && let Some(WorkspaceRetention::Cold { kind, .. }) = self.cold_workspace_retention(id)
        {
            // Input is already bound to this process's admitted evidence. A
            // successful terminal write restores both mirrors in place.
            WorkspaceRetention::Cold {
                fingerprint: blake3::hash(&canonical),
                mirrors: WorkspaceMirrors::Both,
                kind,
            }
        } else if !owned {
            let published = self.published_normal_workspace(workspace)?;
            if !published && !self.empty_aborted_workspace(workspace) {
                WorkspaceRetention::Resident
            } else {
                // Classify before either write. Actual I/O failures cannot turn
                // a durable terminal result into a later derived-state refusal.
                let locations = self.workspace_locations()?;
                let paths = workspace_paths(&locations, id)?;
                let local = self.root.join("workspaces").join(id).join("workspace.json");
                let primary = self
                    .workspace_data_directory(workspace)?
                    .join("workspace.json");
                if paths
                    .iter()
                    .any(|path| path.path() != local.as_path() && path.path() != primary.as_path())
                {
                    return Err(recovery_required(
                        "workspace mirror is not on its authoritative primary",
                    ));
                }
                if workspace_mirrors(&paths) == Some(WorkspaceMirrors::Both)
                    && workspace_directories_clear(&locations, id, published)?
                {
                    WorkspaceRetention::Cold {
                        fingerprint: blake3::hash(&canonical),
                        mirrors: WorkspaceMirrors::Both,
                        kind: if published {
                            WorkspaceColdKind::Published
                        } else {
                            WorkspaceColdKind::EmptyAbort
                        },
                    }
                } else {
                    WorkspaceRetention::Resident
                }
            }
        } else {
            WorkspaceRetention::Resident
        };
        self.persist_workspace_as(workspace, &canonical, retention)?;
        if retention.is_cold() {
            self.workspaces.remove(id);
        } else {
            self.workspaces.insert(id.clone(), workspace.clone());
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
        let primary = if authoritative || self.state.branches.contains_key(&workspace.branch_id) {
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
            if let Some(primary) = &primary
                && !file.source_path.starts_with(primary)
                && !file.source_path.starts_with(&local)
            {
                return Err(Error::new(
                    storage_contract::ErrorCode::UnsafePath,
                    "workspace staging path escapes owned directory",
                ));
            }
            // Missing bytes can be an existing unavailable-content condition.
            // Permission and I/O failures are not evidence of missing content.
            match self
                .planning_context()
                .admit_staged_source(&workspace.workspace_id, file)
                .and_then(|source| source.map(|source| source.open()).transpose())
            {
                Ok(_) => {}
                Err(Error::Filesystem(platform_io::fs_safety::Error::Io(source)))
                | Err(Error::Contract(storage_contract::Error::Io { source, .. }))
                    if source.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
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
        let directories = self
            .planning_context()
            .workspace_evidence_directories(workspace)?;
        let mut damage = None;
        for directory in &directories {
            let Some(request) = read_workspace_evidence::<CommitRequest>(
                directory.join("commit-request.json").open(),
                &mut damage,
            )?
            else {
                continue;
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
                for (record, _) in self
                    .metadata_integrity
                    .observe(crate::journal::scan_at(
                        &self.disk_resources.get(member)?.root,
                        member,
                    ))?
                    .records
                {
                    if record.sequence != operation.sequence
                        || record.record_type != RecordType::Publication
                    {
                        continue;
                    }
                    let candidate: crate::journal::Candidate =
                        crate::publication_evidence::verified_object_observed_at(
                            &self.disk_resources,
                            &members,
                            "candidate",
                            &record.candidate_hash,
                            &mut |error| self.metadata_integrity.fault(error),
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
        Err(missing_evidence(
            damage,
            "workspace completion has no bound publication snapshot",
        ))
    }

    fn reconcile_workspace_receipt(&self, workspace: &mut Workspace) -> Result<bool> {
        let directories = self
            .planning_context()
            .workspace_evidence_directories(workspace)?;
        let mut completion = None;
        let mut damage = None;
        for directory in &directories {
            let Some(request) = read_workspace_evidence::<CommitRequest>(
                directory.join("commit-request.json").open(),
                &mut damage,
            )?
            else {
                continue;
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
                let fence = std::num::NonZeroU64::new(
                    storage_contract::parse_u64_decimal(
                        plan.input["workspace_fencing_token"]
                            .as_str()
                            .ok_or_else(|| {
                                recovery_required("historical workspace plan lacks fencing token")
                            })?,
                    )
                    .map_err(|source| {
                        Error::from(source).context(
                            ErrorCode::RecoveryRequired,
                            "invalid historical workspace fencing token",
                        )
                    })?,
                )
                .ok_or_else(|| recovery_required("historical workspace fencing token is zero"))?;
                let fence = fence.get();
                let next = fence
                    .checked_add(1)
                    .ok_or_else(|| recovery_required("workspace fencing generation exhausted"))?;
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
        if completion.is_none()
            && let Some(error) = damage
        {
            return Err(error.context(
                ErrorCode::RecoveryRequired,
                "no healthy workspace request evidence",
            ));
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

fn workspace_absent(error: &Error) -> bool {
    matches!(error,
        Error::Filesystem(platform_io::fs_safety::Error::Io(source))
        | Error::Contract(storage_contract::Error::Io { source, .. })
        if source.kind() == std::io::ErrorKind::NotFound)
}

fn workspace_directory(root: &DiskRoot, path: &Path) -> Result<bool> {
    let Some(metadata) = root.optional_metadata(path)? else {
        return Ok(false);
    };
    if metadata.file_type().is_symlink() {
        return Err(Error::new(
            ErrorCode::UnsafePath,
            "linked workspace directory",
        ));
    }
    Ok(metadata.is_dir())
}

fn workspace_mirrors(paths: &[WorkspaceSource<'_>]) -> Option<WorkspaceMirrors> {
    match paths {
        [single] => Some(match single.location {
            WorkspaceLocation::System(_) => WorkspaceMirrors::System,
            WorkspaceLocation::Primary(_) => WorkspaceMirrors::Primary,
        }),
        [left, right] => match (left.location, right.location) {
            (WorkspaceLocation::System(_), WorkspaceLocation::Primary(_))
            | (WorkspaceLocation::Primary(_), WorkspaceLocation::System(_)) => {
                Some(WorkspaceMirrors::Both)
            }
            _ => None,
        },
        _ => None,
    }
}

fn workspace_paths<'a>(
    locations: &'a [WorkspaceLocation],
    id: &str,
) -> Result<Vec<WorkspaceSource<'a>>> {
    let mut paths = Vec::new();
    for location in locations {
        let path = location.directory().join(id).join("workspace.json");
        match location.root().optional_metadata(&path) {
            Ok(Some(_)) => {}
            Ok(None) => continue,
            Err(error) if workspace_absent(&error) => continue,
            Err(error) => return Err(error),
        }
        if paths.len() == 2 {
            return Err(recovery_required("too many workspace mirrors"));
        }
        paths.push(location.source(path));
    }
    Ok(paths)
}

fn workspace_directories_clear(
    locations: &[WorkspaceLocation],
    id: &str,
    published: bool,
) -> Result<bool> {
    // Mirror discovery is independent, so an early unknown-material witness
    // cannot hide a third mirror. FULL generations remain on their original paths.
    for location in locations {
        let directory = location.directory().join(id);
        let entries = match location.root().entries(&directory) {
            Ok(entries) => entries,
            Err(error) if workspace_absent(&error) => continue,
            Err(error) => return Err(error),
        };
        // A directory without a manifest can still contain an older full
        // generation whose workspace must remain a payload-recovery hint.
        for entry in entries {
            let entry = entry?;
            let name = entry.as_os_str();
            if name == "workspace.json" {
                continue;
            }
            if !published
                || !(name == "commit-request.json"
                    || name == "wire-request.json"
                    || retained_full_generation(entry.as_path()))
            {
                return Ok(false);
            }
            // A known name cannot hide a directory, symlink or special file.
            location
                .root()
                .open_regular(&directory.join(entry.as_path()))?;
        }
    }
    Ok(true)
}

fn recovery_required(message: &str) -> Error {
    Error::new(storage_contract::ErrorCode::RecoveryRequired, message)
}

fn retained_full_generation(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some((item, generation)) = name
        .strip_suffix(".full")
        .and_then(|stem| stem.rsplit_once('-'))
    else {
        return false;
    };
    validate_id(item, "entry").is_ok()
        && storage_contract::parse_u64_decimal(generation).is_ok_and(|generation| generation > 0)
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
        let mut damage = None;
        for directory in self.workspace_evidence_directories(workspace)? {
            match read_workspace_evidence::<CommitRequest>(
                directory.join("commit-request.json").open(),
                &mut damage,
            )? {
                Some(saved) if canonical_json(&saved)? == expected => return Ok(()),
                Some(_) | None => {}
            }
        }
        Err(missing_evidence(
            damage,
            "no exact saved workspace commit request is available",
        ))
    }
}

impl<'a> crate::engine::PlanningContext<'a> {
    pub(crate) fn workspace_disk(&self, workspace: &Workspace) -> Result<&'a Arc<DiskHandle>> {
        let repo = self.repository_for_branch(&workspace.branch_id)?;
        self.disk_resources.get(self.disk(&repo.primary_slot_id)?)
    }

    pub(crate) fn workspace_data_root(&self, workspace: &Workspace) -> Result<&'a DiskRoot> {
        Ok(&self.workspace_disk(workspace)?.root)
    }

    pub(crate) fn workspace_data_directory(&self, w: &Workspace) -> Result<PathBuf> {
        let repo = self.repository_for_branch(&w.branch_id)?;
        let disk = self.disk(&repo.primary_slot_id)?;
        super::provenance::validate_owned_seal(self.disk_resources, disk, self.metadata_integrity)?;
        Ok(disk.root.join("staging/workspaces").join(&w.workspace_id))
    }

    fn workspace_evidence_directories(&self, workspace: &Workspace) -> Result<Vec<ReadSource>> {
        let mut directories = vec![ReadSource::external(
            self.root.join("workspaces").join(&workspace.workspace_id),
        )];
        let repo = self.repository_for_branch(&workspace.branch_id)?;
        let disk = self.disk(&repo.primary_slot_id)?;
        if let Some(owner) = self.disk_resources.available(disk) {
            directories.push(ReadSource::on_disk(
                owner,
                self.workspace_data_directory(workspace)?,
            ));
        }
        Ok(directories)
    }

    /// Decode a persisted StagedFile address within its workspace namespace.
    /// Public source paths never enter this boundary. Recovery may run before
    /// the branch/primary is known; validate_workspace_mirror separately binds
    /// an authoritative manifest to that primary after journal replay.
    fn admit_staged_source(
        &self,
        workspace: &str,
        file: &StagedFile,
    ) -> Result<Option<ReadSource>> {
        let path = &file.source_path;
        if path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "workspace staging path escapes owned directory",
            ));
        }
        let local = self.root.join("workspaces").join(workspace);
        if path.strip_prefix(&local).is_ok() {
            return Ok(Some(ReadSource::external(path.clone())));
        }
        for disk in &self.config.disks {
            let directory = disk.root.join("staging/workspaces").join(workspace);
            if path.strip_prefix(directory).is_ok() {
                return Ok(self
                    .disk_resources
                    .available(disk)
                    .map(|owner| ReadSource::on_disk(owner, path.clone())));
            }
        }
        Err(Error::new(
            ErrorCode::UnsafePath,
            "workspace staging path escapes owned directory",
        ))
    }

    pub(crate) fn staged_source(
        &self,
        workspace: &Workspace,
        file: &StagedFile,
    ) -> Result<ReadSource> {
        self.admit_staged_source(&workspace.workspace_id, file)?
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::DiskUnavailable,
                    "workspace staging disk was unavailable at admission",
                )
            })
    }
}

// These bounded redundant files may be absent or contain a torn JSON mirror.
// I/O/permission failures are not evidence of absence or recoverable corruption.
fn read_workspace_evidence<T: data_serialization::de::DeserializeOwned>(
    file: Result<fs::File>,
    damage: &mut Option<Error>,
) -> Result<Option<T>> {
    let bytes = file.and_then(|file| read_file_bytes(file, MAX_METADATA));
    match bytes.and_then(|bytes| decode_strict(&bytes).map_err(Into::into)) {
        Ok(value) => Ok(Some(value)),
        Err(Error::Filesystem(platform_io::fs_safety::Error::Io(source)))
        | Err(Error::Contract(storage_contract::Error::Io { source, .. }))
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(None)
        }
        Err(error @ Error::Contract(storage_contract::Error::Json { .. })) => {
            if damage.is_none() {
                *damage = Some(error);
            }
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn missing_evidence(damage: Option<Error>, message: &str) -> Error {
    match damage {
        Some(error) => error.context(ErrorCode::RecoveryRequired, message),
        None => recovery_required(message),
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/workspace_storage_tests.rs"]
mod tests;
