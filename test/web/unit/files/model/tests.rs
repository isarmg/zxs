use super::*;
use json_codec::json;

#[test]
fn refreshed_selection_invalidates_old_history_only_when_context_changes() {
    let original = json_codec::json!({"entry_id":"entry", "file_id":"file", "path":"a.txt", "version_id":"v1", "selected_branch_id":"main"});
    let mut model = Model {
        selected: Some(original.clone()),
        entries: vec![original.clone()].into(),
        file_history: json_codec::json!({"selected_branch_id":"main"}).into(),
        history_loading: true,
        ..Default::default()
    };
    model.reconcile_selection(model.entries.first().cloned());
    assert_eq!(model.history_generation, 0);
    Arc::make_mut(&mut model.entries)[0]["selected_branch_id"] = "draft".into();
    Arc::make_mut(&mut model.entries)[0]["version_id"] = "v2".into();
    model.checked_nodes.insert("old-node".into());
    model.reconcile_selection(model.entries.first().cloned());
    assert_eq!(model.selected.as_ref().unwrap()["version_id"], "v2");
    assert!(model.file_history.is_null());
    assert!(!model.history_loading);
    assert!(model.checked_nodes.is_empty());
    assert_eq!(model.history_generation, 1);
}
#[test]
fn backup_context_never_guesses_a_primary_alias_from_its_prefix() {
    let model = Model {
        repository_id: "wrong-listing-repo".into(),
        repository_path: "wrong-prefix".into(),
        ..Default::default()
    };
    let mut backup = json_codec::json!({"backup":true,"read_only":true,"name":"f.txt","path":"我的备份/f.txt","repository_id":"repo","repository_path":"actual/primary/f.txt"});
    assert_eq!(
        file_context(&model, &backup),
        Some(("repo".into(), "actual/primary/f.txt".into()))
    );
    backup["repository_path"] = Value::Null;
    assert!(file_context(&model, &backup).is_none());
    backup["repository_path"] = "stale/alias.txt".into();
    backup["historical_only"] = true.into();
    assert!(file_context(&model, &backup).is_none());
    backup.as_object_mut().unwrap().remove("historical_only");
    backup.as_object_mut().unwrap().remove("repository_path");
    assert!(file_context(&model, &backup).is_none());
}

#[test]
fn backup_history_keeps_its_stored_body_and_current_node() {
    let entry = json_codec::json!({"backup":true,"read_only":true,"version_id":"stored-old","node_id":"stored-old"});
    let history = json_codec::json!({"version_id":"primary-new","node_id":"primary-new","nodes":[{"node_id":"stored-old","version_id":"stored-old","current":false},{"node_id":"primary-new","version_id":"primary-new","current":true}],"versions":[{"version_id":"stored-old"},{"version_id":"primary-new","current":true}]});
    let result = history_for_entry(history.clone(), &entry);
    assert_eq!(result["version_id"], "stored-old");
    assert_eq!(result["current_node_id"], "stored-old");
    assert_eq!(result["nodes"][0]["current"], true);
    assert_eq!(result["nodes"][1]["current"], false);
    assert_eq!(result["versions"][0]["current"], true);
    assert_eq!(result["read_only"], true);
    assert_eq!(
        history_for_entry(
            history.clone(),
            &json_codec::json!({"version_id":"stored-old"})
        ),
        history
    );
    let mut model = Model {
        writable: true,
        selected: Some(entry),
        file_history: json_codec::json!({"branches":[{"branch_id":"main","current":true}]}).into(),
        ..Default::default()
    };
    assert!(selected_entry_read_only(&model));
    model.selected = Some(json_codec::json!({"read_only":true}));
    assert!(selected_entry_read_only(&model));
}
const DEGRADED: &str = include_str!("../fixtures/degraded-save-operation.json");
const DAMAGE: &str = include_str!("../fixtures/damaged-published-save-operation.json");
#[test]
fn publication_protection_is_not_rewritten_by_later_damage() {
    let damage: Value = json_codec::from_str(DAMAGE).unwrap();
    // Derive a healthy comparison from this real receipt, changing only
    // current health. Frozen publication evidence must give the same label.
    let mut good = damage.clone();
    good["current_health"]["state"] = "HEALTHY".into();
    assert_eq!(good["result"], damage["result"]);
    assert_eq!(operation_status(&good), SaveStatus::Published);
    assert_eq!(operation_status(&damage), SaveStatus::Published);
}
#[test]
fn permanent_loss_is_distinct_from_ordinary_save() {
    let loss: Value = json_codec::from_str(DEGRADED).unwrap();
    assert_eq!(operation_status(&loss), SaveStatus::Degraded);
    assert!(publication_notice(SaveStatus::Degraded).contains("Permanently lost"));
}
#[test]
fn uncertain_save_cannot_be_relabelled_as_a_failure() {
    assert_eq!(
        operation_status(&json_codec::json!({"state":"in_doubt"})),
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
fn visible_inherited_content_is_readable_without_becoming_owned_content() {
    let pending = json_codec::json!({"materialized":false,"content_state":"PENDING","version_id":null,"source_version_id":"source"});
    assert!(!materialized(&pending));
    assert!(!visible_content(&pending));
    let inherited = json_codec::json!({"materialized":false,"inherited":true,"version_id":"source","own_version_id":null,"source_version_id":"source","size":"6"});
    assert!(visible_content(&inherited));
    assert!(inherited_content(&inherited));
    assert!(!materialized(&inherited));
    assert!(!materialized(
        &json_codec::json!({"version_id":null,"source_version_id":"source"})
    ));
    assert!(!materialized(
        &json_codec::json!({"materialized":false,"version_id":"source"})
    ));
    assert!(!materialized(
        &json_codec::json!({"materialized":true,"version_id":null,"source_version_id":"source"})
    ));
    assert!(materialized(&json_codec::json!({"version_id":"body"})));
    assert!(materialized(
        &json_codec::json!({"materialized":true,"version_id":"saved","size":"0"})
    ));
    assert!(!inherited_content(
        &json_codec::json!({"materialized":true,"inherited":false,"version_id":"saved","own_version_id":"saved","size":"0"})
    ));
    assert!(
        branch_name(&json_codec::json!({"name":"draft","materialized":false}))
            .contains("Pending content")
    );
    assert_eq!(branch_name(&json_codec::json!({"name":"legacy"})), "legacy");
}
#[test]
fn current_branch_identity_does_not_require_a_content_version() {
    let mut model = Model {
        file_history: json_codec::json!({"selected_branch_id":"pending","branches":[{"branch_id":"pending","materialized":false,"version_id":null}]}).into(),
        ..Default::default()
    };
    assert_eq!(selected_branch(&model), "pending");
    model.file_history = Value::Null.into();
    model.selected = Some(json_codec::json!({"selected_branch_id":"entry-pending"}));
    assert_eq!(selected_branch(&model), "entry-pending");
}
#[test]
fn an_authoritative_history_node_list_does_not_duplicate_its_body_versions() {
    let mut history = json_codec::json!({"versions":[{"version_id":"ver-one"}],"nodes":[{"node_id":"fnode-rename","version_id":"ver-one"}]});
    assert_eq!(history_nodes(&history).len(), 1);
    assert_eq!(history_node_id(&history_nodes(&history)[0]), "fnode-rename");
    history["nodes"] = json_codec::json!([]);
    assert!(history_nodes(&history).is_empty());
    history.as_object_mut().unwrap().remove("nodes");
    assert!(history_nodes(&history).is_empty());
    assert!(history_node_id(&json_codec::json!({"version_id":"ver-one"})).is_empty());
}
#[test]
fn generated_public_file_branch_names_are_editable_only_after_loading_a_real_record() {
    let mut model = Model {
        selected: Some(json_codec::json!({"file_id":"file-a","selected_branch_id":"generated-a"})),
        ..Default::default()
    };
    assert!(selected_file_branch_record(&model).is_none());
    model.file_history = json_codec::json!({"selected_branch_id":"generated-a","branches":[{"branch_id":"generated-a","name":"方案甲-7c2a","materialized":true},{"branch_id":"other","name":"other"}]}).into();
    assert_eq!(
        selected_file_branch_record(&model),
        Some(("file-a".into(), "generated-a".into(), "方案甲-7c2a".into()))
    );
    model.file_history["branches"][0]["name"] = "用户改名".into();
    assert_eq!(
        selected_file_branch_record(&model).unwrap().1,
        "generated-a"
    );
    assert_eq!(selected_file_branch_record(&model).unwrap().2, "用户改名");
}
#[test]
fn root_default_selection_uses_real_directory_ids_and_subfolders_keep_names() {
    let branch = json_codec::json!({"directory_branch_id":"directory-a","name":"auto-generated","current":true,"materialized":false});
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
    model
        .directory_branches
        .push(json_codec::json!({"name":"main","current":true,"directory_branch_id":null}));
    assert!(selected_directory_branch(&model).is_none());
}
#[test]
fn renaming_an_entry_preserves_selection_and_file_history() {
    let mut model = Model {
        selected: Some(json_codec::json!({"entry_id":"file-a","name":"old.txt","path":"old.txt"})),
        entries: vec![json_codec::json!({"entry_id":"file-a","name":"new.txt","path":"new.txt"})]
            .into(),
        file_history: json_codec::json!({"versions":[{"version_id":"v1"}]}).into(),
        ..Default::default()
    };
    model.reconcile_selection(model.entries.first().cloned());
    assert_eq!(model.selected.unwrap()["name"], "new.txt");
    assert_eq!(model.file_history["versions"][0]["version_id"], "v1");
    assert_eq!(
        model.history_generation, 1,
        "old path responses are invalidated"
    );
}
#[test]
fn file_context_uses_the_logical_alias_instead_of_the_container_owner() {
    let model = Model {
        repository_id: "repo".into(),
        repository_path: "a".into(),
        ..Default::default()
    };
    let alias = json_codec::json!({"entry_id":"f","file_id":"f","name":"old.txt","path":"disk/a/old.txt","repository_id":"repo","repository_path":"a/old.txt","container_path":"b/new.txt"});
    assert_eq!(
        file_context(&model, &alias),
        Some(("repo".into(), "a/old.txt".into()))
    );
    assert_eq!(
        file_context(&model, &json_codec::json!({"name":"fallback.txt"})),
        Some(("repo".into(), "a/fallback.txt".into()))
    );
    assert_ne!(
        entry_key(&alias),
        entry_key(&json_codec::json!({"entry_id":"f","path":"b/new.txt"}))
    );
}
#[test]
fn two_aliases_of_one_file_keep_the_selected_logical_path() {
    let mut model = Model {
        selected: Some(json_codec::json!({"entry_id":"f","path":"b/f.txt"})),
        entries: vec![
            json_codec::json!({"entry_id":"f","path":"a/f.txt","version_id":"F1"}),
            json_codec::json!({"entry_id":"f","path":"b/f.txt","version_id":"F2"}),
        ]
        .into(),
        ..Default::default()
    };
    model.reconcile_selection(model.entries.get(1).cloned());
    assert_eq!(model.selected.as_ref().unwrap()["version_id"], "F2");
    model.selected.as_mut().unwrap()["path"] = "missing/f.txt".into();
    model.reconcile_selection(None);
    assert!(model.selected.is_none());
}
#[test]
fn a_removed_entry_discards_metadata() {
    let mut model = Model {
        selected: Some(json_codec::json!({"entry_id":"file-a"})),
        file_history: json_codec::json!({"versions":[{"version_id":"v1"}]}).into(),
        history_loading: true,
        history_generation: 4,

        ..Default::default()
    };
    model.reconcile_selection(model.entries.first().cloned());
    assert!(model.selected.is_none());
    assert!(model.file_history.is_null());
    assert!(!model.history_loading);
    assert_eq!(model.history_generation, 5);
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
    let kept = json!({"entry_id":"shared", "path":"a/file.txt"});
    let removed = json!({"entry_id":"shared", "path":"b/file.txt"});
    let new_alias = json!({"entry_id":"shared", "path":"c/file.txt"});
    let unrelated = json!({"entry_id":"other", "path":"other.txt"});
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
    let old = json!({"entry_id":"file", "path":"old.txt", "file_id":"file"});
    let renamed = json!({"entry_id":"file", "path":"new.txt", "file_id":"file"});
    let mut model = Model {
        selected: Some(old),
        entries: vec![renamed.clone()].into(),
        ..Default::default()
    };
    model.reconcile_selection(model.entries.first().cloned());
    assert!(model.checked_entries.is_empty());
    assert_eq!(model.selected, Some(renamed));
    assert_eq!(model.history_generation, 1);
}

#[test]
fn targeted_refresh_preserves_details_outside_the_loaded_page() {
    let selected = json!({"entry_id":"outside", "path":"other.txt", "version_id":"v2"});
    let mut model = Model {
        selected: Some(selected.clone()),
        entries: vec![json!({"entry_id":"page", "path":"first.txt"})].into(),
        ..Default::default()
    };
    model.reconcile_selection(Some(selected.clone()));
    assert_eq!(model.selected, Some(selected));
    model.reconcile_selection(None);
    assert!(model.selected.is_none());
}
