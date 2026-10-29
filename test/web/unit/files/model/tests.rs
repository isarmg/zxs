use super::*;
use crate::files::history::tests::{
    branch as file_branch, branch_page, branch_wire, chain, loaded,
};
use crate::files::test_entries::{branch, entry, update};
use json_codec::json;

#[test]
fn navigation_resets_only_location_owned_state() {
    use crate::files::review::{Kind, Publication};
    for (destination, slot, directory, query, loaded) in [
        (Navigation::Root, "", "", "old query", true),
        (Navigation::Disk("B01".into()), "B01", "", "", false),
        (
            Navigation::Directory("资料/子目录".into()),
            "P01",
            "资料/子目录",
            "",
            false,
        ),
    ] {
        let selected = entry(json!({"entry_id":"selected","path":"old/file.txt"}));
        let disks = vec![Disk {
            slot_id: "P01".into(),
            online: true,
            writable: true,
            available_bytes: 4096,
        }];
        let mut degraded: Value = json_codec::from_str(DEGRADED).unwrap();
        // Derive a synthetic outcome from the captured diagnostic; navigation
        // preserves the accepted result, not a fresh health observation.
        degraded.as_object_mut().unwrap().remove("current_health");
        let operations = vec![
            FileOperation::unknown("original"),
            FileOperation::from_receipt(degraded["operation_id"].as_str().unwrap(), &degraded),
        ];
        // Enumerate the model so a new state owner requires an explicit choice.
        let mut model = Model {
            disks: disks.clone(),
            disks_loaded: true,
            loading_disks: true,
            disks_generation: 5,
            slot: "P01".into(),
            directory: "old".into(),
            writable: true,
            entries: vec![selected.clone()].into(),
            cursor: Some("current".into()),
            next_cursor: Some("next".into()),
            previous_cursor: Some("previous".into()),
            total_entries: 205,
            listing_failed: true,
            listing_cancellation: None,
            search_delay: None,
            directory_branches: vec![branch(json!({"current":true}))],
            directory_file_count: 210,
            directory_branches_supported: true,
            repository_id: "repository".into(),
            repository_path: "old".into(),
            default_branch_scope: true,
            loaded: true,
            loading: true,
            generation: 7,
            selected: Some(selected.clone()),
            history: {
                let mut history = History::default();
                history.install(chain(1));
                history.generation = 11;
                history
            },
            open_editors: 1,
            checked_entries: [entry_key(&selected)].into_iter().collect(),
            batch: None,
            merge: None,
            backup: None,
            tag_edit: None,
            checked_nodes: [(
                "n1".to_owned(),
                crate::files::tags::CheckedNode {
                    root: crate::files::history::HistoryRoot::parse(
                        crate::files::history::tests::ROOT,
                    )
                    .unwrap(),
                    node_id: "n1".into(),
                    branch: "branch".into(),
                    expected: "n1".into(),
                    tags: BTreeSet::new(),
                    label: "1".into(),
                },
            )]
            .into_iter()
            .collect(),
            checked_bytes: 64,
            publications: [(
                Kind::Tags,
                Publication {
                    operation_id: Some("original".into()),
                    error: "unconfirmed".into(),
                },
            )]
            .into_iter()
            .collect(),
            operations: operations.clone(),
            polling_operations: vec!["original".into()],
            mutation_pending: true,
            query: "old query".into(),
        };

        model.navigate(destination);
        assert_eq!((&*model.slot, &*model.directory), (slot, directory));
        assert_eq!(model.query, query);
        assert_eq!(model.loaded, loaded);
        assert!(!model.writable && !model.loading && !model.listing_failed);
        assert!(model.entries.is_empty());
        assert!(model.cursor.is_none());
        assert!(model.next_cursor.is_none() && model.previous_cursor.is_none());
        assert_eq!(model.total_entries, 0);
        assert!(model.search_delay.is_none() && model.listing_cancellation.is_none());
        assert_eq!(model.generation, 8);
        assert!(model.directory_branches.is_empty());
        assert_eq!(model.directory_file_count, 0);
        assert!(!model.directory_branches_supported && !model.default_branch_scope);
        assert!(model.repository_id.is_empty() && model.repository_path.is_empty());
        assert!(model.selected.is_none() && model.history.window().is_none());
        assert!(!model.history.loading());
        assert_eq!(model.history.generation, 12);
        assert!(model.checked_entries.is_empty() && model.checked_nodes.is_empty());
        assert_eq!(model.checked_bytes, 0);
        assert_eq!(
            model.open_editors, 1,
            "editors unmount with their owner, not here"
        );
        assert!(model.batch.is_none() && model.merge.is_none());
        assert!(model.backup.is_none() && model.tag_edit.is_none());

        assert_eq!(model.disks, disks);
        assert!(model.disks_loaded && model.loading_disks);
        assert_eq!(model.disks_generation, 5);
        assert_eq!(model.operations, operations);
        assert_eq!(model.operations[1].status(), SaveStatus::Degraded);
        assert_eq!(model.polling_operations, ["original".to_owned()]);
        assert!(model.mutation_pending);
        assert_eq!(model.publications.len(), 1);
        assert_eq!(
            model.publications[&Kind::Tags].operation_id.as_deref(),
            Some("original")
        );
        assert_eq!(model.publications[&Kind::Tags].error, "unconfirmed");
    }
}

#[test]
fn refreshed_selection_invalidates_old_history_only_when_context_changes() {
    let original = entry(
        json_codec::json!({"entry_id":"entry", "file_id":"file", "path":"a.txt", "version_id":"v1", "selected_branch_id":"main"}),
    );
    let mut model = Model {
        selected: Some(original.clone()),
        entries: vec![original.clone()].into(),
        ..Default::default()
    };
    loaded(&mut model, chain(1));
    model.history.begin_main(None);
    model.reconcile_selection(model.entries.first().cloned());
    assert_eq!(model.history.generation, 1);
    update(
        &mut Arc::make_mut(&mut model.entries)[0],
        json!({"selected_branch_id":"draft","version_id":"v2"}),
    );
    assert_eq!(
        crate::files::tags::check(&mut model, "n1", true),
        crate::files::tags::Check::NotResident,
        "a loading window offers nothing to check"
    );
    model.checked_bytes = 10;
    model.reconcile_selection(model.entries.first().cloned());
    assert_eq!(model.selected.as_ref().unwrap().version_id().unwrap(), "v2");
    assert!(model.history.window().is_none());
    assert!(!model.history.loading());
    assert!(model.checked_nodes.is_empty());
    assert_eq!(model.checked_bytes, 0);
    assert_eq!(model.history.generation, 2);
}
#[test]
fn backup_context_never_guesses_a_primary_alias_from_its_prefix() {
    let mut backup = entry(
        json_codec::json!({"backup":true,"read_only":true,"name":"f.txt","path":"我的备份/f.txt","repository_id":"repo","repository_path":"actual/primary/f.txt"}),
    );
    assert_eq!(
        file_context(&backup),
        Some(("repo".into(), "actual/primary/f.txt".into()))
    );
    update(
        &mut backup,
        json!({"repository_path":null,"historical_only":true,"selected_branch_id":null}),
    );
    assert!(file_context(&backup).is_none());
}

#[test]
fn backup_history_keeps_its_stored_body_and_current_node() {
    use crate::files::history::tests::{node, window_wire};
    let selected_entry = entry(
        json_codec::json!({"backup":true,"read_only":true,"version_id":"stored-old","node_id":"stored-old"}),
    );
    let page = ["primary-new", "stored-old"];
    let mut wire = window_wire(
        vec![
            node("primary-new", 2, &["stored-old"], &page),
            node("stored-old", 1, &[], &page),
        ],
        json!({}),
    );
    wire["context"]["current_node_id"] = "primary-new".into();
    wire["context"]["version_id"] = "primary-new".into();
    wire["context"]["own_version_id"] = "primary-new".into();
    wire["nodes"][0]["current"] = true.into();
    let history = crate::files::history::HistoryWindow::parse(wire, "file").unwrap();
    let result = history.clone().for_entry(&selected_entry);
    assert_eq!(
        result
            .context
            .content
            .as_ref()
            .unwrap()
            .version_id
            .as_deref(),
        Some("stored-old")
    );
    assert_eq!(
        result.context.current_node_id.as_deref(),
        Some("stored-old")
    );
    assert!(!result.nodes[0].current);
    assert!(result.nodes[1].current);
    assert_eq!(
        history
            .clone()
            .for_entry(&entry(json_codec::json!({"version_id":"stored-old"}))),
        history
    );
    let mut model = Model {
        writable: true,
        selected: Some(selected_entry),
        ..Default::default()
    };
    loaded(&mut model, history);
    model.history.branches = Some(branch_page(
        vec![branch_wire(json!({"branch_id":"main","current":true}))],
        Some("main"),
    ));
    assert!(selected_entry_read_only(&model));
    model.selected = Some(entry(json_codec::json!({"read_only":true})));
    assert!(selected_entry_read_only(&model));
}
const DEGRADED: &str = include_str!("../fixtures/degraded-save-operation.json");
const DAMAGE: &str = include_str!("../fixtures/damaged-published-save-operation.json");
#[test]
fn captured_http_outcomes_and_diagnostics_keep_separate_consumer_contracts() {
    use storage_contract::client_protocol::{OperationStatus, operation_observation_status};

    // Real HTTP captures: https://github.com/isarmg/zxs/actions/runs/40834921728
    // The run records source 93f1f962 and its rustfmt patch. Existing captures
    // remain unchanged; these pairs were obtained from the two actual routes.
    for (outcome, observation, status, save_status) in [
        (
            include_str!("../fixtures/github-degraded-save-outcome.json"),
            include_str!("../fixtures/github-degraded-save-observation.json"),
            OperationStatus::Degraded,
            SaveStatus::Degraded,
        ),
        (
            include_str!("../fixtures/github-damaged-published-save-outcome.json"),
            include_str!("../fixtures/github-damaged-published-save-observation.json"),
            OperationStatus::Published,
            SaveStatus::Published,
        ),
        (
            include_str!("../fixtures/github-namespace-batch-outcome.json"),
            include_str!("../fixtures/github-namespace-batch-observation.json"),
            OperationStatus::Published,
            SaveStatus::Published,
        ),
    ] {
        let outcome: Value = json_codec::from_str(outcome).unwrap();
        let observation: Value = json_codec::from_str(observation).unwrap();
        let id = outcome["operation_id"].as_str().unwrap();
        assert!(outcome.get("current_health").is_none());
        assert_eq!(FileOperation::from_receipt(id, &outcome).status(), save_status);
        assert_eq!(operation_observation_status(&observation), Some(status));
        assert_eq!(
            FileOperation::from_receipt(id, &observation).status(),
            SaveStatus::Unknown
        );
        let mut observed_outcome = observation;
        observed_outcome
            .as_object_mut()
            .unwrap()
            .remove("current_health");
        assert_eq!(outcome, observed_outcome);
    }
}
#[test]
fn publication_protection_is_not_rewritten_by_later_damage() {
    let mut damage: Value = json_codec::from_str(DAMAGE).unwrap();
    assert_eq!(
        FileOperation::from_receipt(damage["operation_id"].as_str().unwrap(), &damage).status(),
        SaveStatus::Unknown
    );
    // Derive a healthy comparison from this real receipt, changing only
    // current health. Frozen publication evidence must give the same label.
    let mut good = damage.clone();
    good["current_health"]["state"] = "HEALTHY".into();
    assert_eq!(good["result"], damage["result"]);
    good.as_object_mut().unwrap().remove("current_health");
    damage.as_object_mut().unwrap().remove("current_health");
    assert_eq!(good, damage);
    let id = damage["operation_id"].as_str().unwrap();
    assert_eq!(
        FileOperation::from_receipt(id, &good).status(),
        SaveStatus::Published
    );
    assert_eq!(
        FileOperation::from_receipt(id, &damage).status(),
        SaveStatus::Published
    );
}
#[test]
fn permanent_loss_is_distinct_from_ordinary_save() {
    let mut loss: Value = json_codec::from_str(DEGRADED).unwrap();
    loss.as_object_mut().unwrap().remove("current_health");
    let id = loss["operation_id"].as_str().unwrap();
    assert_eq!(
        FileOperation::from_receipt(id, &loss).status(),
        SaveStatus::Degraded
    );
    assert!(publication_notice(SaveStatus::Degraded).contains("Permanently lost"));
}
#[test]
fn uncertain_save_cannot_be_relabelled_as_a_failure() {
    assert_eq!(
        FileOperation::from_receipt("original", &json_codec::json!({"state":"in_doubt"})).status(),
        SaveStatus::Unknown
    );
}
#[test]
fn breadcrumbs_stay_relative_to_the_selected_disk() {
    assert_eq!(
        breadcrumbs("photos/trips"),
        vec![
            ("photos".into(), "photos".into()),
            ("trips".into(), "photos/trips".into())
        ]
    );
    assert_eq!(join_path("", "photo.txt"), "photo.txt");
}
#[test]
fn names_cannot_escape_the_directory() {
    for bad in ["", ".", "..", "a/b", "a\\b", "a\n"] {
        assert!(!valid_name(bad));
    }
    assert!(valid_name("report 2026.txt"));
}
#[test]
fn branch_names_mark_pending_content_without_reinterpreting_records() {
    assert!(
        branch_name(&file_branch(json!({"name":"draft","materialized":false,"version_id":null,"own_version_id":null,"node_id":null})))
            .contains("Pending content")
    );
    assert_eq!(branch_name(&file_branch(json!({"name":"saved"}))), "saved");
}
#[test]
fn current_branch_identity_does_not_require_a_content_version() {
    let mut model = Model::default();
    loaded(&mut model, chain(1));
    model.history.branches = Some(branch_page(
        vec![branch_wire(
            json!({"branch_id":"pending","current":true,"materialized":false,"version_id":null,"own_version_id":null,"node_id":null}),
        )],
        Some("pending"),
    ));
    assert_eq!(
        selected_branch(&model),
        "",
        "file-wide candidate selection is not a location choice"
    );
    model.history.clear();
    model.selected = Some(entry(
        json_codec::json!({"selected_branch_id":"entry-pending","version_id":null,"source_version_id":""}),
    ));
    assert_eq!(selected_branch(&model), "entry-pending");
}
#[test]
fn generated_public_file_branch_names_are_editable_only_after_loading_a_real_record() {
    let mut model = Model {
        selected: Some(entry(
            json_codec::json!({"file_id":"file-a","selected_branch_id":"generated-a"}),
        )),
        ..Default::default()
    };
    assert!(selected_file_branch_record(&model).is_none());
    loaded(&mut model, chain(1));
    let page = |name: &str| {
        branch_page(
            vec![
                branch_wire(json!({"branch_id":"generated-a","name":name,"current":true})),
                branch_wire(json!({"branch_id":"other","name":"other"})),
            ],
            Some("generated-a"),
        )
    };
    model.history.branches = Some(page("方案甲-7c2a"));
    assert_eq!(
        selected_file_branch_record(&model),
        Some(("file-a".into(), "generated-a".into(), "方案甲-7c2a".into()))
    );
    model.history.branches = Some(page("用户改名"));
    assert_eq!(
        selected_file_branch_record(&model).unwrap().1,
        "generated-a"
    );
    assert_eq!(selected_file_branch_record(&model).unwrap().2, "用户改名");
}
#[test]
fn root_default_selection_uses_real_directory_ids_and_subfolders_keep_names() {
    let branch = branch(
        json_codec::json!({"directory_branch_id":"directory-a","name":"auto-generated","current":true,"materialized":false}),
    );
    assert_eq!(directory_branch_value(&branch, true), "directory-a");
    assert_eq!(directory_branch_value(&branch, false), "auto-generated");
    assert_eq!(
        directory_branch_name(&branch),
        "auto-generated",
        "directory records never claim to own pending file content"
    );
    let mut model = Model {
        repository_id: "repository-a".into(),
        directory_branches: vec![branch],
        ..Default::default()
    };
    assert_eq!(
        selected_directory_branch(&model),
        Some((
            "repository-a".into(),
            "directory-a".into(),
            "auto-generated".into()
        ))
    );
    model.directory_branches.clear();
    assert!(
        selected_directory_branch(&model).is_none(),
        "initial namespace main has no invented rename target"
    );
    let mut invalid =
        crate::files::test_entries::branch_wire(json!({"name":"main","current":true}));
    invalid["directory_branch_id"] = Value::Null;
    assert!(DirectoryBranch::parse(invalid).is_err());
}

#[test]
fn selected_branch_rename_uses_the_admitted_state_and_alias_disposition() {
    for (state, alias, read_only) in [
        ("active", None, false),
        ("active", Some("survivor"), true),
        ("archived", None, true),
        ("migrated", None, true),
        ("merged", Some("survivor"), true),
    ] {
        let mut model = Model {
            selected: Some(entry(
                json!({"file_id":"file","selected_branch_id":"branch"}),
            )),
            ..Default::default()
        };
        loaded(&mut model, chain(1));
        model.history.branches = Some(branch_page(
            vec![branch_wire(json!({
                "state":state,"alias_of":alias,"read_only":read_only,"current":true
            }))],
            Some("branch"),
        ));
        assert_eq!(selected_branch(&model), "branch");
        assert_eq!(selected_file_branch_record(&model).is_some(), !read_only);
    }
}
#[test]
fn renaming_an_entry_preserves_selection_and_file_history() {
    let mut model = Model {
        selected: Some(entry(
            json_codec::json!({"entry_id":"file-a","name":"old.txt","path":"old.txt"}),
        )),
        entries: vec![entry(
            json_codec::json!({"entry_id":"file-a","name":"new.txt","path":"new.txt"}),
        )]
        .into(),
        ..Default::default()
    };
    loaded(&mut model, chain(1));
    model.reconcile_selection(model.entries.first().cloned());
    assert_eq!(model.selected.unwrap().name(), "new.txt");
    assert_eq!(model.history.loaded().unwrap().nodes[0].node_id, "n1");
    assert_eq!(
        model.history.generation, 1,
        "old path responses are invalidated"
    );
}
#[test]
fn file_context_uses_the_logical_alias_instead_of_the_container_owner() {
    let alias = entry(
        json!({"entry_id":"f","file_id":"f","name":"old.txt","path":"disk/a/old.txt","repository_id":"repo","repository_path":"a/old.txt"}),
    );
    assert_eq!(
        file_context(&alias),
        Some(("repo".into(), "a/old.txt".into()))
    );
    let other = entry(json!({"entry_id":"f","file_id":"f","path":"b/new.txt"}));
    assert_ne!(entry_key(&alias), entry_key(&other));
}

#[test]
fn two_aliases_of_one_file_keep_the_selected_logical_path() {
    let mut model = Model {
        selected: Some(entry(json_codec::json!({"entry_id":"f","path":"b/f.txt"}))),
        entries: vec![
            entry(json_codec::json!({"entry_id":"f","path":"a/f.txt","version_id":"F1"})),
            entry(json_codec::json!({"entry_id":"f","path":"b/f.txt","version_id":"F2"})),
        ]
        .into(),
        ..Default::default()
    };
    model.reconcile_selection(model.entries.get(1).cloned());
    assert_eq!(model.selected.as_ref().unwrap().version_id().unwrap(), "F2");
    update(
        model.selected.as_mut().unwrap(),
        json!({"path":"missing/f.txt"}),
    );
    model.reconcile_selection(None);
    assert!(model.selected.is_none());
}
#[test]
fn a_removed_entry_discards_metadata() {
    let mut model = Model {
        selected: Some(entry(json_codec::json!({"entry_id":"file-a"}))),
        ..Default::default()
    };
    loaded(&mut model, chain(1));
    model.history.generation = 4;
    model.history.begin_main(None);
    model.reconcile_selection(model.entries.first().cloned());
    assert!(model.selected.is_none());
    assert!(model.history.window().is_none());
    assert!(!model.history.loading());
    assert_eq!(model.history.generation, 6);
}

#[test]
fn disk_list_accepts_current_contract_and_rejects_legacy_or_partial_records() {
    let disk = json!({"slot_id":"P01", "online":true, "writable":false, "available_bytes":"18446744073709551615"});
    let parsed = parse_disks(&json!({"disks":[disk.clone()]})).unwrap();
    assert_eq!(parsed[0].slot_id, "P01");
    assert_eq!(parsed[0].available_bytes, u64::MAX);
    assert!(!parsed[0].writable);
    assert!(parse_disks(&json!({"disks":[]})).unwrap().is_empty());
    for invalid in [
        json!([disk.clone()]),
        json!({"disks":[{"id":"legacy", "online":true, "writable":true,"available_bytes":"0"}]}),
        json!({"disks":[disk, {"slot_id":"P02"}]}),
        json!({"disks":null}),
    ] {
        assert!(parse_disks(&invalid).is_err());
    }
}

#[test]
fn refreshed_checked_entries_preserve_exact_alias_paths() {
    let kept = entry(json!({"entry_id":"shared", "path":"a/file.txt"}));
    let removed = entry(json!({"entry_id":"shared", "path":"b/file.txt"}));
    let new_alias = entry(json!({"entry_id":"shared", "path":"c/file.txt"}));
    let unrelated = entry(json!({"entry_id":"other", "path":"other.txt"}));
    let mut model = Model {
        checked_entries: [entry_key(&kept), entry_key(&removed), entry_key(&unrelated)]
            .into_iter()
            .collect(),
        entries: vec![kept.clone(), new_alias, unrelated.clone()].into(),
        ..Default::default()
    };
    model.reconcile_selection(model.entries.first().cloned());
    assert_eq!(
        model.checked_entries,
        [entry_key(&kept), entry_key(&unrelated)]
            .into_iter()
            .collect()
    );
    model.entries = Default::default();
    model.reconcile_selection(model.entries.first().cloned());
    assert!(model.checked_entries.is_empty());
}

#[test]
fn empty_checked_entries_still_reconcile_the_detail_selection() {
    let old = entry(json!({"entry_id":"file", "path":"old.txt", "file_id":"file"}));
    let renamed = entry(json!({"entry_id":"file", "path":"new.txt", "file_id":"file"}));
    let mut model = Model {
        selected: Some(old),
        entries: vec![renamed.clone()].into(),
        ..Default::default()
    };
    model.reconcile_selection(model.entries.first().cloned());
    assert!(model.checked_entries.is_empty());
    assert_eq!(model.selected, Some(renamed));
    assert_eq!(model.history.generation, 1);
}

#[test]
fn targeted_refresh_preserves_details_outside_the_loaded_page() {
    let selected = entry(json!({"entry_id":"outside", "path":"other.txt", "version_id":"v2"}));
    let mut model = Model {
        selected: Some(selected.clone()),
        entries: vec![entry(json!({"entry_id":"page", "path":"first.txt"}))].into(),
        ..Default::default()
    };
    model.reconcile_selection(Some(selected.clone()));
    assert_eq!(model.selected, Some(selected));
    model.reconcile_selection(None);
    assert!(model.selected.is_none());
}
