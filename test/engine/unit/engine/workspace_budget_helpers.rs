use super::*;

impl Engine {
    pub(crate) fn retained_workspace_metadata_bytes(&self) -> u64 {
        self.workspace_budget.bytes
    }

    pub(crate) fn assert_workspace_budget_consistent(&self) {
        let mut expected = WorkspaceBudget::default();
        for id in self.workspace_budget.records.keys() {
            let workspace = self.workspace(id).unwrap();
            let canonical = canonical_json(&workspace).unwrap();
            let charge = self
                .workspace_metadata_charge(&workspace, canonical.len())
                .unwrap();
            let resident = self.workspaces.contains_key(id);
            let retention = if resident {
                WorkspaceRetention::Resident
            } else {
                match self.workspace_budget.records[id].retention {
                    WorkspaceRetention::Cold { mirrors, kind, .. } => WorkspaceRetention::Cold {
                        fingerprint: blake3::hash(&canonical),
                        mirrors,
                        kind,
                    },
                    WorkspaceRetention::Resident => panic!("missing resident workspace"),
                }
            };
            expected.records.insert(
                id.clone(),
                WorkspaceCharge {
                    principal: workspace.principal.clone(),
                    bytes: charge,
                    retention,
                },
            );
            expected.bytes += charge;
            expected.resident += usize::from(resident);
            let principal = expected.principals.entry(workspace.principal).or_default();
            principal.resident += usize::from(resident);
            principal.cold += usize::from(!resident);
            principal.bytes += charge;
        }
        assert_eq!(self.workspace_budget.resident, expected.resident);
        assert_eq!(self.workspace_budget.records, expected.records);
        assert_eq!(self.workspace_budget.principals, expected.principals);
        assert_eq!(self.workspace_budget.bytes, expected.bytes);
    }
}

#[test]
fn lifecycle_reservation_cannot_mask_the_durable_manifest_boundary() {
    let limit = crate::disk::MAX_METADATA;
    assert_eq!(
        checked_metadata_charge(limit - 1, limit).unwrap(),
        limit as u64
    );
    assert_eq!(
        checked_metadata_charge(1, limit + 1).unwrap_err().code(),
        storage_contract::ErrorCode::ResourceLimit
    );
    assert_eq!(
        checked_metadata_charge(limit + 1, 1).unwrap_err().code(),
        storage_contract::ErrorCode::ResourceLimit
    );
    let config = Config::development(["/draft/only".into()]);
    assert!(config.max_retained_workspace_metadata_bytes_per_principal >= limit as u64);
    assert!(
        config.max_retained_workspace_metadata_bytes
            >= config.max_retained_workspace_metadata_bytes_per_principal
    );
}
