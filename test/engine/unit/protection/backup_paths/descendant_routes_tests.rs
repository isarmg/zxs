use super::*;

fn mkdir(engine: &mut Engine, repo: &Repository, path: &str) {
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation {
                kind: storage_contract::NamespaceOperationKind::Mkdir,
                path: path.into(),
                destination: None,
            },
            "admin",
            &new_id("op"),
        )
        .unwrap();
}

fn place(engine: &mut Engine, repo: &Repository, path: &str) {
    engine
        .update_placement_rule(PlacementRuleUpdate {
            rule_id: &new_id("rule"),
            repository_id: &repo.repository_id,
            path_prefix: path,
            primary_slot_id: "P02",
            expected_generation: 0,
            principal: "admin",
            operation_id: &new_id("op"),
        })
        .unwrap();
}

fn configure(
    engine: &mut Engine,
    repo: &Repository,
    path: &str,
    copies: u32,
    values: &[(&str, &str)],
    generation: u64,
) -> Result<OperationResult> {
    engine.configure_backups(BackupConfigurationRequest {
        repository_id: &repo.repository_id,
        path,
        target_copies: copies,
        backup_prefixes: &prefixes(values),
        expected_generation: generation,
        principal: "admin",
        operation_id: &new_id("op"),
    })
}

#[test]
fn empty_descendant_routes_use_normalized_identity_and_root_scope() {
    for (directory, scope, rule, write_path) in [
        ("docs/empty", "docs", "docs/empty", "docs/empty/new.txt"),
        (
            "Straße/empty",
            "straße",
            "STRASSE/empty",
            "Straße/empty/new.txt",
        ),
        ("docs/empty", "", "docs/empty", "docs/empty/new.txt"),
        ("docs", "docs", "docs/future", "docs/future/new.txt"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let (mut engine, repo) = initialize_routes(temp.path());
        mkdir(&mut engine, &repo, directory);
        place(&mut engine, &repo, rule);
        let before = canonical_json(engine.state()).unwrap();
        let read = engine
            .backup_configuration(&repo.repository_id, scope)
            .unwrap();
        assert_eq!(
            read["backup_routes"],
            json!([["P01", "B01", "C01"], ["P02", "B02"]])
        );
        assert_eq!(read["backup_slot_ids"], json!(["B01", "B02", "C01"]));
        assert_eq!(read["max_copies"], 2);
        assert_eq!(read["file_count"], "0");
        assert_eq!(canonical_json(engine.state()).unwrap(), before);
        for (copies, values, code) in [
            (
                2,
                vec![("B01", "first")],
                storage_contract::ErrorCode::BackupPrefixRequired,
            ),
            (
                3,
                vec![("B01", "first"), ("B02", "second"), ("C01", "cold")],
                storage_contract::ErrorCode::RouteIncomplete,
            ),
        ] {
            assert_eq!(
                configure(&mut engine, &repo, scope, copies, &values, 0)
                    .unwrap_err()
                    .code(),
                code
            );
            assert_eq!(canonical_json(engine.state()).unwrap(), before);
        }
        configure(
            &mut engine,
            &repo,
            scope,
            2,
            &[("B01", "first"), ("B02", "second")],
            0,
        )
        .unwrap();
        let written = save(&mut engine, &repo, write_path, b"first descendant write");
        assert_eq!(
            engine.state.placements[&written.version_id].route_slot_ids,
            vec!["P02", "B02"]
        );
        drop(engine);
        let engine = Engine::open(&temp.path().join("state")).unwrap();
        assert_eq!(
            engine.state.placements[&written.version_id].route_slot_ids,
            vec!["P02", "B02"]
        );
    }
}

#[test]
fn existing_parent_configuration_can_add_a_later_descendant_route() {
    let temp = tempfile::tempdir().unwrap();
    let (mut engine, repo) = initialize_routes(temp.path());
    mkdir(&mut engine, &repo, "docs/empty");
    configure(&mut engine, &repo, "docs", 2, &[("B01", "first")], 0).unwrap();
    place(&mut engine, &repo, "docs/empty");
    let read = engine
        .backup_configuration(&repo.repository_id, "docs")
        .unwrap();
    assert_eq!(
        read["backup_routes"],
        json!([["P01", "B01", "C01"], ["P02", "B02"]])
    );
    assert_eq!(read["generation"], "1");
    let before = canonical_json(engine.state()).unwrap();
    assert_eq!(
        configure(&mut engine, &repo, "docs", 2, &[("B01", "first")], 1)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::BackupPrefixRequired
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    configure(
        &mut engine,
        &repo,
        "docs",
        2,
        &[("B01", "first"), ("B02", "second")],
        1,
    )
    .unwrap();
    let written = save(
        &mut engine,
        &repo,
        "docs/empty/new.txt",
        b"after route change",
    );
    assert_eq!(
        engine.state.placements[&written.version_id].route_slot_ids,
        vec!["P02", "B02"]
    );
    assert_eq!(
        engine.state.file_backup_paths[&written.file_id]["B02"],
        "second/empty/new.txt"
    );
}

#[test]
fn independent_empty_child_policies_do_not_limit_parent_routes() {
    for copies in [1, 2] {
        let temp = tempfile::tempdir().unwrap();
        let (mut engine, repo) = initialize_routes(temp.path());
        mkdir(&mut engine, &repo, "docs/empty");
        place(&mut engine, &repo, "docs/empty");
        let child_prefixes = if copies == 1 {
            vec![]
        } else {
            vec![("B02", "child")]
        };
        configure(&mut engine, &repo, "docs/empty", copies, &child_prefixes, 0).unwrap();
        let before = engine
            .backup_configuration(&repo.repository_id, "docs/empty")
            .unwrap();
        let read = engine
            .backup_configuration(&repo.repository_id, "docs")
            .unwrap();
        assert_eq!(read["backup_routes"], json!([["P01", "B01", "C01"]]));
        assert_eq!(read["max_copies"], 3);
        configure(
            &mut engine,
            &repo,
            "docs",
            3,
            &[("B01", "parent"), ("C01", "cold")],
            0,
        )
        .unwrap();
        let after = engine
            .backup_configuration(&repo.repository_id, "docs/empty")
            .unwrap();
        assert_eq!(after, before);
        let written = save(
            &mut engine,
            &repo,
            "docs/empty/new.txt",
            b"independent child",
        );
        assert_eq!(
            engine.state.placements[&written.version_id].target_copies,
            copies
        );
        assert_eq!(
            engine.state.placements[&written.version_id].primary_slot_id,
            "P02"
        );
        if copies == 2 {
            assert_eq!(
                engine.state.file_backup_paths[&written.file_id]["B02"],
                "child/new.txt"
            );
        }
    }
}

#[test]
fn sibling_prefixes_and_other_repositories_do_not_add_parent_routes() {
    let temp = tempfile::tempdir().unwrap();
    let (mut engine, repo) = initialize_routes(temp.path());
    mkdir(&mut engine, &repo, "docs");
    for path in ["docs-sibling/empty", "docs2", "unrelated/docs"] {
        place(&mut engine, &repo, path);
    }
    let other = engine
        .create_repository("other routes", "P01", 1, "admin", &new_id("op"))
        .unwrap();
    place(&mut engine, &other, "docs/empty");
    let read = engine
        .backup_configuration(&repo.repository_id, "docs")
        .unwrap();
    assert_eq!(read["backup_routes"], json!([["P01", "B01", "C01"]]));
    assert_eq!(read["max_copies"], 3);
    configure(
        &mut engine,
        &repo,
        "docs",
        3,
        &[("B01", "first"), ("C01", "cold")],
        0,
    )
    .unwrap();
    let written = save(&mut engine, &repo, "docs/new.txt", b"own directory");
    assert_eq!(
        engine.state.placements[&written.version_id].route_slot_ids,
        vec!["P01", "B01", "C01"]
    );
}

#[test]
fn shadowed_ancestor_placement_does_not_add_a_phantom_route() {
    let temp = tempfile::tempdir().unwrap();
    let (mut engine, repo) = initialize_routes(temp.path());
    mkdir(&mut engine, &repo, "docs/selected");
    place(&mut engine, &repo, "docs");
    engine
        .update_placement_rule(PlacementRuleUpdate {
            rule_id: &new_id("rule"),
            repository_id: &repo.repository_id,
            path_prefix: "docs/selected",
            primary_slot_id: "P01",
            expected_generation: 0,
            principal: "admin",
            operation_id: &new_id("op"),
        })
        .unwrap();
    let read = engine
        .backup_configuration(&repo.repository_id, "docs/selected")
        .unwrap();
    assert_eq!(read["backup_routes"], json!([["P01", "B01", "C01"]]));
    assert_eq!(read["max_copies"], 3);
    configure(
        &mut engine,
        &repo,
        "docs/selected",
        3,
        &[("B01", "first"), ("C01", "cold")],
        0,
    )
    .unwrap();
    let written = save(
        &mut engine,
        &repo,
        "docs/selected/new.txt",
        b"selected route",
    );
    assert_eq!(
        engine.state.placements[&written.version_id].route_slot_ids,
        vec!["P01", "B01", "C01"]
    );
}
