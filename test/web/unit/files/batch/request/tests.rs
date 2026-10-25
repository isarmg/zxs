use super::*;
fn file(id: &str, path: &str, version: &str) -> Value {
    json!({"entry_id":id,"file_id":id,"path":path,"selected_path":path,"name":path.rsplit('/').next().unwrap(),"kind":"file","version_id":version})
}
fn resolved(id: &str, path: &str, selected_path: &str) -> Value {
    json!({"entry_id":id,"path":path,"selected_path":selected_path})
}
fn branch(id: &str, path: &str, entries: Vec<Value>) -> Value {
    json!({"directory_branch_id":id,"head_tree_id":format!("tree-{id}"),"name":id,"path":path,"entries":entries})
}
fn draft(kind: Kind) -> Draft {
    let f = file("f", "a/old.txt", "F1");
    let g = file("g", "a/data.txt", "G1");
    Draft {
        id: "draft".into(),
        kind,
        repository: "repo".into(),
        repository_path: "a".into(),
        disk_prefix: String::new(),
        entries: vec![f.clone(), g.clone()],
        names: BTreeMap::from([
            (entry_key(&file("f", "a/old.txt", "F1")), "new.txt".into()),
            (entry_key(&file("g", "a/data.txt", "G1")), "data.txt".into()),
        ]),
        sources: vec![
            branch("m", "a", vec![f, g]),
            branch(
                "n",
                "a",
                vec![
                    resolved("f", "a/alias.txt", "a/old.txt"),
                    file("g", "a/data.txt", "G2"),
                ],
            ),
            branch("r", "a", vec![]),
        ],
        source_resolved: true,
        scope_read: None,
        selected_sources: BTreeSet::from(["m".into(), "n".into()]),
        target_path: "b".into(),
        target_resolved: true,
        targets: vec![branch("l", "b", vec![]), branch("p", "b", vec![])],
        mappings: vec![
            Mapping {
                source: "m".into(),
                target: "l".into(),
            },
            Mapping {
                source: "n".into(),
                target: "p".into(),
            },
        ],
        loading: false,
        submitted_operation: None,
        registration: None,
        generation: 0,
        error: String::new(),
    }
}
#[test]
fn reviews_load_only_captured_source_facts_before_submission() {
    Owner::new().with(|| {
        for kind in [Kind::Move, Kind::Copy, Kind::Rename, Kind::Delete] {
            let state = RwSignal::new(State {
                authenticated: true,
                session_epoch: 8,
                ..Default::default()
            });
            let mut captured = draft(kind);
            let expected = request(&captured).unwrap();
            let facts = captured.sources.clone();
            for source in &mut captured.sources {
                source.as_object_mut().unwrap().remove("entries");
            }
            super::super::actions::open_draft(state, captured);
            state.with_untracked(|s| {
                let opened = s.files.batch.as_ref().unwrap();
                assert!(opened.loading);
                assert!(!opened.source_resolved);
                assert!(request(opened).is_err());
                assert_eq!(s.page, crate::ui::Page::Files);
            });
            super::super::actions::complete_source_scopes(
                state,
                8,
                "draft",
                1,
                Ok(json!({"repository_id":"repo","directory_branches":facts})),
            );
            state.with_untracked(|s| {
                let opened = s.files.batch.as_ref().unwrap();
                assert!(!opened.loading);
                assert_eq!(request(opened).unwrap(), expected);
            });
        }
    });
}

#[test]
fn source_facts_reject_stale_reads_head_changes_and_cancellation() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 8,
            ..Default::default()
        });
        let captured = draft(Kind::Delete);
        let facts = captured.sources.clone();
        super::super::actions::open_draft(state, captured);
        let value = json!({"repository_id":"repo","directory_branches":facts});
        for (epoch, id, generation) in [(7, "draft", 1), (8, "replaced", 1), (8, "draft", 0)] {
            super::super::actions::complete_source_scopes(
                state,
                epoch,
                id,
                generation,
                Ok(value.clone()),
            );
            assert!(state.with_untracked(|s| s.files.batch.as_ref().unwrap().loading));
        }
        let mut changed = value.clone();
        changed["directory_branches"][0]["head_tree_id"] = "new-tree".into();
        super::super::actions::complete_source_scopes(state, 8, "draft", 1, Ok(changed));
        assert!(state.with_untracked(|s| !s.files.batch.as_ref().unwrap().source_resolved));
        super::super::actions::load_sources(state);
        super::super::actions::complete_source_scopes(state, 8, "draft", 1, Ok(value.clone()));
        assert!(state.with_untracked(|s| s.files.batch.as_ref().unwrap().loading));
        state.update(|s| s.files.batch = None);
        super::super::actions::complete_source_scopes(state, 8, "draft", 2, Ok(value));
        assert!(state.with_untracked(|s| s.files.batch.is_none()));
    });
}

#[test]
fn missing_selected_alias_cannot_resolve_to_another_selected_alias() {
    let mut draft = draft(Kind::Delete);
    draft.selected_sources = BTreeSet::from(["m".into()]);
    draft.entries = vec![file("f", "a/first", "F1"), file("f", "a/second", "F1")];
    draft.sources[0]["entries"] = json!([resolved("f", "a/first", "a/first")]);
    assert!(request(&draft).is_err());
    draft.sources[0]["entries"] = json!([
        resolved("f", "a/first", "a/first"),
        resolved("f", "a/second", "a/second")
    ]);
    assert_eq!(
        request(&draft).unwrap()["items"].as_array().unwrap().len(),
        2
    );
}

#[test]
fn uncertain_results_keep_the_original_operation_until_its_own_receipt() {
    let mut draft = draft(Kind::Copy);
    draft.submitted_operation = Some("operation-one".into());
    let mut model = Model {
        batch: Some(draft),
        checked_entries: BTreeSet::from(["f".into()]),
        ..Default::default()
    };
    settle_operation(
        &mut model,
        "operation-one",
        crate::files::model::SaveStatus::Unknown,
    );
    settle_operation(
        &mut model,
        "unrelated",
        crate::files::model::SaveStatus::Published,
    );
    assert!(model.batch.is_some());
    settle_operation(
        &mut model,
        "operation-one",
        crate::files::model::SaveStatus::Rejected,
    );
    assert!(model.batch.as_ref().unwrap().submitted_operation.is_none());
    model.batch.as_mut().unwrap().submitted_operation = Some("operation-two".into());
    settle_operation(
        &mut model,
        "operation-one",
        crate::files::model::SaveStatus::Published,
    );
    assert!(model.batch.is_some());
    settle_operation(
        &mut model,
        "operation-two",
        crate::files::model::SaveStatus::Published,
    );
    assert!(model.batch.is_none());
    assert!(model.checked_entries.is_empty());
}

#[test]
fn malformed_http_success_preserves_the_batch_draft_until_a_real_poll_receipt() {
    Owner::new().with(|| {
        let state = RwSignal::new(State::default());
        let mut batch = draft(Kind::Copy);
        batch.submitted_operation = Some("original".into());
        state.update(|s| {
            s.files.batch = Some(batch);
            s.files.checked_entries.insert("f".into());
        });
        for response in [
            json!({"operation_id":"original", "state":"SUCCEEDED"}),
            json!({"operation_id":"original", "state":"REJECTED"}),
            json!({"operation_id":"original", "commit_state":"PUBLISHED"}),
        ] {
            assert_eq!(
                file_actions::record_receipt(state, "original", response),
                crate::files::model::SaveStatus::Unknown
            );
            state.with_untracked(|s| {
                assert_eq!(
                    s.files
                        .batch
                        .as_ref()
                        .unwrap()
                        .submitted_operation
                        .as_deref(),
                    Some("original")
                );
                assert!(s.files.checked_entries.contains("f"));
                assert!(file_actions::pending(s));
            });
        }
        let mut receipt: Value = json_codec::from_str(include_str!(
            "../../fixtures/namespace-batch-operation.json"
        ))
        .unwrap();
        receipt["operation_id"] = "original".into();
        assert_eq!(
            file_actions::record_receipt(state, "original", receipt),
            crate::files::model::SaveStatus::Published
        );
        state.with_untracked(|s| {
            assert!(s.files.batch.is_none());
            assert!(s.files.checked_entries.is_empty());
            assert!(!file_actions::pending(s));
        });
    });
}
#[test]
fn multiple_aliases_of_one_identity_resolve_by_path_without_sharing_name_edits() {
    let mut draft = draft(Kind::Rename);
    draft.selected_sources = BTreeSet::from(["m".into()]);
    let first = file("f", "a/first.txt", "F1");
    let second = file("f", "a/second.txt", "F2");
    draft.entries = vec![first.clone(), second.clone()];
    draft.sources[0]["entries"] = json!([first, second]);
    draft.names = BTreeMap::from([
        (entry_key(&draft.entries[0]), "renamed-first.txt".into()),
        (entry_key(&draft.entries[1]), "renamed-second.txt".into()),
    ]);
    let body = request(&draft).unwrap();
    assert_eq!(body["items"][0]["source"]["path"], "a/first.txt");
    assert_eq!(body["items"][1]["source"]["path"], "a/second.txt");
    assert_eq!(body["items"][0]["target"]["path"], "a/renamed-first.txt");
    assert_eq!(body["items"][1]["target"]["path"], "a/renamed-second.txt");
}
#[test]
fn paired_moves_keep_per_branch_paths_and_frozen_tree_guards() {
    let body = request(&draft(Kind::Move)).unwrap();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 4);
    assert_eq!(
        items[0]["source"],
        json!({"directory_branch_id":"m","expected_tree_id":"tree-m","entry_id":"f","path":"a/old.txt"})
    );
    assert_eq!(
        items[0]["target"],
        json!({"directory_branch_id":"l","expected_tree_id":"tree-l","path":"b/new.txt"})
    );
    assert_eq!(items[2]["source"]["path"], "a/alias.txt");
    assert_eq!(items[2]["target"]["directory_branch_id"], "p");
    assert!(
        items
            .iter()
            .all(|item| item["source"]["directory_branch_id"] != "r")
    );
}
#[test]
fn copying_to_multiple_explicit_targets_never_infers_a_cartesian_product() {
    let mut draft = draft(Kind::Copy);
    draft.selected_sources = BTreeSet::from(["m".into()]);
    draft.entries.truncate(1);
    draft.mappings = vec![
        Mapping {
            source: "m".into(),
            target: "l".into(),
        },
        Mapping {
            source: "m".into(),
            target: "p".into(),
        },
    ];
    assert_eq!(
        request(&draft).unwrap()["items"].as_array().unwrap().len(),
        2
    );
    draft.mappings.push(draft.mappings[0].clone());
    assert!(request(&draft).unwrap_err().contains("duplicate"));
}
#[test]
fn an_absent_entry_rejects_the_entire_selected_scope() {
    let mut draft = draft(Kind::Delete);
    draft.sources[1]["entries"] = json!([resolved("f", "a/alias.txt", "a/old.txt")]);
    assert!(request(&draft).is_err());
    draft.sources[1]["entries"] = json!([
        resolved("f", "a/alias.txt", "a/old.txt"),
        file("g", "a/data.txt", "G2")
    ]);
    assert_eq!(
        request(&draft).unwrap()["items"].as_array().unwrap().len(),
        4
    );
}
#[test]
fn rename_and_delete_stay_inside_each_chosen_branch() {
    let mut draft = draft(Kind::Rename);
    let body = request(&draft).unwrap();
    assert_eq!(body["items"][2]["source"]["path"], "a/alias.txt");
    assert_eq!(body["items"][2]["target"]["path"], "a/new.txt");
    assert_eq!(body["items"][2]["target"]["directory_branch_id"], "n");
    draft.kind = Kind::Delete;
    assert!(
        request(&draft).unwrap()["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item.get("target").is_none())
    );
}
#[test]
fn no_mapping_missing_guard_or_invalid_utf8_byte_length_is_submitted() {
    let mut draft = draft(Kind::Move);
    draft.mappings[1].target.clear();
    assert!(request(&draft).is_err());
    draft.mappings[1].target = "p".into();
    draft.sources[0]["head_tree_id"] = Value::Null;
    assert!(request(&draft).is_err());
    draft.sources[0]["head_tree_id"] = "tree-m".into();
    draft
        .names
        .insert(entry_key(&draft.entries[0]), "文".repeat(81));
    assert!(request(&draft).is_err());
    draft
        .names
        .insert(entry_key(&draft.entries[0]), "文".repeat(80));
    assert!(request(&draft).is_ok());
    for bad in ["../b", "a//b", "a\\b", "a/./b"] {
        assert!(!valid_path(bad));
    }
    assert!(valid_path("资料/报告"));
}

#[test]
fn current_long_names_can_be_renamed_or_moved_and_copied_unchanged() {
    let name = "文".repeat(43);
    assert_eq!(name.len(), 129);
    for kind in [Kind::Rename, Kind::Move, Kind::Copy] {
        let mut draft = draft(kind);
        let source_path = if kind == Kind::Rename {
            "a/old.txt".into()
        } else {
            format!("a/{name}")
        };
        let entry = file("f", &source_path, "F1");
        draft.entries = vec![entry.clone()];
        draft.names = BTreeMap::from([(entry_key(&entry), name.clone())]);
        draft.sources = vec![branch("m", "a", vec![entry])];
        draft.selected_sources = BTreeSet::from(["m".into()]);
        draft.mappings.truncate(1);
        let parent = if kind == Kind::Rename { "a" } else { "b" };
        let destination = format!("{parent}/{name}");
        assert!(name_policy::validate_path(&source_path).is_ok());
        assert!(name_policy::validate_path(&destination).is_ok());
        let body = request(&draft).unwrap();
        assert_eq!(body["items"][0]["source"]["path"], source_path);
        assert_eq!(body["items"][0]["target"]["path"], destination);
    }
}

#[test]
fn every_name_operation_rejects_invalid_components() {
    for kind in [Kind::Rename, Kind::Move, Kind::Copy] {
        let mut draft = draft(kind);
        for name in [
            "",
            ".",
            "..",
            "a/b",
            "a\\b",
            "CON",
            "bad.",
            "e\u{301}",
            "a\n",
            &"文".repeat(81),
        ] {
            draft
                .names
                .insert(entry_key(&draft.entries[0]), name.into());
            assert!(request(&draft).is_err(), "{name}");
        }
    }
}

#[test]
fn composed_destinations_obey_the_same_path_budget_as_the_engine() {
    for kind in [Kind::Rename, Kind::Move, Kind::Copy] {
        let mut draft = draft(kind);
        draft.entries.truncate(1);
        draft.selected_sources = BTreeSet::from(["m".into()]);
        draft.mappings.truncate(1);
        draft
            .names
            .insert(entry_key(&draft.entries[0]), "文字".into());
        let mut cases = vec![("a".repeat(197), true), ("a".repeat(198), false)];
        if kind != Kind::Rename {
            cases.extend([
                (vec!["a"; 31].join("/"), true),
                (vec!["a"; 32].join("/"), false),
            ]);
        }
        for (parent, allowed) in cases {
            assert!(valid_path(&parent));
            if kind == Kind::Rename {
                let source = format!("{parent}/x");
                assert!(name_policy::validate_path(&source).is_ok());
                let entry = file("f", &source, "F1");
                draft.names = BTreeMap::from([(entry_key(&entry), "文字".into())]);
                draft.entries = vec![entry.clone()];
                draft.sources[0]["entries"] = json!([entry]);
                draft.sources[0]["path"] = parent.clone().into();
            } else {
                draft.targets[0]["path"] = parent.clone().into();
            }
            let destination = format!("{parent}/文字");
            assert_eq!(name_policy::validate_path(&destination).is_ok(), allowed);
            assert_eq!(request(&draft).is_ok(), allowed, "{destination}");
        }
    }
    assert!(!valid_path(&"a".repeat(201)));
    assert!(!valid_path(&vec!["a"; 33].join("/")));
    assert!(valid_path(""));
}

#[test]
fn stale_detail_rename_keeps_original_identity_and_tree_instead_of_retargeting_same_path() {
    let mut draft = draft(Kind::Rename);
    draft.entries.truncate(1);
    draft.selected_sources = BTreeSet::from(["m".into()]);
    let body = request(&draft).unwrap();
    assert_eq!(
        body["items"][0]["source"],
        json!({"directory_branch_id":"m","expected_tree_id":"tree-m","entry_id":"f","path":"a/old.txt"})
    );
    assert_eq!(
        body["items"][0]["target"],
        json!({"directory_branch_id":"m","expected_tree_id":"tree-m","path":"a/new.txt"})
    );
    // If scope loading sees a replacement at the old path, fail closed rather
    // than constructing a path-only rename for the replacement identity.
    draft.sources[0]["entries"] = json!([file("replacement", "a/old.txt", "replacement-version")]);
    draft.sources[0]["head_tree_id"] = "replacement-tree".into();
    assert!(request(&draft).is_err());
}

#[test]
fn target_reads_preserve_the_draft_when_the_session_is_inactive() {
    Owner::new().with(|| {
        for (authenticated, session_transition) in [
            (false, crate::ui::SessionTransition::Idle),
            (true, crate::ui::SessionTransition::Restoring),
            (true, crate::ui::SessionTransition::LoggingOut),
            (true, crate::ui::SessionTransition::LogoutUnconfirmed),
        ] {
            let state = RwSignal::new(State {
                authenticated,
                session_transition,
                ..Default::default()
            });
            state.update(|s| s.files.batch = Some(draft(Kind::Copy)));
            super::super::actions::load_target(state);
            state.with_untracked(|s| {
                let draft = s.files.batch.as_ref().unwrap();
                assert_eq!(draft.generation, 0);
                assert!(!draft.loading);
                assert!(draft.target_resolved);
                assert_eq!(draft.targets.len(), 2);
            });
        }
    });
}

#[test]
fn destination_lookup_replaces_cached_targets_using_authoritative_directory_identity() {
    Owner::new().with(|| {
        let mut batch = draft(Kind::Copy);
        let mut old = branch("old-directory-branch", "b", vec![]);
        old["directory_id"] = "old-directory".into();
        let mut fresh = branch("fresh-directory-branch", "b", vec![]);
        fresh["directory_id"] = "fresh-directory".into();
        batch.targets = vec![old];
        batch.target_resolved = true;
        let state = RwSignal::new(State {
            authenticated: true,
            ..Default::default()
        });
        state.update(|s| s.files.batch = Some(batch));
        // The native task scope is not mounted, so the real action exposes its
        // synchronous read-admission state without issuing a browser request.
        super::super::actions::load_target(state);
        state.with_untracked(|s| {
            let batch = s.files.batch.as_ref().unwrap();
            assert!(
                batch.loading,
                "a cached path must not bypass authoritative identity lookup"
            );
            assert!(!batch.target_resolved);
            assert!(
                batch.targets.is_empty(),
                "a moved directory's frozen branch must not be offered for a replacement directory"
            );
        });
        let generation = state.with_untracked(|s| s.files.batch.as_ref().unwrap().generation);
        assert!(!super::super::actions::complete_target_scopes(
            state,
            0,
            "draft",
            generation,
            Ok(json!({"repository_id":"repo","directory_branches":[fresh.clone()]})),
        ));
        state.with_untracked(|s| {
            let batch = s.files.batch.as_ref().unwrap();
            assert!(!batch.loading);
            assert!(batch.target_resolved);
            assert_eq!(batch.targets, vec![fresh]);
        });
    });
}

#[test]
fn registration_poll_reloads_its_branch_records_without_reposting() {
    Owner::new().with(|| {
        for destination in [false, true] {
            let mut batch = draft(Kind::Copy);
            if destination {
                batch.targets.clear();
            } else {
                batch.sources.clear();
                batch.selected_sources.clear();
                batch.mappings.clear();
            }
            // Use the exact synchronous registration admission used by the action.
            assert!(super::super::actions::begin_scope_registration(&mut batch, "scope-original".into(), destination).is_some());
            let state = RwSignal::new(State {
                authenticated: true,
                ..Default::default()
            });
            state.update(|s| s.files.batch = Some(batch));
            file_actions::record_operation(state, json!({"operation_id":"scope-original","state":"in_doubt"}));
            assert!(state.with_untracked(file_actions::pending));
            let mut receipt: Value = json_codec::from_str(include_str!("../../fixtures/namespace-batch-operation.json")).unwrap();
            receipt["operation_id"] = "scope-original".into();
            receipt["operation_kind"] = "directory.scope.initialize".into();
            receipt["result"] = json!({"directory_branch_id":"registered","head_tree_id":"registered-tree"});
            assert_eq!(file_actions::record_receipt(state, "scope-original", receipt), crate::files::model::SaveStatus::Published);
            state.with_untracked(|s| {
                assert!(!file_actions::pending(s));
                let batch = s.files.batch.as_ref().unwrap();
                assert!(batch.loading, "the successful poll must resume the scope read rather than offer another registration POST");
                assert_eq!(batch.names[&entry_key(&batch.entries[0])], "new.txt");
            });
        }
    });
}

#[test]
fn target_scope_completion_ignores_superseded_reads_and_preserves_historical_scopes() {
    Owner::new().with(|| {
        let state = RwSignal::new(State { authenticated: true, session_epoch: 8, ..Default::default() });
        let mut captured = draft(Kind::Copy);
        captured.loading = true;
        captured.generation = 3;
        captured.targets.clear();
        captured.target_resolved = false;
        state.update(|s| s.files.batch = Some(captured));
        let value = json!({"repository_id":"repo","directory_branches":[branch("historical", "old-path", vec![])]});
        for (epoch, id, generation) in [(7,"draft",3), (8,"old-draft",3), (8,"draft",2)] {
            assert!(!super::super::actions::complete_target_scopes(state, epoch, id, generation, Ok(value.clone())));
            assert!(state.with_untracked(|s| s.files.batch.as_ref().unwrap().loading));
            assert!(state.with_untracked(|s| s.files.batch.as_ref().unwrap().targets.is_empty()));
        }
        assert!(!super::super::actions::complete_target_scopes(state, 8, "draft", 3, Ok(value)));
        state.with_untracked(|s| {
            let draft = s.files.batch.as_ref().unwrap();
            assert_eq!(draft.targets[0]["directory_branch_id"], "historical");
            assert!(draft.target_resolved);
            assert!(!draft.loading);
        });
        state.update(|s| { let draft=s.files.batch.as_mut().unwrap(); draft.loading=true; draft.targets.clear(); draft.target_resolved=false; });
        assert!(super::super::actions::complete_target_scopes(state, 8, "draft", 3, Ok(json!({"repository_id":"repo","directory_branches":[]}))));
        assert!(state.with_untracked(|s| s.files.batch.as_ref().unwrap().loading));
        assert!(!state.with_untracked(|s| s.files.batch.as_ref().unwrap().target_resolved));
    });
}

fn registration_review(destination: bool) -> RwSignal<State> {
    let mut batch = draft(Kind::Copy);
    if destination {
        batch.targets.clear();
    } else {
        batch.sources.clear();
        batch.selected_sources.clear();
        batch.mappings.clear();
    }
    assert!(
        super::super::actions::begin_scope_registration(
            &mut batch,
            "scope-original".into(),
            destination,
        )
        .is_some()
    );
    let state = RwSignal::new(State {
        authenticated: true,
        session_epoch: 7,
        ..Default::default()
    });
    state.update(|s| {
        s.management.storage = crate::management::Storage::Ready;
        s.files.writable = true;
        s.files.batch = Some(batch);
    });
    state
}
fn scope_receipt() -> Value {
    let mut receipt: Value = json_codec::from_str(include_str!(
        "../../fixtures/namespace-batch-operation.json"
    ))
    .unwrap();
    receipt["operation_id"] = "scope-original".into();
    receipt["operation_kind"] = "directory.scope.initialize".into();
    receipt["result"] =
        json!({"directory_branch_id":"registered","head_tree_id":"registered-tree"});
    receipt
}
fn scope_read_error(status: u16) -> api::Error {
    api::Error {
        status,
        code: "SCOPE_READ_FAILED".into(),
        message: "Scope read failed".into(),
        retryable: true,
        request_id: None,
    }
}

#[test]
fn published_registration_read_failure_retries_only_the_read_and_keeps_inputs() {
    Owner::new().with(|| {
        for destination in [false, true] {
            let state = registration_review(destination);
            let original_names =
                state.with_untracked(|s| s.files.batch.as_ref().unwrap().names.clone());
            // A complete direct receipt uses the same hook as the later poll.
            assert_eq!(
                file_actions::record_receipt(state, "scope-original", scope_receipt()),
                crate::files::model::SaveStatus::Published
            );
            assert!(state.with_untracked(|s| s.files.batch.as_ref().unwrap().loading));
            super::super::actions::complete_registered_scope(
                state,
                7,
                "draft",
                "scope-original",
                0,
                Err(scope_read_error(503)),
            );
            state.with_untracked(|s| {
                let draft = s.files.batch.as_ref().unwrap();
                assert!(!draft.loading);
                assert_eq!(draft.names, original_names);
                assert!(draft.registration.as_ref().unwrap().published);
                assert!(!draft.error.is_empty());
                assert!(!file_actions::pending(s));
            });
            // These real action entries must return before secure-ID generation
            // or a new POST, even though general write admission is healthy.
            super::super::actions::initialize_scope(state, destination);
            super::super::actions::submit(state);
            super::super::actions::load_target(state);
            state.with_untracked(|s| {
                assert!(!s.files.mutation_pending);
                let draft = s.files.batch.as_ref().unwrap();
                assert_eq!(draft.generation, 0);
                assert!(!draft.loading);
                assert!(draft.submitted_operation.is_none());
                assert_eq!(
                    draft.registration.as_ref().unwrap().operation_id,
                    "scope-original"
                );
            });
            reload_registered_scope(state);
            assert!(state.with_untracked(|s| s.files.batch.as_ref().unwrap().loading));
            // A repeated receipt cannot release or duplicate the in-flight read.
            file_actions::record_receipt(state, "scope-original", scope_receipt());
            assert!(state.with_untracked(|s| s.files.batch.as_ref().unwrap().loading));
            let mut registered = branch("registered", if destination { "b" } else { "a" }, vec![]);
            registered["current"] = true.into();
            super::super::actions::complete_registered_scope(
                state,
                7,
                "draft",
                "scope-original",
                0,
                Ok(json!({"repository_id":"repo","directory_branches":[registered.clone()]})),
            );
            state.with_untracked(|s| {
                let draft = s.files.batch.as_ref().unwrap();
                assert!(!draft.loading);
                assert!(draft.registration.is_none());
                assert!(draft.error.is_empty());
                assert_eq!(draft.names, original_names);
                if destination {
                    assert_eq!(draft.targets, vec![registered]);
                    assert!(draft.target_resolved);
                } else {
                    assert_eq!(draft.sources, vec![registered]);
                    assert_eq!(
                        draft.selected_sources,
                        BTreeSet::from(["registered".into()])
                    );
                    assert_eq!(draft.mappings.len(), 1);
                    assert_eq!(draft.mappings[0].source, "registered");
                }
                assert_eq!(s.files.operations.len(), 1);
            });
        }
    });
}

#[test]
fn registration_receipts_and_reads_belong_only_to_the_original_draft_and_session() {
    Owner::new().with(|| {
        let state = registration_review(true);
        state.update(|s| {
            assert!(!settle_operation(&mut s.files, "unrelated", crate::files::model::SaveStatus::Published));
            assert!(!s.files.batch.as_ref().unwrap().registration.as_ref().unwrap().published);
        });
        file_actions::record_receipt(state, "scope-original", scope_receipt());
        let ready = json!({"repository_id":"repo","directory_branches":[branch("registered", "b", vec![])]});
        for (epoch, id, operation, generation) in [(6,"draft","scope-original",0),(7,"old-draft","scope-original",0),(7,"draft","other",0),(7,"draft","scope-original",1)] {
            super::super::actions::complete_registered_scope(state, epoch, id, operation, generation, Ok(ready.clone()));
            assert!(state.with_untracked(|s| s.files.batch.as_ref().unwrap().loading));
            assert!(state.with_untracked(|s| s.files.batch.as_ref().unwrap().targets.is_empty()));
        }
        // Missing records after confirmed publication remain a read-retry, not
        // permission to register again under a new operation ID.
        super::super::actions::complete_registered_scope(state, 7, "draft", "scope-original", 0, Ok(json!({"repository_id":"repo","directory_branches":[]})));
        assert!(state.with_untracked(|s| s.files.batch.as_ref().unwrap().registration.is_some()));
        assert!(!state.with_untracked(|s| s.files.batch.as_ref().unwrap().loading));
        state.update(|s| s.files.batch = None);
        super::super::actions::complete_registered_scope(state, 7, "draft", "scope-original", 0, Ok(ready));
        assert!(state.with_untracked(|s| s.files.batch.is_none()));
        // Closing the review cannot hide expiry of its still-current session.
        super::super::actions::complete_registered_scope(state, 7, "draft", "scope-original", 0, Err(scope_read_error(401)));
        assert!(!state.with_untracked(State::session_active));
    });
}

#[test]
fn rejected_registration_releases_only_its_own_post_for_explicit_retry() {
    Owner::new().with(|| {
        let state = registration_review(false);
        state.update(|s| {
            assert!(!settle_operation(
                &mut s.files,
                "other",
                crate::files::model::SaveStatus::Rejected
            ));
            assert!(s.files.batch.as_ref().unwrap().registration.is_some());
            assert!(!settle_operation(
                &mut s.files,
                "scope-original",
                crate::files::model::SaveStatus::Rejected
            ));
            let draft = s.files.batch.as_mut().unwrap();
            assert!(draft.registration.is_none());
            assert!(!draft.loading);
            assert_eq!(draft.names[&entry_key(&draft.entries[0])], "new.txt");
            assert!(
                super::super::actions::begin_scope_registration(
                    draft,
                    "retry-after-rejection".into(),
                    false
                )
                .is_some()
            );
        });
    });
}

#[test]
fn a_replacement_source_without_own_scopes_stays_unregistered() {
    Owner::new().with(|| {
        let mut initial = State::default();
        initial.files.repository_path = "a".into();
        initial.files.entries = vec![file("new-directory-file", "a/new.txt", "new-version")].into();
        // The server's identity-aware listing has no scopes for the replacement
        // directory even though a frozen old directory once used the same path.
        initial.files.directory_branches.clear();
        let mut captured = draft(Kind::Copy);
        captured.sources = source_branches(&initial.files);
        captured.selected_sources.clear();
        captured.mappings.clear();
        captured.entries = initial.files.entries.to_vec();
        let state = RwSignal::new(initial);
        super::super::actions::open_draft(state, captured);
        state.update(|s| {
            let draft = s.files.batch.as_mut().unwrap();
            assert!(draft.sources.is_empty());
            assert!(draft.selected_sources.is_empty());
            assert!(draft.mappings.is_empty());
            assert!(!draft.loading);
            assert!(
                super::super::actions::begin_scope_registration(
                    draft,
                    "new-directory-registration".into(),
                    false
                )
                .is_some()
            );
            assert_eq!(draft.registration.as_ref().unwrap().path, "a");
        });
    });
}

#[test]
fn confirmed_registration_cannot_be_downgraded_by_late_receipts() {
    use crate::files::model::SaveStatus;
    Owner::new().with(|| {
        for confirmed in [SaveStatus::Published, SaveStatus::Degraded] {
            for late in [
                SaveStatus::Unknown,
                SaveStatus::Rejected,
                SaveStatus::Published,
                SaveStatus::Degraded,
            ] {
                for loading in [true, false] {
                    let state = registration_review(true);
                    state.update(|s| {
                        assert!(settle_operation(&mut s.files, "scope-original", confirmed));
                        let draft = s.files.batch.as_mut().unwrap();
                        draft.loading = loading;
                        draft.error = if loading {
                            String::new()
                        } else {
                            "Retry this read".into()
                        };
                        let error = draft.error.clone();
                        let names = draft.names.clone();
                        assert!(!settle_operation(&mut s.files, "scope-original", late));
                        let draft = s.files.batch.as_ref().unwrap();
                        assert!(
                            draft
                                .registration
                                .as_ref()
                                .is_some_and(|registration| registration.published
                                    && registration.operation_id == "scope-original"),
                            "confirmed publication must keep its original read association"
                        );
                        assert_eq!(
                            draft.loading, loading,
                            "late receipts cannot release or restart the follow-on read"
                        );
                        assert_eq!(draft.error, error);
                        assert_eq!(draft.names, names);
                    });
                }
            }
        }
    });
}

#[test]
fn selected_scope_query_carries_only_captured_heads_and_selection_pairs() {
    let mut draft = draft(Kind::Delete);
    draft.entries[0]["repository_path"] = "a/logical.txt".into();
    let body =
        super::super::actions::selected_scope_body(&draft, &draft.sources, &draft.repository_path);
    assert_eq!(body["path"], "a");
    assert_eq!(
        body["sources"][0],
        json!({"directory_branch_id":"m","expected_tree_id":"tree-m"})
    );
    assert_eq!(
        body["entries"][0],
        json!({"entry_id":"f","path":"a/logical.txt"})
    );
    assert_eq!(
        body["entries"].as_array().unwrap().len(),
        draft.entries.len()
    );
    assert!(body["sources"][0].get("entries").is_none());
}

#[test]
fn unresolved_sources_never_allow_submission_or_registration() {
    let mut draft = draft(Kind::Delete);
    draft.source_resolved = false;
    assert!(request(&draft).is_err());
    draft.sources.clear();
    assert!(
        super::super::actions::begin_scope_registration(&mut draft, "must-not-write".into(), false)
            .is_none()
    );
}
