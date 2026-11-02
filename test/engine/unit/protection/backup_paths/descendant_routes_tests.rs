use super::*;

fn mkdir(engine: &mut Engine, repo: &Repository, path: &str) {
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(storage_contract::NamespaceOperationKind::Mkdir, path, None)
                .unwrap(),
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
}

fn place(engine: &mut Engine, repo: &Repository, path: &str) {
    engine
        .update_placement_rule(PlacementRuleUpdate {
            rule_id: &new_id("rule").unwrap(),
            repository_id: &repo.repository_id,
            path_prefix: path,
            primary_slot_id: "P02",
            expected_generation: 0,
            principal: "admin",
            operation_id: storage_contract::OperationId::generate().unwrap(),
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
        operation_id: storage_contract::OperationId::generate().unwrap(),
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
            json_codec::json!(read.backup_routes),
            json!([["P01", "B01", "C01"], ["P02", "B02"]])
        );
        assert_eq!(
            json_codec::json!(read.backup_slot_ids),
            json!(["B01", "B02", "C01"])
        );
        assert_eq!(read.max_copies, 2);
        assert_eq!(read.file_count, 0);
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
        json_codec::json!(read.backup_routes),
        json!([["P01", "B01", "C01"], ["P02", "B02"]])
    );
    assert_eq!(read.generation, 1);
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
        assert_eq!(
            json_codec::json!(read.backup_routes),
            json!([["P01", "B01", "C01"]])
        );
        assert_eq!(read.max_copies, 3);
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
        .create_repository(
            &storage_contract::RepositoryName::parse("other routes").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    place(&mut engine, &other, "docs/empty");
    let read = engine
        .backup_configuration(&repo.repository_id, "docs")
        .unwrap();
    assert_eq!(
        json_codec::json!(read.backup_routes),
        json!([["P01", "B01", "C01"]])
    );
    assert_eq!(read.max_copies, 3);
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
            rule_id: &new_id("rule").unwrap(),
            repository_id: &repo.repository_id,
            path_prefix: "docs/selected",
            primary_slot_id: "P01",
            expected_generation: 0,
            principal: "admin",
            operation_id: storage_contract::OperationId::generate().unwrap(),
        })
        .unwrap();
    let read = engine
        .backup_configuration(&repo.repository_id, "docs/selected")
        .unwrap();
    assert_eq!(
        json_codec::json!(read.backup_routes),
        json!([["P01", "B01", "C01"]])
    );
    assert_eq!(read.max_copies, 3);
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

#[test]
fn admission_rejects_equivalent_placement_prefixes() {
    let low = "rule_00000000000000000000000000000000";
    let high = "rule_ffffffffffffffffffffffffffffffff";
    for (path, equivalent) in [("docs", "docs"), ("docs", "DOCS"), ("", "")] {
        let temp = tempfile::tempdir().unwrap();
        let (mut engine, repo) = initialize_routes(temp.path());
        engine
            .update_placement_rule(PlacementRuleUpdate {
                rule_id: low,
                repository_id: &repo.repository_id,
                path_prefix: path,
                primary_slot_id: "P01",
                expected_generation: 0,
                principal: "admin",
                operation_id: OperationId::generate().unwrap(),
            })
            .unwrap();
        let duplicate = engine
            .update_placement_rule(PlacementRuleUpdate {
                rule_id: high,
                repository_id: &repo.repository_id,
                path_prefix: equivalent,
                primary_slot_id: "P02",
                expected_generation: 0,
                principal: "admin",
                operation_id: OperationId::generate().unwrap(),
            })
            .unwrap_err();
        assert_eq!(duplicate.code(), storage_contract::ErrorCode::NameCollision);

        let baseline = engine.state().clone();
        for (first, second) in [(low, high), (high, low)] {
            let mut state = baseline.clone();
            state.placement_rules.clear();
            for (id, prefix, primary) in [(first, path, "P01"), (second, equivalent, "P02")] {
                let mut rule = baseline.placement_rules[low].clone();
                rule.rule_id = id.into();
                rule.path_prefix = prefix.into();
                rule.primary_slot_id = primary.into();
                state.placement_rules.insert(id.into(), rule);
            }
            let raw: crate::history_package::RawState =
                decode_strict(&canonical_json(&state).unwrap()).unwrap();
            let error = State::try_from(raw)
                .expect_err("equivalent placement prefixes must fail snapshot admission");
            assert_eq!(error.code(), storage_contract::ErrorCode::StateInvalid);
        }
    }
}

#[test]
fn admission_preserves_distinct_placement_prefixes_and_repositories() {
    let temp = tempfile::tempdir().unwrap();
    let (mut engine, repo) = initialize_routes(temp.path());
    mkdir(&mut engine, &repo, "docs");
    for path in ["", "docs"] {
        engine
            .update_placement_rule(PlacementRuleUpdate {
                rule_id: &new_id("rule").unwrap(),
                repository_id: &repo.repository_id,
                path_prefix: path,
                primary_slot_id: "P01",
                expected_generation: 0,
                principal: "admin",
                operation_id: OperationId::generate().unwrap(),
            })
            .unwrap();
    }
    for path in ["docs/child", "docs-sibling", "docs2"] {
        place(&mut engine, &repo, path);
    }
    let other = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("other admitted routes").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    place(&mut engine, &other, "DOCS");
    let raw: crate::history_package::RawState =
        decode_strict(&canonical_json(engine.state()).unwrap()).unwrap();
    engine.state = std::sync::Arc::new(State::try_from(raw).unwrap());
    for (repository, path, expected) in [
        (&repo, "other/new.txt", "P01"),
        (&repo, "docs/new.txt", "P01"),
        (&repo, "docs/child/new.txt", "P02"),
        (&repo, "docs-sibling/new.txt", "P02"),
        (&repo, "docs2/new.txt", "P02"),
        (&other, "docs/new.txt", "P02"),
    ] {
        assert_eq!(
            engine
                .new_file_placement_for_file(&repository.repository_id, None, path)
                .unwrap()
                .0,
            expected
        );
    }
    let read = engine
        .backup_configuration(&repo.repository_id, "docs")
        .unwrap();
    assert_eq!(
        json_codec::json!(read.backup_routes),
        json!([["P01", "B01", "C01"], ["P02", "B02"]])
    );
}

#[test]
fn placement_specificity_uses_normalized_component_depth() {
    let low = "rule_00000000000000000000000000000000";
    let high = "rule_ffffffffffffffffffffffffffffffff";
    assert!("ſſſ".len() > "SSS/x".len());
    for (ancestor, spelling, query) in [("ſſſ", "SSS", "sss"), ("docs", "DOCS", "docs")] {
        let admitted_path = name_policy::PortablePath::parse(ancestor).unwrap();
        assert_eq!(admitted_path.key(), query);
        let child = format!("{spelling}/x");
        let sibling = format!("{spelling}-sibling");
        name_policy::PortablePath::parse(&child).unwrap();
        assert_eq!(name_policy::path_key(&child), format!("{query}/x"));
        for (parent_id, child_id) in [(low, high), (high, low)] {
            let temp = tempfile::tempdir().unwrap();
            let (mut engine, repo) = initialize_routes(temp.path());
            for (id, path, primary) in [
                ("rule_11111111111111111111111111111111", "", "P02"),
                (parent_id, ancestor, "P01"),
                (child_id, child.as_str(), "P02"),
                (
                    "rule_22222222222222222222222222222222",
                    sibling.as_str(),
                    "P01",
                ),
            ] {
                engine
                    .update_placement_rule(PlacementRuleUpdate {
                        rule_id: id,
                        repository_id: &repo.repository_id,
                        path_prefix: path,
                        primary_slot_id: primary,
                        expected_generation: 0,
                        principal: "admin",
                        operation_id: OperationId::generate().unwrap(),
                    })
                    .unwrap();
            }
            for (path, expected) in [
                (format!("{query}/x/new.txt"), "P02"),
                (format!("{query}/new.txt"), "P01"),
                (format!("{query}-sibling/new.txt"), "P01"),
                (format!("{query}2/new.txt"), "P02"),
                ("unrelated/new.txt".into(), "P02"),
            ] {
                let actual = engine
                    .new_file_placement_for_file(&repo.repository_id, None, &path)
                    .unwrap()
                    .0;
                eprintln!(
                    "accepted ancestor {ancestor:?}, key {query:?}; query {path:?} selected {actual}"
                );
                assert_eq!(actual, expected, "ancestor={ancestor}, path={path}");
            }
            let written = save(
                &mut engine,
                &repo,
                &format!("{query}/x/new.txt"),
                b"most specific placement",
            );
            assert_eq!(
                engine.state.placements[&written.version_id].primary_slot_id,
                "P02"
            );
            let configuration = engine
                .backup_configuration(&repo.repository_id, &format!("{query}/x"))
                .unwrap();
            assert_eq!(
                json_codec::json!(configuration.backup_routes),
                json!([["P02", "B02"]])
            );
        }
    }
}
