use super::*;
use json_codec::json;

const ROOT: DirectoryKey = DirectoryKey(String::new());

fn overlay(mut value: Value, patch: &Value) -> Value {
    for (key, field) in patch.as_object().unwrap() {
        value[key] = field.clone();
    }
    value
}

/// Test fixtures complete the current wire shape, then enter the real parser.
pub(crate) fn entry_wire(patch: Value) -> Value {
    let directory = patch["kind"] == "directory";
    let path = patch["path"]
        .as_str()
        .or_else(|| patch["name"].as_str())
        .unwrap_or("file.txt");
    let name = path.rsplit('/').next().unwrap();
    let mut value = json!({"entry_id":"entry", "name":name, "path":path,
        "kind":if directory {"directory"} else {"file"}, "size":"0",
        "file_id":if directory {""} else {"file"}, "version_id":if directory {""} else {"version"}});
    let backup = patch["backup"] == true;
    let historical =
        patch["historical_only"] == true || patch.get("repository_path") == Some(&Value::Null);
    if !directory || patch.get("repository_id").is_some() {
        value = overlay(
            value,
            &json!({"repository_id":"repo", "repository_path":path,
            "container":!directory,"selected_branch_id":null,"note":""}),
        );
    }
    if !directory {
        value = overlay(
            value,
            &json!({"node_id":"node","read_only":backup,
            "materialized":true,"inherited":false,"own_version_id":"version",
            "source_version_id":null,"content_state":"LIVE","visible_content_state":"LIVE"}),
        );
    }
    value = overlay(value, &patch);
    if backup {
        value["read_only"] = true.into();
        if !directory {
            value["historical_only"] = historical.into();
            if historical {
                value["repository_path"] = Value::Null;
                value["selected_branch_id"] = Value::Null;
            }
        }
    }
    if patch["backup"] == false {
        value.as_object_mut().unwrap().remove("backup");
    }
    if !directory {
        fixture_content(&mut value, &patch);
    }
    value
}
fn fixture_content(value: &mut Value, patch: &Value) {
    let inherited = value["inherited"] == true
        || patch
            .get("source_version_id")
            .is_some_and(|source| !source.is_null())
        || patch["materialized"] == false && !value["version_id"].is_null()
        || patch["content_state"] == "PENDING";
    if !patch.as_object().unwrap().contains_key("source_version_id") {
        value["source_version_id"] = if inherited {
            value["version_id"].as_str().unwrap_or("").into()
        } else {
            Value::Null
        };
    }
    if !patch.as_object().unwrap().contains_key("inherited") {
        value["inherited"] = inherited.into();
    }
    if !patch.as_object().unwrap().contains_key("materialized") {
        value["materialized"] = (!inherited && !value["version_id"].is_null()).into();
    }
    if !patch.as_object().unwrap().contains_key("own_version_id") {
        value["own_version_id"] = if inherited {
            Value::Null
        } else {
            value["version_id"].clone()
        };
    }
    if !patch.as_object().unwrap().contains_key("content_state") {
        value["content_state"] = if inherited {
            json!("PENDING")
        } else {
            value["visible_content_state"].clone()
        };
    }
}
pub(crate) fn parse_entry(value: Value) -> Result<Entry, api::Error> {
    super::entry::parse(value).map_err(Into::into)
}
pub(crate) fn entry(patch: Value) -> Entry {
    super::entry::parse(entry_wire(patch)).unwrap()
}
pub(crate) fn branch_wire(patch: Value) -> Value {
    overlay(
        json!({"directory_branch_id":"directory", "repository_id":"repo",
        "directory_id":"directory-id", "name":"main", "parent_branch_id":null,
        "head_tree_id":"tree", "commit_id":"commit", "path":"", "materialized":true,
        "current":false, "sequence":"1", "file_count":"0"}),
        &patch,
    )
}
pub(crate) fn branch(patch: Value) -> DirectoryBranch {
    DirectoryBranch::parse(branch_wire(patch)).unwrap()
}
pub(crate) fn wire(entry: &Entry) -> Value {
    let mut patch = json!({"entry_id":entry.entry_id(), "name":entry.name(), "path":entry.path(),
        "kind":entry.kind(), "size":entry.size().to_string(), "file_id":entry.file_id()});
    if let Some(location) = &entry.location {
        patch["repository_id"] = location.repository_id.clone().into();
        patch["repository_path"] = entry
            .repository_path()
            .map_or(Value::Null, |path| path.into());
        patch["selected_branch_id"] = entry
            .selected_branch_id()
            .map_or(Value::Null, |id| id.into());
        patch["note"] = entry.note().unwrap().into();
    }
    if entry.kind() == NamespaceEntryKind::File {
        patch["version_id"] = entry.version_id().map_or(Value::Null, |id| id.into());
        patch["node_id"] = entry.node_id().unwrap_or("").into();
        patch["materialized"] = entry.materialized().into();
        patch["inherited"] = entry.inherited().into();
        patch["read_only"] = entry.read_only().into();
    }
    if entry.backup() {
        patch["backup"] = true.into();
    }
    entry_wire(patch)
}
pub(crate) fn update(entry: &mut Entry, patch: Value) {
    let mut value = wire(entry);
    if patch.get("kind").is_some_and(|kind| kind != &value["kind"]) {
        for field in [
            "file_id",
            "version_id",
            "size",
            "node_id",
            "materialized",
            "inherited",
            "own_version_id",
            "source_version_id",
            "content_state",
            "visible_content_state",
            "read_only",
            "historical_only",
            "container",
            "selected_branch_id",
        ] {
            value.as_object_mut().unwrap().remove(field);
        }
        if patch["kind"] == "directory" && value["backup"] == true {
            for field in ["repository_id", "repository_path", "note"] {
                value.as_object_mut().unwrap().remove(field);
            }
        }
        value = entry_wire(overlay(value, &patch));
    }
    if patch.get("path").is_some() {
        let path = patch["path"].as_str().unwrap();
        if patch.get("name").is_none() {
            value["name"] = path.rsplit('/').next().unwrap().into();
        }
        if patch.get("repository_path").is_none()
            && value
                .get("repository_path")
                .is_some_and(|path| !path.is_null())
        {
            value["repository_path"] = path.into();
        }
    }
    value = overlay(value, &patch);
    if value["kind"] == "file" {
        fixture_content(&mut value, &patch);
    }
    if patch["backup"] == true {
        value["read_only"] = true.into();
        if value["kind"] == "file" {
            value["historical_only"] = value["repository_path"].is_null().into();
        } else {
            for field in [
                "repository_id",
                "repository_path",
                "note",
                "container",
                "selected_branch_id",
            ] {
                value.as_object_mut().unwrap().remove(field);
            }
        }
    }
    if patch["backup"] == false {
        value.as_object_mut().unwrap().remove("backup");
        value.as_object_mut().unwrap().remove("historical_only");
        if value["kind"] == "directory" {
            value.as_object_mut().unwrap().remove("read_only");
        } else if patch.get("read_only").is_none() {
            value["read_only"] = false.into();
        }
    }
    *entry = super::entry::parse(value).unwrap();
}
fn response(count: usize) -> Value {
    let entries = (0..count)
        .map(|index| {
            entry_wire(json!({"entry_id":format!("entry-{index}"),
        "file_id":format!("file-{index}"),"path":format!("file-{index}.txt")}))
        })
        .collect::<Vec<_>>();
    json!({"slot_id":"P01","path":"","writable":true,"entries":entries,"selected_entry":null,
        "total_entries":"100000","snapshot":"snapshot","next_cursor":"next","previous_cursor":null,
        "context":{"repository_id":"repo","repository_path":"","branch_id":"main","base_head":"head"},
        "directory_file_count":"100000","directory_branches":[],"directory_branches_supported":true,"default_branch_scope":true})
}
fn parse(value: Value) -> Result<Page, api::Error> {
    Page::parse(value, "P01", &ROOT, PAGE_SIZE, None)
}
#[test]
fn destination_request_identity_uses_the_parsed_target_key() {
    let target = name_policy::PortablePath::parse("Café/资料").unwrap();
    for prefix in ["", "Repository"] {
        let directory = DirectoryKey::destination(prefix, Some(&target));
        let path = crate::files::model::join_path(prefix, target.as_str());
        assert_eq!(directory.0, name_policy::path_key(&path));
        assert_eq!(
            DirectoryKey::destination(prefix, None).0,
            name_policy::path_key(prefix)
        );
    }
}
#[test]
fn bounds_page_by_request_and_preserves_large_decimal_values() {
    assert_eq!(parse(response(200)).unwrap().into_parts().total, 100000);
    assert!(parse(response(201)).is_err());
    assert!(Page::parse(response(2), "P01", &ROOT, 1, None).is_err());
    let mut value = response(1);
    value["entries"][0]["size"] = u64::MAX.to_string().into();
    assert_eq!(
        parse(value).unwrap().into_parts().entries[0].size(),
        u64::MAX
    );
}
#[test]
fn every_required_page_field_is_required_even_when_nullable() {
    for key in [
        "slot_id",
        "path",
        "writable",
        "entries",
        "selected_entry",
        "total_entries",
        "snapshot",
        "next_cursor",
        "previous_cursor",
        "context",
        "directory_file_count",
        "directory_branches",
        "directory_branches_supported",
        "default_branch_scope",
    ] {
        let mut value = response(1);
        value.as_object_mut().unwrap().remove(key);
        assert!(parse(value).is_err(), "missing {key}");
    }
}
#[test]
fn rejects_wrong_scalar_types_noncanonical_counts_and_unknown_fields() {
    for (field, bad) in [
        ("total_entries", json!(1)),
        ("total_entries", json!("01")),
        ("total_entries", json!("18446744073709551616")),
        ("directory_file_count", json!(null)),
        ("writable", json!("true")),
        ("entries", json!({})),
        ("context", json!([])),
        ("snapshot", json!("")),
        ("snapshot", json!(null)),
        ("next_cursor", json!("")),
        ("previous_cursor", json!(false)),
        ("extra", json!(true)),
        ("slot_id", json!("P02")),
        ("path", json!("other")),
        ("backup", json!(false)),
    ] {
        let mut value = response(1);
        value[field] = bad;
        let error = match parse(value) {
            Err(error) => error,
            Ok(_) => panic!("accepted {field}"),
        };
        assert_eq!(
            error.code,
            storage_contract::ErrorCode::InvalidDirectoryPage
        );
        assert!(matches!(
            error.source.as_deref(),
            Some(api::ErrorSource::DirectoryPage(_))
        ));
    }
    let mut value = response(1);
    value["next_cursor"] = "a".repeat(32_769).into();
    assert!(parse(value).is_err());
}
#[test]
fn rejects_truncated_file_shapes_null_identities_and_kind_confusion() {
    let fixture = entry_wire(json!({}));
    for key in fixture.as_object().unwrap().keys() {
        let mut missing = fixture.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(super::entry::parse(missing).is_err(), "missing {key}");
    }
    for (field, bad) in [
        ("entry_id", json!(null)),
        ("entry_id", json!("")),
        ("file_id", json!("")),
        ("kind", json!("symlink")),
        ("kind", json!({"file":null})),
        ("kind", json!("directory")),
        ("name", json!("different.txt")),
        ("path", json!("../file.txt")),
        ("repository_path", json!(null)),
        ("version_id", json!("")),
        ("materialized", json!(false)),
        ("source_version_id", json!("other")),
        ("read_only", json!(null)),
    ] {
        let mut value = fixture.clone();
        value[field] = bad;
        assert!(super::entry::parse(value).is_err(), "invalid {field}");
    }
}
#[test]
fn virtual_current_and_backup_directories_remain_distinct() {
    let virtual_entry =
        entry(json!({"kind":"directory", "entry_id":"repo-id", "path":"Repository"}));
    assert!(virtual_entry.context().is_none());
    assert_eq!(virtual_entry.file_id(), "");
    assert_eq!(virtual_entry.version_id(), None);
    let current = entry(json!({"kind":"directory", "repository_id":"repo", "path":"dir"}));
    assert_eq!(current.context(), Some(("repo", "dir")));
    let backup = entry(
        json!({"kind":"directory", "entry_id":"backupdir_hash", "path":"P01", "backup":true}),
    );
    assert!(backup.backup() && backup.read_only());
    let mut wrong = wire(&virtual_entry);
    wrong["note"] = "".into();
    assert!(super::entry::parse(wrong).is_err());
}
#[test]
fn pending_empty_source_and_backup_version_override_keep_real_semantics() {
    let pending = entry(
        json!({"version_id":null,"source_version_id":"", "inherited":true, "materialized":false}),
    );
    assert!(pending.inherited());
    assert!(!pending.visible_content());
    let inherited = entry(json!({"version_id":"source","source_version_id":"source"}));
    assert!(inherited.inherited_content());
    assert!(!inherited.materialized());
    let backup = entry(
        json!({"backup":true,"version_id":"stored","source_version_id":"source","inherited":true,
        "materialized":false,"own_version_id":null,"historical_only":false}),
    );
    assert_eq!(backup.version_id(), Some("stored"));
    assert!(backup.inherited_content() && backup.read_only());
    let historical = entry(json!({"backup":true,"historical_only":true,"repository_path":null}));
    assert_eq!(historical.repository_id(), Some("repo"));
    assert!(historical.context().is_none());
}
#[test]
fn identities_are_path_scoped_but_not_reused_for_different_entities() {
    let mut value = response(2);
    value["entries"][1]["entry_id"] = value["entries"][0]["entry_id"].clone();
    assert!(parse(value.clone()).is_err());
    value["entries"][1]["file_id"] = value["entries"][0]["file_id"].clone();
    assert!(parse(value.clone()).is_ok());
    value["entries"][1] = value["entries"][0].clone();
    assert!(parse(value).is_err());
    let mut value = response(1);
    value["entries"][0] = entry_wire(json!({"path":"outside/file.txt"}));
    assert!(parse(value).is_err());
}
#[test]
fn selected_observation_is_independent_of_page_and_matches_requested_identity() {
    let requested = entry(json!({"entry_id":"outside","path":"Other.txt"}));
    let mut value = response(1);
    value["selected_entry"] = wire(&requested);
    let page = Page::parse(value.clone(), "P01", &ROOT, PAGE_SIZE, Some(&requested)).unwrap();
    assert_eq!(page.into_parts().selected, Some(requested.clone()));
    assert!(parse(value.clone()).is_err());
    value["selected_entry"]["entry_id"] = "replacement".into();
    assert!(Page::parse(value, "P01", &ROOT, PAGE_SIZE, Some(&requested)).is_err());
    assert!(
        Page::parse(response(1), "P01", &ROOT, PAGE_SIZE, Some(&requested))
            .unwrap()
            .into_parts()
            .selected
            .is_none()
    );
}
#[test]
fn filtered_empty_page_preserves_an_independent_selected_file() {
    let mut selected = json!({"entry_id":"selected", "name":"Other.txt", "path":"Other.txt",
        "kind":"file", "size":"0", "file_id":"file", "version_id":"version",
        "repository_id":"repo", "repository_path":"Other.txt", "container":true,
        "selected_branch_id":null, "note":"", "node_id":"node", "read_only":false,
        "materialized":true, "inherited":false, "own_version_id":"version",
        "source_version_id":null, "content_state":"LIVE", "visible_content_state":"LIVE"});
    for pending in [false, true] {
        if pending {
            selected["version_id"] = Value::Null;
            selected["own_version_id"] = Value::Null;
            selected["source_version_id"] = "".into();
            selected["materialized"] = false.into();
            selected["inherited"] = true.into();
            selected["content_state"] = "PENDING".into();
        }
        let requested = parse_entry(selected.clone()).unwrap();
        // Search filters the page count, not the independent selection or directory file count.
        let value = json!({"slot_id":"P01", "path":"", "writable":true, "entries":[],
            "selected_entry":selected.clone(), "total_entries":"0", "snapshot":"snapshot",
            "next_cursor":null, "previous_cursor":null,
            "context":{"repository_id":"repo", "repository_path":"", "branch_id":"main", "base_head":"head"},
            "directory_file_count":"1", "directory_branches":[],
            "directory_branches_supported":true, "default_branch_scope":true});
        let page = Page::parse(value, "P01", &ROOT, PAGE_SIZE, Some(&requested))
            .unwrap()
            .into_parts();
        assert!(page.entries.is_empty());
        assert_eq!(page.total, 0);
        assert_eq!(page.directory_file_count, 1);
        assert!(page.next.is_none() && page.previous.is_none());
        assert_eq!(requested.inherited(), pending);
        assert_eq!(requested.visible_content(), !pending);
        assert_eq!(page.selected, Some(requested));
    }
}
#[test]
fn canonical_lookup_context_accepts_nfd_without_accepting_malformed_display_paths() {
    let selected = entry(json!({"entry_id":"selected","path":"Café/Å.txt"}));
    let mut value = response(1);
    value["path"] = "Cafe\u{301}".into();
    value["entries"][0] = wire(&selected);
    value["selected_entry"] = wire(&selected);
    value["context"]["repository_path"] = "Cafe\u{301}".into();
    value["default_branch_scope"] = false.into();
    let directory = DirectoryKey::navigation("Café");
    assert!(Page::parse(value.clone(), "P01", &directory, PAGE_SIZE, Some(&selected)).is_ok());
    value["entries"][0]["path"] = "Café/../Å.txt".into();
    assert!(Page::parse(value, "P01", &directory, PAGE_SIZE, Some(&selected)).is_err());
}
#[test]
fn branch_summaries_are_complete_and_do_not_invent_same_path_history_constraints() {
    let mut value = response(1);
    value["directory_branches"] =
        json!([branch_wire(json!({"path":"old/location","current":true}))]);
    assert_eq!(
        parse(value.clone()).unwrap().into_parts().branches[0].path(),
        "old/location"
    );
    value["directory_branches"][0]["repository_id"] = "other".into();
    assert!(parse(value).is_err());
    for field in branch_wire(json!({})).as_object().unwrap().keys() {
        let mut value = branch_wire(json!({}));
        value.as_object_mut().unwrap().remove(field);
        assert!(DirectoryBranch::parse(value).is_err(), "missing {field}");
    }
}

#[test]
fn backup_pages_keep_historical_locations_and_require_disk_read_only_flags() {
    let mut value = response(1);
    value["backup"] = true.into();
    value["writable"] = false.into();
    value["context"] = Value::Null;
    value["directory_branches_supported"] = false.into();
    value["default_branch_scope"] = false.into();
    value["entries"][0] = entry_wire(json!({"backup":true, "historical_only":true,
        "path":"stored.txt", "repository_path":null, "version_id":"stored"}));
    let entries = parse(value.clone()).unwrap().into_parts().entries;
    assert!(entries[0].read_only());
    assert!(entries[0].context().is_none());
    for (field, bad) in [
        ("writable", json!(true)),
        ("directory_branches_supported", json!(true)),
        ("default_branch_scope", json!(true)),
    ] {
        let mut bad_page = value.clone();
        bad_page[field] = bad;
        assert!(parse(bad_page).is_err());
    }
    value["entries"][0]["node_id"] = "".into();
    assert!(parse(value).is_err());
}
#[test]
fn context_branches_and_entry_locations_are_admitted_as_one_observation() {
    for field in ["repository_id", "repository_path", "branch_id", "base_head"] {
        let mut value = response(1);
        value["context"].as_object_mut().unwrap().remove(field);
        assert!(parse(value).is_err(), "context missing {field}");
    }
    for patch in [
        json!({"repository_id":"other"}),
        json!({"repository_path":"elsewhere/file-0.txt"}),
    ] {
        let mut value = response(1);
        value["entries"][0] = overlay(value["entries"][0].clone(), &patch);
        assert!(parse(value).is_err());
    }
    let mut value = response(1);
    value["context"] = Value::Null;
    assert!(parse(value).is_err());
    let mut value = response(1);
    value["directory_branches"] = json!([
        branch_wire(json!({"current":true})),
        branch_wire(json!({"directory_branch_id":"other","current":true}))
    ]);
    assert!(parse(value).is_err());
}
#[test]
fn cursors_and_counts_cannot_describe_an_impossible_partial_page() {
    let mut value = response(1);
    value["total_entries"] = "0".into();
    assert!(parse(value).is_err());
    let mut value = response(1);
    value["directory_file_count"] = "0".into();
    assert!(parse(value).is_err());
    let mut value = response(1);
    value["total_entries"] = "1".into();
    assert!(parse(value.clone()).is_err());
    value["next_cursor"] = Value::Null;
    assert!(parse(value).is_ok());
    let mut empty = response(0);
    assert!(parse(empty.clone()).is_err());
    empty["next_cursor"] = Value::Null;
    assert!(parse(empty).is_ok());
}
#[test]
fn source_errors_and_fixture_transitions_preserve_the_boundary() {
    use std::error::Error;
    let mut value = response(1);
    value["entries"][0]["path"] = "..".into();
    let error = parse(value).err().unwrap();
    assert!(error.source().unwrap().source().unwrap().source().is_some());
    let mut selected = entry(json!({"path":"docs/a.txt"}));
    update(&mut selected, json!({"kind":"directory"}));
    assert_eq!(selected.kind(), NamespaceEntryKind::Directory);
    assert_eq!(selected.version_id(), None);
    update(&mut selected, json!({"kind":"file"}));
    update(&mut selected, json!({"backup":true}));
    assert!(selected.read_only());
    update(&mut selected, json!({"backup":false}));
    assert!(!selected.backup());
    assert!(!selected.read_only());
    update(
        &mut selected,
        json!({"path":"docs/renamed.txt", "version_id":"new"}),
    );
    assert_eq!(selected.name(), "renamed.txt");
    assert_eq!(selected.version_id(), Some("new"));
}

#[test]
fn requested_selection_cannot_disappear_while_its_page_entry_remains() {
    let value = response(1);
    let requested = parse_entry(value["entries"][0].clone()).unwrap();
    assert!(Page::parse(value.clone(), "P01", &ROOT, PAGE_SIZE, Some(&requested)).is_err());
    let mut value = value;
    value["selected_entry"] = wire(&requested);
    assert!(Page::parse(value, "P01", &ROOT, PAGE_SIZE, Some(&requested)).is_ok());
}

#[test]
fn malformed_backup_locations_are_rejected_without_fixture_normalization() {
    let valid = entry_wire(json!({"backup":true,"historical_only":true}));
    for (field, value) in [
        ("repository_path", json!("stale/alias.txt")),
        ("selected_branch_id", json!("stale-branch")),
        ("historical_only", json!(false)),
        ("read_only", json!(false)),
        ("node_id", json!("")),
    ] {
        let mut malformed = valid.clone();
        malformed[field] = value;
        assert!(parse_entry(malformed).is_err(), "{field}");
    }
    let mut missing = valid;
    missing.as_object_mut().unwrap().remove("repository_path");
    assert!(parse_entry(missing).is_err());
}

#[test]
fn nonempty_partial_page_requires_a_navigation_cursor() {
    let mut value = response(1);
    value["next_cursor"] = Value::Null;
    assert!(parse(value.clone()).is_err());
    value["previous_cursor"] = "previous".into();
    assert!(parse(value).is_ok());
}

#[test]
fn independent_selection_and_page_share_entity_identity_and_count_constraints() {
    let requested = entry(json!({"entry_id":"shared","file_id":"file-a","path":"selected.txt"}));
    let mut value = response(1);
    value["entries"][0]["entry_id"] = "shared".into();
    value["entries"][0]["file_id"] = "file-b".into();
    value["selected_entry"] = wire(&requested);
    assert!(Page::parse(value.clone(), "P01", &ROOT, PAGE_SIZE, Some(&requested)).is_err());
    value["entries"][0]["file_id"] = "file-a".into();
    assert!(Page::parse(value.clone(), "P01", &ROOT, PAGE_SIZE, Some(&requested)).is_ok());
    value["directory_file_count"] = "1".into();
    assert!(Page::parse(value.clone(), "P01", &ROOT, PAGE_SIZE, Some(&requested)).is_err());
    value["directory_file_count"] = "2".into();
    assert!(Page::parse(value, "P01", &ROOT, PAGE_SIZE, Some(&requested)).is_ok());

    let requested = entry(json!({"kind":"directory","entry_id":"shared","path":"selected-dir"}));
    let mut value = response(1);
    value["entries"][0] =
        entry_wire(json!({"kind":"directory","entry_id":"shared","path":"listed-dir"}));
    value["selected_entry"] = wire(&requested);
    assert!(Page::parse(value, "P01", &ROOT, PAGE_SIZE, Some(&requested)).is_err());
}
