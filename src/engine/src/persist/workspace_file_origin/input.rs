//! Complete original logical input when the actual before/after carrier might
//! otherwise make a mixed or substituted request indistinguishable. No spool
//! path, open descriptor, credentials, sessions or body bytes are retained.
use super::*;
use super::subject::{Binding, Fences};
use name_policy::PortablePath;
use storage_contract::{BranchId, ContentHash, Generation, VersionId};

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Item {
    item: EntryId,
    path: PortablePath,
    hash: ContentHash,
    #[serialization(with = "storage_contract::u64_string")]
    size: u64,
    generation: Generation,
    base: Option<VersionId>,
}
#[derive(Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Command { Native {}, Wire { body: json_codec::Value } }

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Source {
    request: CommitRequest,
    branch: BranchId,
    files: Vec<Item>,
    namespace_operations: Vec<crate::NamespaceOperation>,
    selection: Fences,
    command: Command,
}

impl Source {
    pub(super) fn capture(
        workspace: &Workspace, request: &CommitRequest, wire: Option<&crate::wire::WireCommit>,
        binding: &Binding,
    ) -> Result<Self> {
        if workspace.purpose != crate::WorkspacePurpose::Normal
            || workspace.workspace_id != request.workspace_id || workspace.base_head != request.expected_head
            || workspace.principal != request.principal || workspace.fencing_token != request.fencing_token
        { return Err(invalid()); }
        let mut request = request.clone();
        request.request_digest = None;
        let files = workspace.files.iter().map(|(key, file)| {
            if key != &file.item_id { return Err(invalid()); }
            Ok(Item {
                item: file.item_id.parse()?, path: file.path.clone(), hash: file.content_hash.clone(),
                size: file.size, generation: Generation::new(file.dirty_generation),
                base: file.base_version_id.as_deref().map(str::parse).transpose()?,
            })
        }).collect::<Result<Vec<_>>>()?;
        Ok(Self {
            request, branch: workspace.branch_id.parse()?, files,
            namespace_operations: workspace.namespace_operations.clone(),
            selection: Fences::captured(workspace, binding)?,
            command: wire.map_or(Command::Native {}, |wire| Command::Wire { body: wire.body.clone() }),
        })
    }

    pub(super) fn require_digest(&self, result: &OperationResult, actor: &str) -> Result<()> {
        let req = &self.request;
        if req.operation_id != result.operation_id || req.principal != actor || req.request_digest.is_some()
            || result.branch_id.as_deref() != Some(self.branch.to_string().as_str())
            || self.files.is_empty() || self.files.iter().any(|file| file.generation.get() == 0)
            || self.files.windows(2).any(|rows| rows[0].item >= rows[1].item)
        { return Err(invalid()); }
        let _: storage_contract::Id<crate::persist::namespace_directory::workspace::WorkspaceIdentity> =
            req.workspace_id.parse()?;
        let _: storage_contract::CommitId = req.expected_head.parse()?;
        match &self.command {
            Command::Native {} => {
                let input = crate::business::sealed_workspace_commit_input(
                    &req.workspace_id, &req.expected_head, req, &self.namespace_operations,
                    self.files.iter().map(|file| crate::business::native_manifest_item(
                        file.item, &file.path, &file.hash, file.size, file.generation.get(),
                    )).collect(),
                )?;
                if crate::Engine::native_request_digest(KIND, actor, &input)? != result.request_digest {
                    return Err(invalid());
                }
            }
            Command::Wire { body } => {
                let envelope = crate::PublicCommitEnvelope::parse(&req.workspace_id, body.clone())
                    .map_err(|_| invalid())?;
                let manifest = crate::wire::sealed_manifest_hash(
                    &req.workspace_id, &self.branch.to_string(), &req.expected_head, req.fencing_token,
                    self.files.iter().map(|item| crate::wire::sealed_manifest_item(
                        item.item, &item.path, item.size, &item.hash, item.generation.get(),
                    )).collect(), &self.namespace_operations,
                )?;
                if envelope.operation_id() != req.operation_id || envelope.workspace_id() != req.workspace_id
                    || envelope.branch_id() != self.branch.to_string() || envelope.expected_head() != req.expected_head
                    || envelope.fencing_token() != req.fencing_token || envelope.message() != req.message
                    || envelope.requested_protection() != "strict" || envelope.sealed_manifest_hash() != manifest
                    || envelope.digest() != result.request_digest || envelope.claimed_digest() != result.request_digest
                    || envelope.file_placement().len() != self.files.len()
                    || self.files.iter().any(|item| !envelope.file_placement().contains_key(&item.item.to_string()))
                { return Err(invalid()); }
            }
        }
        Ok(())
    }

    pub(super) fn require_original(&self, result: &OperationResult, actor: &str, binding: &Binding) -> Result<()> {
        self.require_digest(result, actor)?;
        if self.branch != binding.branch || self.selection != binding.fences { return Err(invalid()); }
        if let Command::Wire { body } = &self.command {
            let envelope = crate::PublicCommitEnvelope::parse(&self.request.workspace_id, body.clone())
                .map_err(|_| invalid())?;
            if envelope.repository_id() != binding.target.repository.to_string()
                || storage_contract::parse_u64_decimal(envelope.expected_topology_generation())? != binding.topology.get()
            { return Err(invalid()); }
            for file in self.files.iter().filter(|f| f.size == binding.size && f.hash == binding.hash
                && f.path.key() == binding.target.path.key()) {
                let p = envelope.file_placement().get(&file.item.to_string()).ok_or_else(invalid)?;
                if p.primary_slot_id != binding.slot || p.target_copies != 1 { return Err(invalid()); }
            }
        }
        Ok(())
    }

    pub(super) fn require_message(&self, state: &State, result: &OperationResult) -> Result<()> {
        let commit = result.commit_id.as_ref().and_then(|id| state.commits.get(id)).ok_or_else(invalid)?;
        if commit.message != self.request.message { return Err(invalid()); }
        Ok(())
    }

    pub(super) fn require_base(&self, state: &State, binding: &Binding) -> Result<()> {
        let base = state.commits.get(&self.request.expected_head).ok_or_else(invalid)?;
        let mut files = base.entries.iter().filter(|e| e.kind == crate::NamespaceEntryKind::File);
        let row = files.next().ok_or_else(invalid)?;
        if files.next().is_some() || base.history_tree_id != binding.history.to_string()
            || row.entry_id != binding.target.entry.to_string() || row.file_id != binding.target.file.to_string()
            || row.version_id != binding.base_version.to_string() || base.pending_file_versions.contains_key(&row.file_id)
        { return Err(invalid()); }
        Ok(())
    }

    pub(super) fn proves_created(&self, binding: &Binding) -> Result<bool> {
        if !self.namespace_operations.is_empty() || self.files.len() != 1 { return Ok(false); }
        let file = &self.files[0];
        if file.path.key() != binding.target.path.key() || file.hash != binding.hash
            || file.size != binding.size || file.base != Some(binding.base_version)
        { return Err(invalid()); }
        Ok(true)
    }
}
