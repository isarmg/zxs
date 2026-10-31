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

#[test]
fn context_reservation_bounds_follow_closed_id_families() {
    for (prefix, bound) in [
        ("ver", CONTEXT_NODE_BYTES),
        ("fnode", CONTEXT_NODE_BYTES),
        ("fmeta", CONTEXT_NODE_BYTES),
        ("br", CONTEXT_BRANCH_BYTES),
        ("dbr", CONTEXT_ORIGIN_BYTES),
    ] {
        let maximum = format!("{prefix}_{}", "f".repeat(32));
        validate_id(&maximum, prefix).unwrap();
        assert!(maximum.len() <= bound);
        assert!(validate_id(&format!("{maximum}f"), prefix).is_err());
        assert!(validate_id(&format!("{prefix}_{}", "g".repeat(32)), prefix).is_err());
    }
    assert_eq!(CONTEXT_NODE_BYTES, 38);
    assert_eq!(CONTEXT_BRANCH_BYTES, 35);
    assert_eq!(CONTEXT_ORIGIN_BYTES, 36);
}

#[test]
fn exact_context_widths_reserve_every_state_and_maximum_future_fence() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("state"),
        Config::development([temp.path().join("disk")]),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("reservation").unwrap(),
            "P01",
            1,
            "bench",
            OperationId::generate(),
        )
        .unwrap();
    let mut workspace = engine
        .create_workspace(&repo.main_branch_id, "bench")
        .unwrap();
    let key = format!("{}/file", repo.repository_id);
    workspace.selected_file_contexts.insert(
        key.clone(),
        FileContext {
            node_id: None,
            selection_generation: 0,
            branch_id: format!("br_{}", "f".repeat(32)),
            inherited_from: None,
        },
    );
    let initial = engine
        .workspace_metadata_charge(&workspace, canonical_json(&workspace).unwrap().len())
        .unwrap();
    for prefix in ["ver", "fnode", "fmeta"] {
        for state in [
            WorkspaceState::Open,
            WorkspaceState::Sealed,
            WorkspaceState::Committed,
            WorkspaceState::Aborted,
        ] {
            let mut future = workspace.clone();
            future.state = state;
            future.fencing_token = u64::MAX;
            let context = future.selected_file_contexts.get_mut(&key).unwrap();
            context.node_id = Some(format!("{prefix}_{}", "f".repeat(32)));
            context.selection_generation = u64::MAX;
            context.inherited_from = Some(format!("dbr_{}", "f".repeat(32)));
            let actual = canonical_json(&future).unwrap().len();
            let charge = engine.workspace_metadata_charge(&future, actual).unwrap();
            assert_eq!(charge, initial);
            assert!(charge >= actual as u64);
        }
    }
    // Invalid oversized retained material cannot gain a smaller charge merely
    // because valid future IDs have narrower bounds. Boundary validators reject
    // these IDs; accounting must still never hide their actual bytes.
    let context = workspace.selected_file_contexts.get_mut(&key).unwrap();
    context.node_id = Some("x".repeat(80));
    context.branch_id = "y".repeat(80);
    context.inherited_from = Some("z".repeat(80));
    let actual = canonical_json(&workspace).unwrap().len();
    assert!(
        engine
            .workspace_metadata_charge(&workspace, actual)
            .unwrap()
            >= actual as u64
    );
}
