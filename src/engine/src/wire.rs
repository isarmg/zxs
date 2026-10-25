//! Frozen upload manifests and the public request digest envelope.
use crate::{disk::*, *};
use json_codec::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone)]
pub(crate) struct WireCommit {
    pub digest: String,
    pub body: Value,
    pub placements: BTreeMap<String, (String, u32)>,
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
fn text<'a>(body: &'a Value, key: &str) -> Result<&'a str> {
    body.get(key).and_then(Value::as_str).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::InvalidRequest,
            format!("missing string {key}"),
        )
    })
}
impl Engine {
    pub fn seal_workspace(&mut self, id: &str, principal: &str) -> Result<Value> {
        self.require_write()?;
        let mut w = self.workspace(id)?;
        if w.principal != principal {
            return Err(Error::new(
                storage_contract::ErrorCode::PermissionDenied,
                "workspace belongs to another principal",
            ));
        }
        if !["open", "sealed"].contains(&w.state.as_str()) {
            return Err(Error::new(
                storage_contract::ErrorCode::WorkspaceClosed,
                "workspace cannot be sealed",
            ));
        }
        self.repository_for_branch(&w.branch_id)?;
        let repository = self.repository_for_branch(&w.branch_id)?.clone();
        // Once sealed, no uploader or namespace request can mutate this generation.
        w.state = "sealed".into();
        self.persist_workspace(&w)?;
        self.workspaces.insert(id.into(), w.clone());
        let mut placement = json_codec::Map::new();
        for f in w.files.values() {
            let (primary, copies) = self.new_file_placement(&repository.repository_id, &f.path)?;
            placement.insert(
                f.item_id.clone(),
                json!({"primary_slot_id":primary,"target_copies":copies}),
            );
        }
        Ok(
            json!({"operation_id":new_id("op"),"workspace_id":id,"repository_id":repository.repository_id,"branch_id":w.branch_id,"expected_head":w.base_head,"fencing_token":w.fencing_token.to_string(),"message":"","requested_protection":"strict","file_placement":placement,"expected_topology_generation":self.state.topology_generation.to_string(),"sealed_manifest_hash":manifest_hash(&w)?}),
        )
    }
    pub fn commit_workspace_wire(
        &mut self,
        request: CommitRequest,
        body: Value,
    ) -> Result<OperationResult> {
        let plan = self.plan_workspace_commit_wire(request, body)?;
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
        mut request: CommitRequest,
        body: Value,
    ) -> Result<crate::business::WorkspaceMutation> {
        let fields = [
            "operation_id",
            "workspace_id",
            "repository_id",
            "branch_id",
            "expected_head",
            "fencing_token",
            "message",
            "requested_protection",
            "file_placement",
            "expected_topology_generation",
            "sealed_manifest_hash",
            "request_digest",
        ];
        let object = body.as_object().ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::InvalidRequest,
                "commit body must be an object",
            )
        })?;
        if object.len() != fields.len() || object.keys().any(|k| !fields.contains(&k.as_str())) {
            return Err(Error::new(
                storage_contract::ErrorCode::InvalidRequest,
                "commit body has missing or unknown fields",
            ));
        }
        let wire_digest = commit_request_digest(&request.workspace_id, &body)?;
        if let Some(existing) = self.state.operations.get(&request.operation_id)
            && (existing.request_digest != wire_digest
                || text(&body, "request_digest")? != wire_digest
                || self.operation_owner(&request.operation_id).as_deref()
                    != Some(&request.principal))
        {
            return Err(Error::new(
                storage_contract::ErrorCode::IdempotencyKeyReused,
                "completed wire request differs from its registered digest",
            ));
        }
        // Replay skips live topology/manifest preconditions, never immutable
        // binding between the authenticated request and its original body.
        let w = self.workspace(&request.workspace_id)?;
        let repo = self.repository_for_branch(&w.branch_id)?;
        if text(&body, "operation_id")? != request.operation_id
            || text(&body, "workspace_id")? != w.workspace_id
            || text(&body, "repository_id")? != repo.repository_id
            || text(&body, "branch_id")? != w.branch_id
            || text(&body, "expected_head")? != request.expected_head
            || text(&body, "fencing_token")? != request.fencing_token.to_string()
            || text(&body, "message")? != request.message
        {
            return Err(Error::new(
                storage_contract::ErrorCode::RequestBindingMismatch,
                "wire request differs from authenticated workspace input",
            ));
        }
        if self.state.operations.contains_key(&request.operation_id) {
            let workspace = self.validate_workspace_commit_replay(&request, &wire_digest)?;
            return Ok(crate::business::WorkspaceMutation {
                mutation: crate::engine::MutationPlan::Existing(
                    self.operation(&request.operation_id)?,
                ),
                workspace,
                request,
                wire: None,
            });
        }
        if text(&body, "requested_protection")? != "strict" {
            return Err(Error::new(
                storage_contract::ErrorCode::SaveModeUnsupported,
                "only strict full-file saves are supported",
            ));
        }
        if text(&body, "expected_topology_generation")?
            != self.state.topology_generation.to_string()
        {
            return Err(Error::new(
                storage_contract::ErrorCode::TopologyGenerationMismatch,
                "topology changed after request preparation",
            ));
        }
        if text(&body, "sealed_manifest_hash")? != manifest_hash(&w)? {
            return Err(Error::new(
                storage_contract::ErrorCode::SealedManifestMismatch,
                "workspace bytes or namespace order differ from the sealed request",
            ));
        }
        let digest = commit_request_digest(&w.workspace_id, &body)?;
        if text(&body, "request_digest")? != digest {
            return Err(Error::new(
                storage_contract::ErrorCode::RequestDigestMismatch,
                "server canonical request digest differs",
            ));
        }
        let mut placements = BTreeMap::new();
        let requested = body["file_placement"].as_object().ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::InvalidRequest,
                "file placement must be an object",
            )
        })?;
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
            let obj = p.as_object().ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::PlacementInvalid,
                    "placement entry is not an object",
                )
            })?;
            if obj.len() != 2
                || !obj.contains_key("primary_slot_id")
                || !obj.contains_key("target_copies")
            {
                return Err(Error::new(
                    storage_contract::ErrorCode::PlacementInvalid,
                    "invalid placement fields",
                ));
            }
            let primary = text(p, "primary_slot_id")?;
            let copies = p["target_copies"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::PlacementInvalid,
                        "target copies must be a positive bounded integer",
                    )
                })?;
            self.route(primary, copies)?;
            placements.insert(item.clone(), (primary.into(), copies));
        }
        if w.state != "sealed" && !self.state.operations.contains_key(&request.operation_id) {
            return Err(Error::new(
                storage_contract::ErrorCode::WorkspaceNotSealed,
                "seal the immutable upload manifest before commit",
            ));
        }
        request.request_digest = Some(digest.clone());
        self.plan_workspace_commit_inner(
            request,
            Some(WireCommit {
                digest,
                body,
                placements,
            }),
        )
    }
    pub fn version_health(&self, version: &str) -> Result<String> {
        let v = self.state.versions.get(version).ok_or_else(|| {
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
        let mut healthy = 0;
        for slot in &p.route_slot_ids {
            let d = self.disk(slot)?;
            if validate_seal(&self.config, d).is_ok() {
                let dir = version_dir(d, &v.history_tree_id, version);
                if crate::journal::verify_local_publication(&dir, v).is_ok()
                    && verify_payload(&dir, v).is_ok()
                {
                    healthy += 1;
                }
            }
        }
        Ok(if healthy == 0 {
            "UNAVAILABLE"
        } else if healthy < p.target_copies as usize {
            "DEGRADED"
        } else {
            "HEALTHY"
        }
        .into())
    }
}
