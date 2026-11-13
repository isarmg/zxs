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
    names: BTreeMap<storage_contract::FileId, name_policy::Name>,
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
            names: self.names.clone(),
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
            if named_entries_hash(entries, &record.names)? != record.entries_hash {
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
                names: record.names,
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

/// The receipt carries the names at admission, not today's names. Exact file
/// coverage prevents a supplied partial map from silently preserving old aliases.
pub(super) fn named_entries_hash(
    entries: &[Entry],
    names: &BTreeMap<storage_contract::FileId, name_policy::Name>,
) -> Result<String> {
    let files = entries.iter().filter(|entry| entry.kind == crate::NamespaceEntryKind::File)
        .map(|entry| Ok(entry.file_id.parse::<storage_contract::FileId>()?))
        .collect::<Result<BTreeSet<_>>>()?;
    if files.iter().ne(names.keys()) {
        return Err(crate::persist::path_changes::invalid());
    }
    let mut projected = entries.to_vec();
    crate::business::namespace::container_name::apply_file_names(&mut projected, names)?;
    Ok(object_hash("current-workspace-namespace", &projected)?)
}

/// A new durable receipt must describe the previous admitted Current, not a
/// namespace/name map asserted by the receipt itself. Recovery of old receipts
/// uses their original source and saved names only after this publication proof.
pub(super) fn verify_admissions(
    view: &CurrentNamespaces,
    previous: &State,
    next: &State,
) -> Result<()> {
    for (id, result) in next.operations.iter() {
        if result.kind != ADMISSION_KIND || previous.operations.contains_key(id) {
            continue;
        }
        let value = result.result.get("namespace_admission")
            .ok_or_else(crate::persist::path_changes::invalid)?;
        let record: AdmissionRecord = json_codec::from_value(value.clone())?;
        let scope = view.writable_scope(previous, &record.branch_id)?;
        let names = crate::business::namespace::container_name::current_names(
            previous, scope.entries.as_ref(),
        )?;
        if record.registry != view.registry || record.mask_hash != view.mask_hash
            || record.head != scope.head || record.names != names
            || record.entries_hash != named_entries_hash(scope.entries.as_ref(), &names)?
            || next.operation_principals.get(id) != Some(&record.principal)
            || result.request_digest != Engine::native_request_digest(
                ADMISSION_KIND, &record.principal, &json_codec::to_value(&record)?,
            )?
        {
            return Err(crate::persist::path_changes::invalid());
        }
    }
    Ok(())
}
