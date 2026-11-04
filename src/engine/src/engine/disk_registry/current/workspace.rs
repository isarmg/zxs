//! A permanent publication proves when a workspace joined a registry view.
//! Local manifests and caller-provided fencing values never mint this authority.
use super::*;
use crate::{CommitState, OperationId, object_hash};
use data_serialization::{Deserialize, Serialize};

const ADMISSION_KIND: &str = "workspace.namespace.admit";

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct AdmissionRecord {
    registry: DiskRegistryCommitment,
    mask_hash: Option<String>,
    workspace_id: String,
    branch_id: String,
    head: String,
    principal: String,
    #[serialization(with = "storage_contract::u64_string")]
    fencing_token: u64,
    entries_hash: String,
}

impl WorkspaceNamespaceBaseline {
    fn record(&self) -> AdmissionRecord {
        AdmissionRecord {
            registry: self.registry.clone(),
            mask_hash: self.mask_hash.clone(),
            workspace_id: self.workspace_id.clone(),
            branch_id: self.branch_id.clone(),
            head: self.head.clone(),
            principal: self.principal.clone(),
            fencing_token: self.fencing_token,
            entries_hash: self.entries_hash.clone(),
        }
    }
}

impl Engine {
    pub(crate) fn admit_namespace_workspace(
        &mut self,
        workspace: &Workspace,
    ) -> Result<Option<WorkspaceNamespaceBaseline>> {
        let Some(view) = &self.current_namespaces else {
            return Ok(None);
        };
        let baseline = view.workspace_baseline(&self.state, workspace)?;
        let record = baseline.record();
        let input = json_codec::to_value(&record)?;
        self.mutate(
            ADMISSION_KIND,
            &workspace.principal,
            OperationId::generate()?,
            input,
            |_, _, _| Ok(json_codec::json!({"namespace_admission": record})),
        )?;
        Ok(Some(baseline))
    }

    /// Called after ordinary published State/recovery admission. Old workspaces
    /// remain readable evidence, but only matching authenticated admission
    /// records can authorize current writes. Nothing is inferred from a manifest.
    pub(in crate::engine) fn restore_namespace_workspace_baselines(&mut self) -> Result<()> {
        let Some(view) = &self.current_namespaces else {
            return Ok(());
        };
        let mut baselines = BTreeMap::new();
        for (id, result) in self.state.operations.iter() {
            if result.kind != ADMISSION_KIND || result.sequence <= view.activation_sequence {
                continue;
            }
            if !matches!(
                result.commit_state,
                CommitState::Published | CommitState::PublishedDegraded
            ) {
                continue;
            }
            let value = result.result.get("namespace_admission").ok_or_else(|| {
                Error::new(
                    ErrorCode::PublicationInvalid,
                    "published workspace admission record is absent",
                )
            })?;
            let record: AdmissionRecord = json_codec::from_value(value.clone())?;
            if record.registry != view.registry || record.mask_hash != view.mask_hash {
                continue;
            }
            if self.state.operation_principals.get(id) != Some(&record.principal)
                || result.request_digest
                    != Self::native_request_digest(
                        ADMISSION_KIND,
                        &record.principal,
                        &json_codec::to_value(&record)?,
                    )?
            {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "workspace admission owner or request binding differs",
                ));
            }
            let scope = view.branches.get(&record.branch_id).ok_or_else(stale)?;
            let commit = self.state.commits.get(&record.head).ok_or_else(stale)?;
            let branch = self
                .state
                .branches
                .get(&record.branch_id)
                .ok_or_else(stale)?;
            if commit.history_tree_id != branch.history_tree_id {
                return Err(stale());
            }
            let entries = if record.head == scope.source_head {
                scope.source_entries.as_slice()
            } else {
                commit.entries.as_slice()
            };
            if object_hash("current-workspace-namespace", entries)? != record.entries_hash {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "workspace admission namespace digest differs",
                ));
            }
            let baseline = WorkspaceNamespaceBaseline {
                registry: record.registry,
                mask_hash: record.mask_hash,
                workspace_id: record.workspace_id.clone(),
                branch_id: record.branch_id,
                head: record.head,
                principal: record.principal,
                fencing_token: record.fencing_token,
                entries_hash: record.entries_hash,
            };
            if baselines.insert(record.workspace_id, baseline).is_some() {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "workspace has conflicting namespace admission receipts",
                ));
            }
        }
        self.workspace_namespace_baselines = baselines;
        Ok(())
    }
}
