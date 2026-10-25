use super::*;

impl Engine {
    pub(crate) fn retained_workspace_metadata_bytes(&self) -> u64 {
        self.workspace_budget.bytes
    }

    pub(crate) fn assert_workspace_budget_consistent(&self) {
        let mut expected = WorkspaceBudget::default();
        for workspace in self.workspaces.values() {
            let charge = self.workspace_metadata_charge(workspace).unwrap();
            expected.records.insert(
                workspace.workspace_id.clone(),
                (workspace.principal.clone(), charge),
            );
            expected.bytes += charge;
            let principal = expected
                .principals
                .entry(workspace.principal.clone())
                .or_default();
            principal.0 += 1;
            principal.1 += charge;
        }
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
