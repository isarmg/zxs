use super::*;
use std::cell::Cell;

fn engine_with_two_routes(root: &std::path::Path) -> (Engine, Repository) {
    let mut config = Config::development([root.join("p"), root.join("b"), root.join("c")]);
    for (slot, level, upstream) in [("P02", 1, None), ("B02", 2, Some("P02"))] {
        config.disks.push(DiskConfig {
            slot_id: slot.into(),
            disk_id: new_id("disk"),
            root: root.join(slot),
            level,
            upstream_slot_id: upstream.map(str::to_owned),
            metadata_member: false,
            binding_generation: 1,
            capacity_bytes: None,
        });
    }
    let mut engine = Engine::initialize(&root.join("state"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("read kernels").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    (engine, repo)
}

#[test]
fn captured_config_route_and_real_planner_share_values_and_errors() {
    let root = tempfile::tempdir().unwrap();
    let (engine, _) = engine_with_two_routes(root.path());
    let config = engine.config.clone();
    let planning = engine.planning_context();
    for (primary, copies) in [
        ("p01", 3),
        ("P02", 2),
        ("P02", 3),
        ("B01", 1),
        ("P01", 0),
        ("missing", 0),
    ] {
        match (
            config.route_observed(primary, copies, None),
            planning.route(primary, copies),
        ) {
            (Ok(captured), Ok(planned)) => assert_eq!(captured, planned),
            (Err(captured), Err(planned)) => {
                assert_eq!(captured.code(), planned.code());
                assert_eq!(captured.message(), planned.message());
            }
            results => panic!("route mismatch: {results:?}"),
        }
    }
    assert_eq!(
        config.route("missing", 0).unwrap_err().code(),
        ErrorCode::SlotNotFound
    );
    let visited = Cell::new(0);
    let stop = || {
        visited.set(visited.get() + 1);
        Err(Error::new(ErrorCode::RequestTimeout, "stop route"))
    };
    assert_eq!(
        config
            .route_observed("P01", 3, Some(&stop))
            .unwrap_err()
            .code(),
        ErrorCode::RequestTimeout
    );
    assert_eq!(visited.get(), 1);
}

#[test]
fn settings_query_preserves_short_route_repair_while_planner_rejects_incomplete_copy() {
    let root = tempfile::tempdir().unwrap();
    let (mut engine, repo) = engine_with_two_routes(root.path());
    engine
        .update_placement_rule(PlacementRuleUpdate {
            rule_id: &new_id("rule"),
            repository_id: &repo.repository_id,
            path_prefix: "future",
            primary_slot_id: "P02",
            expected_generation: 0,
            principal: "admin",
            operation_id: OperationId::generate(),
        })
        .unwrap();
    engine
        .update_protection_policy(ProtectionPolicyUpdate {
            policy_id: &new_id("policy"),
            repository_id: &repo.repository_id,
            path_prefix: "",
            target_copies: 3,
            expected_generation: 0,
            principal: "admin",
            operation_id: OperationId::generate(),
        })
        .unwrap();
    let settings = engine
        .with_namespace_source(|source| {
            file_placement_settings(
                &engine.state,
                &repo.repository_id,
                None,
                "future/new.txt",
                source,
                None,
            )
        })
        .unwrap();
    assert_eq!(settings, ("P02".into(), 3));
    assert_eq!(
        engine
            .file_placement_settings(&repo.repository_id, None, "future/new.txt")
            .unwrap(),
        settings
    );
    assert_eq!(
        engine
            .planning_context()
            .file_placement_settings(&repo.repository_id, None, "future/new.txt")
            .unwrap(),
        settings
    );
    assert_eq!(
        engine
            .new_file_placement(&repo.repository_id, "future/new.txt")
            .unwrap_err()
            .code(),
        ErrorCode::RouteIncomplete
    );
    let snapshot = engine
        .backup_configuration(&repo.repository_id, "")
        .unwrap();
    assert_eq!(snapshot.target_copies, 3);
    assert_eq!(snapshot.max_copies, 2);
    assert_eq!(snapshot.backup_routes.len(), 2);
    assert_eq!(snapshot.route_slot_ids, vec!["P01", "B01", "C01"]);
    let visited = Cell::new(0);
    let stop = || {
        visited.set(visited.get() + 1);
        Err(Error::new(ErrorCode::RequestTimeout, "stop settings"))
    };
    let error = engine
        .with_namespace_source(|source| {
            file_placement_settings(
                &engine.state,
                &repo.repository_id,
                None,
                "future/new.txt",
                source,
                Some(&stop),
            )
        })
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::RequestTimeout);
    assert_eq!(visited.get(), 1);
    visited.set(0);
    let missing = engine
        .with_namespace_source(|source| {
            file_placement_settings(
                &engine.state,
                "missing",
                None,
                "future/new.txt",
                source,
                Some(&stop),
            )
        })
        .unwrap_err();
    assert_eq!(missing.code(), ErrorCode::RepositoryNotFound);
    assert_eq!(visited.get(), 0);
}
