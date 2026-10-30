//! Frozen upload manifests and the public request digest envelope.
use crate::*;
use json_codec::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone)]
pub(crate) struct WireCommit {
    pub digest: String,
    pub body: Value,
    pub placements: BTreeMap<String, (String, u32)>,
}
// A single-use proof for this planning State only. It is consumed by planning,
// never stored in PreparedWorkspaceCommit; finish rechecks the live receipt.
pub(crate) struct ReceiptCheckedEnvelope<'state> {
    envelope: PublicCommitEnvelope,
    _state: &'state State,
}
pub(crate) fn admit_public_commit<'state>(
    envelope: PublicCommitEnvelope,
    request: &CommitRequest,
    state: &'state State,
) -> Result<ReceiptCheckedEnvelope<'state>> {
    if let Some(existing) = state.operations.get(&request.operation_id)
        && (existing.request_digest != envelope.digest()
            || envelope.claimed_digest() != envelope.digest()
            || state
                .operation_principals
                .get(&request.operation_id)
                .map(String::as_str)
                != Some(&request.principal))
    {
        return Err(Error::new(
            ErrorCode::IdempotencyKeyReused,
            "completed wire request differs from its registered digest",
        ));
    }
    Ok(ReceiptCheckedEnvelope {
        envelope,
        _state: state,
    })
}

pub fn manifest_hash(w: &Workspace) -> Result<String> {
    object_hash(
        "workspace-manifest",
        &json!({"workspace_id":w.workspace_id,"branch_id":w.branch_id,"base_head":w.base_head,"fencing_token":w.fencing_token.to_string(),"items":w.files.values().map(|f|json!({"item_id":f.item_id,"path":f.path,"logical_size":f.size.to_string(),"content_hash":f.content_hash,"upload_generation":f.dirty_generation.to_string()})).collect::<Vec<_>>(),"namespace_operations":w.namespace_operations}),
    ).map_err(Into::into)
}
pub fn commit_request_digest(workspace: &str, body: &Value) -> Result<String> {
    request_digest(
        "POST",
        "/api/workspaces/{workspace}/commit",
        json!({"workspace":workspace}),
        json!({}),
        body,
    )
    .map_err(Into::into)
}
impl Engine {
    pub fn seal_workspace(&mut self, id: &str, principal: &str) -> Result<SealedWorkspace> {
        self.require_write()?;
        let mut w = self.workspace(id)?;
        if w.principal != principal {
            return Err(Error::new(
                storage_contract::ErrorCode::PermissionDenied,
                "workspace belongs to another principal",
            ));
        }
        if !matches!(w.state, WorkspaceState::Open | WorkspaceState::Sealed) {
            return Err(Error::new(
                storage_contract::ErrorCode::WorkspaceClosed,
                "workspace cannot be sealed",
            ));
        }
        let repository = self.repository_for_branch(&w.branch_id)?.clone();
        // Once sealed, no uploader or namespace request can mutate this generation.
        w.state = WorkspaceState::Sealed;
        self.persist_workspace(&w)?;
        self.workspaces.insert(id.into(), w.clone());
        let mut placement = BTreeMap::new();
        for f in w.files.values() {
            let (primary, copies) = self.new_file_placement(&repository.repository_id, &f.path)?;
            placement.insert(
                f.item_id.clone(),
                FilePlacement {
                    primary_slot_id: primary,
                    target_copies: copies,
                },
            );
        }
        let sealed_manifest_hash = manifest_hash(&w)?;
        Ok(SealedWorkspace {
            operation_id: OperationId::generate(),
            workspace_id: id.into(),
            repository_id: repository.repository_id,
            branch_id: w.branch_id,
            expected_head: w.base_head,
            fencing_token: w.fencing_token,
            message: String::new(),
            requested_protection: "strict",
            file_placement: placement,
            expected_topology_generation: self.state.topology_generation,
            sealed_manifest_hash,
        })
    }

    pub fn commit_workspace_wire(
        &mut self,
        request: CommitRequest,
        envelope: PublicCommitEnvelope,
    ) -> Result<OperationResult> {
        let plan = self.plan_workspace_commit_wire(request, envelope)?;
        self.persist_workspace_commit_request(&plan)?;
        let result = match plan.mutation {
            crate::engine::MutationPlan::Existing(result) => return Ok(result),
            crate::engine::MutationPlan::Prepared {
                next,
                prepared,
                result,
            } => {
                self.persist(*next, prepared, result.clone())?;
                result
            }
        };
        self.complete_workspace_commit(plan.workspace, result)
    }
    pub(crate) fn plan_workspace_commit_wire(
        &self,
        request: CommitRequest,
        envelope: PublicCommitEnvelope,
    ) -> Result<crate::business::WorkspaceMutation> {
        let envelope = admit_public_commit(envelope, &request, &self.state)?;
        let workspace = self.workspace(&request.workspace_id)?;
        let replay = if self.state.operations.contains_key(&request.operation_id) {
            Some(self.operation_record(request.operation_id)?.into_result())
        } else {
            None
        };
        self.planning_context()
            .plan_workspace_commit_wire(workspace, request, envelope, replay)
    }
    pub fn version_health(&self, version: &str) -> Result<String> {
        self.state.versions.get(version).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::VersionNotFound,
                "version not found",
            )
        })?;
        if let Some(state) = self.state.content_states.get(version)
            && (*state == ContentState::Purged || *state == ContentState::Migrated)
        {
            return Ok("PURGED".into());
        }
        let p = self.state.placements.get(version).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::PlacementNotFound,
                "placement not found",
            )
        })?;
        Ok(self
            .prepare_content(version)?
            .health(p.target_copies)
            .as_str()
            .into())
    }
}

impl crate::engine::PlanningContext<'_> {
    pub(crate) fn plan_workspace_commit_wire(
        &self,
        workspace: Workspace,
        mut request: CommitRequest,
        envelope: ReceiptCheckedEnvelope<'_>,
        replay: Option<OperationResult>,
    ) -> Result<crate::business::WorkspaceMutation> {
        let envelope = envelope.envelope;
        let wire_digest = envelope.digest().to_owned();
        // Replay skips live topology/manifest preconditions, never immutable
        // binding between the authenticated request and its original body.
        let w = workspace;
        let repo = self.repository_for_branch(&w.branch_id)?;
        if envelope.route_workspace() != request.workspace_id
            || envelope.operation_id() != request.operation_id
            || envelope.workspace_id() != w.workspace_id
            || envelope.repository_id() != repo.repository_id
            || envelope.branch_id() != w.branch_id
            || envelope.expected_head() != request.expected_head
            || envelope.fencing_token() != request.fencing_token
            || envelope.message() != request.message
        {
            return Err(Error::new(
                storage_contract::ErrorCode::RequestBindingMismatch,
                "wire request differs from authenticated workspace input",
            ));
        }
        if let Some(result) = replay {
            self.validate_workspace_commit_replay(&w, &request, &wire_digest)?;
            let workspace = w;
            return Ok(crate::business::WorkspaceMutation {
                mutation: crate::engine::MutationPlan::Existing(result),
                workspace,
                request,
                wire: None,
            });
        }
        if envelope.requested_protection() != "strict" {
            return Err(Error::new(
                storage_contract::ErrorCode::SaveModeUnsupported,
                "only strict full-file saves are supported",
            ));
        }
        if envelope.expected_topology_generation() != self.state.topology_generation.to_string() {
            return Err(Error::new(
                storage_contract::ErrorCode::TopologyGenerationMismatch,
                "topology changed after request preparation",
            ));
        }
        if envelope.sealed_manifest_hash() != manifest_hash(&w)? {
            return Err(Error::new(
                storage_contract::ErrorCode::SealedManifestMismatch,
                "workspace bytes or namespace order differ from the sealed request",
            ));
        }
        let digest = wire_digest;
        if envelope.claimed_digest() != digest {
            return Err(Error::new(
                storage_contract::ErrorCode::RequestDigestMismatch,
                "server canonical request digest differs",
            ));
        }
        let mut placements = BTreeMap::new();
        let requested = envelope.file_placement();
        if requested.len() != w.files.len() {
            return Err(Error::new(
                storage_contract::ErrorCode::PlacementInvalid,
                "placement must name every sealed item exactly once",
            ));
        }
        for (item, p) in requested {
            if !w.files.contains_key(item) {
                return Err(Error::new(
                    storage_contract::ErrorCode::PlacementInvalid,
                    "placement contains an unknown item",
                ));
            }
            let primary = &p.primary_slot_id;
            let copies = p.target_copies;
            self.route(primary, copies)?;
            placements.insert(item.clone(), (primary.clone(), copies));
        }
        if w.state != WorkspaceState::Sealed
            && !self.state.operations.contains_key(&request.operation_id)
        {
            return Err(Error::new(
                storage_contract::ErrorCode::WorkspaceNotSealed,
                "seal the immutable upload manifest before commit",
            ));
        }
        request.request_digest = Some(digest.clone());
        self.plan_workspace_commit_inner(
            w,
            request,
            Some(WireCommit {
                digest,
                body: envelope.into_body(),
                placements,
            }),
            None,
        )
    }
}
