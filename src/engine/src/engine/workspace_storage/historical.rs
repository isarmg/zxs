//! Request-scoped completion bindings from the published plan root.
use super::*;

pub(super) struct HistoricalWorkspaceCandidate {
    pub fingerprint: blake3::Hash,
    id: String,
    principal: String,
    source: String,
    completion: Option<HistoricalCompletion>,
}

#[derive(PartialEq, Eq)]
struct HistoricalCompletion {
    fence: u64,
    manifest_hash: [u8; 64],
}

impl HistoricalWorkspaceCandidate {
    pub fn new(workspace: &Workspace, canonical: &[u8]) -> Self {
        Self {
            fingerprint: blake3::hash(canonical),
            id: workspace.workspace_id.clone(),
            principal: workspace.principal.clone(),
            source: workspace.base_head.clone(),
            completion: None,
        }
    }

    pub fn observe(&mut self, plan: &Plan) -> Result<()> {
        if plan.kind != PlanKind::HistoricalSave
            || plan.state != PlanState::Completed
            || plan.principal != self.principal
            || plan.input["workspace_id"].as_str() != Some(self.id.as_str())
            || plan.input["source_commit_id"].as_str() != Some(self.source.as_str())
        {
            return Ok(());
        }
        let fence = storage_contract::parse_u64_decimal(
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
        })?;
        if fence == 0 || fence.checked_add(1).is_none() {
            return Err(recovery_required(
                "historical workspace fencing token is invalid",
            ));
        }
        let manifest_hash = plan.input["sealed_manifest_hash"].as_str().ok_or_else(|| {
            recovery_required("historical workspace plan lacks sealed manifest hash")
        })?;
        if manifest_hash.len() != 64 {
            return Err(recovery_required("invalid historical manifest hash length"));
        }
        let manifest_hash: [u8; 64] = manifest_hash
            .as_bytes()
            .try_into()
            .expect("invariant: manifest digest length was admitted");
        if !manifest_hash
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        {
            return Err(recovery_required("invalid historical manifest hash"));
        }
        let completion = HistoricalCompletion {
            fence,
            manifest_hash,
        };
        if let Some(previous) = &self.completion {
            if previous != &completion {
                return Err(recovery_required(
                    "historical workspace has conflicting completed plans",
                ));
            }
        } else {
            self.completion = Some(completion);
        }
        Ok(())
    }

    pub fn has_completion(&self) -> bool {
        self.completion.is_some()
    }

    pub fn reconcile(&self, workspace: &mut Workspace) -> Result<bool> {
        let Some(completion) = &self.completion else {
            return Ok(false);
        };
        let current_fence = workspace.fencing_token;
        workspace.fencing_token = completion.fence;
        let manifest_hash = crate::wire::manifest_hash(workspace);
        workspace.fencing_token = current_fence;
        if manifest_hash?.as_bytes() != completion.manifest_hash {
            return Ok(false);
        }
        completion.complete(workspace)?;
        Ok(true)
    }

    pub fn reconcile_mirrors(&self, mirrors: &mut [Workspace]) -> Result<bool> {
        let published = self.reconcile(&mut mirrors[0])?;
        if published {
            let completion = self
                .completion
                .as_ref()
                .expect("invariant: only a matched completion reconciles a workspace");
            for workspace in &mut mirrors[1..] {
                completion.complete(workspace)?;
            }
        }
        Ok(published)
    }
}

impl HistoricalCompletion {
    fn complete(&self, workspace: &mut Workspace) -> Result<()> {
        let next = self.fence + 1;

        match workspace.state {
            WorkspaceState::Aborted => {
                if !next
                    .checked_add(1)
                    .is_some_and(|closed| workspace.fencing_token >= closed)
                {
                    return Err(recovery_required(
                        "historical workspace abort precedes its published completion",
                    ));
                }
                return Ok(());
            }
            WorkspaceState::Sealed | WorkspaceState::Committed => {
                if workspace.fencing_token != self.fence && workspace.fencing_token != next {
                    return Err(recovery_required(
                        "historical workspace plan conflicts with retained fence or state",
                    ));
                }
            }
            WorkspaceState::Open => {
                return Err(recovery_required(
                    "historical workspace plan conflicts with retained fence or state",
                ));
            }
        }
        workspace.state = WorkspaceState::Committed;
        workspace.fencing_token = next;
        Ok(())
    }
}
