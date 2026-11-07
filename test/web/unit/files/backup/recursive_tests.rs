use super::*;
use crate::files::model::{FileOperation, Model, Navigation, SaveStatus};

fn draft() -> Draft {
    Draft {
        id: "draft".into(),
        repository: "repo".into(),
        path: "docs".into(),
        expected_scope: Some("scope".into()),
        load: Load::Pending,
    }
}
fn response() -> Value {
    let identity = |slot: &str| json!({"pool_id":"pool","disk_id":format!("disk-{slot}"),"slot_id":slot,"binding_generation":u64::MAX.to_string()});
    json!({
        "configuration": {
            "scope":{"id":"scope","kind":"directory","path":"docs"},
            "generation":u64::MAX.to_string(),"effective_generation":"7","inherited":false,
            "policy_id":null,"effective_policy_id":null,"target_copies":3,
            "backup_prefixes":{"B01":"old"},"backup_routes":[["P01","B01","C01"]],
            "max_copies":3,"file_count":"1","backup_paths":{"B01":"old/file"}
        },
        "capture": {
            "format":"recursive-backup-source-paths","source_sequence":u64::MAX.to_string(),
            "source_business_generation":u64::MAX.to_string(),
            "registry":{"root_hash":"a".repeat(64),"view_generation":u64::MAX.to_string()},
            "disks": {
                "P01":{"identity":identity("P01"),"level":1,"upstream_slot_id":null,"name":"磁盘A"},
                "B01":{"identity":identity("B01"),"level":2,"upstream_slot_id":"P01","name":"磁盘B"},
                "C01":{"identity":identity("C01"),"level":3,"upstream_slot_id":"B01","name":"磁盘C"}
            }
        }
    })
}
fn ready() -> Draft {
    let mut draft = draft();
    draft.load = Load::Ready(Box::new(read_response(response(), &draft).unwrap()));
    draft
}
#[test]
fn detail_entry_uses_file_identity_for_files_and_entry_identity_for_directories() {
    for (kind, scope_id) in [("file", "file-identity"), ("directory", "entry-identity")] {
        let mut wire =
            json!({"kind":kind,"entry_id":"entry-identity","path":"docs","repository_id":"repo"});
        if kind == "file" {
            wire["file_id"] = json!("file-identity");
        }
        let entry = crate::files::page::tests::entry(wire);
        assert_eq!(entry_scope(&entry), scope_id);
        let mut draft = draft();
        draft.expected_scope = Some(entry_scope(&entry).into());
        let mut value = response();
        value["configuration"]["scope"]["id"] = json!(scope_id);
        value["configuration"]["scope"]["kind"] = json!(kind);
        draft.load = Load::Ready(Box::new(read_response(value, &draft).unwrap()));
        assert_eq!(request(&draft).unwrap()["expected_scope_id"], scope_id);
    }
}
fn unauthorized() -> api::Error {
    api::Error {
        code: storage_contract::ErrorCode::Unauthorized,
        message: "expired".into(),
        retryable: false,
        status: 401,
        request_id: Some("recursive-read".into()),
        source: None,
    }
}

#[test]
fn recursive_wire_preserves_the_entire_capture_and_u64_strings_without_custom_prefixes() {
    let draft = ready();
    let body = request(&draft).unwrap();
    assert_eq!(body["capture"], response()["capture"]);
    assert_eq!(body["expected_generation"], u64::MAX.to_string());
    assert_eq!(body["expected_scope_id"], "scope");
    assert_eq!(body["path"], "docs");
    assert_eq!(body["target_copies"], 3);
    assert!(body.get("backup_prefixes").is_none());
    assert!(body.get("operation_id").is_none()); // The publication boundary allocates it once.
}
#[test]
fn preview_uses_every_immediate_source_and_keeps_primaries_independent() {
    let mut value = response();
    value["configuration"]["backup_routes"] = json!([["P01", "B01", "C01"], ["P02", "B02", "C02"]]);
    for (from, to, upstream, name) in [
        ("P01", "P02", None, "另主盘"),
        ("B01", "B02", Some("P02"), "另二级"),
        ("C01", "C02", Some("B02"), "另三级"),
    ] {
        let mut disk = value["capture"]["disks"][from].clone();
        disk["identity"]["slot_id"] = json!(to);
        disk["identity"]["disk_id"] = json!(format!("disk-{to}"));
        disk["upstream_slot_id"] = json!(upstream);
        disk["name"] = json!(name);
        value["capture"]["disks"][to] = disk;
    }
    let response = read_response(value, &draft()).unwrap();
    let prefixes = response.prefixes(3);
    assert_eq!(prefixes["B01"], "磁盘A");
    assert_eq!(prefixes["C01"], "磁盘B/磁盘A");
    assert_eq!(prefixes["B02"], "另主盘");
    assert_eq!(prefixes["C02"], "另二级/另主盘");
    assert!(response.prefixes(1).is_empty());
    assert_eq!(
        response.prefixes(2),
        [
            ("B01".into(), "磁盘A".into()),
            ("B02".into(), "另主盘".into())
        ]
        .into_iter()
        .collect()
    );
}
#[test]
fn recursive_capture_required_fields_unknown_fields_and_nullable_upstream_are_strict() {
    for pointer in [
        "/capture/format",
        "/capture/source_sequence",
        "/capture/source_business_generation",
        "/capture/registry",
        "/capture/disks",
        "/capture/registry/root_hash",
        "/capture/registry/view_generation",
        "/capture/disks/P01/identity",
        "/capture/disks/P01/name",
        "/capture/disks/P01/level",
        "/capture/disks/P01/upstream_slot_id",
        "/capture/disks/P01/identity/pool_id",
        "/capture/disks/P01/identity/disk_id",
        "/capture/disks/P01/identity/slot_id",
        "/capture/disks/P01/identity/binding_generation",
        "/configuration/file_count",
        "/configuration/generation",
        "/configuration/backup_routes",
        "/configuration/max_copies",
        "/configuration/backup_paths",
    ] {
        let mut value = response();
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        value
            .pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove(key);
        let error = read_response(value, &draft()).err().expect(pointer);
        assert!(matches!(
            error.source.as_deref(),
            Some(api::ErrorSource::BackupConfiguration(_))
        ));
    }
    for pointer in [
        "",
        "/capture",
        "/capture/registry",
        "/capture/disks/P01",
        "/capture/disks/P01/identity",
    ] {
        let mut value = response();
        value.pointer_mut(pointer).unwrap()["unknown"] = json!(true);
        assert!(read_response(value, &draft()).is_err(), "{pointer}");
    }
    let mut value = response();
    value["capture"]["disks"]["P01"]["upstream_slot_id"] = Value::Null;
    assert!(read_response(value, &draft()).is_ok());
}
#[test]
fn malformed_capture_never_becomes_a_writable_baseline() {
    for (pointer, bad) in [
        ("/capture/format", json!("other")),
        ("/capture/source_sequence", json!(42)),
        (
            "/capture/source_business_generation",
            json!("18446744073709551616"),
        ),
        ("/capture/registry/view_generation", json!(42)),
        ("/capture/disks/P01/identity/binding_generation", json!(42)),
        ("/capture/disks/P01/name", json!("../escape")),
        ("/capture/disks/P01/identity/slot_id", json!("different")),
        ("/capture/disks/P01/upstream_slot_id", json!("B01")),
        ("/capture/disks/B01/upstream_slot_id", json!("C01")),
        ("/capture/disks/B01/level", json!(3)),
        ("/capture/disks/B01/identity/pool_id", json!("different")),
        ("/configuration/backup_routes", json!([["P01", "absent"]])),
        ("/configuration/backup_routes", json!([[]])),
        ("/configuration/file_count", json!(1)),
        ("/configuration/scope/id", json!("different")),
        ("/configuration/scope/path", json!("another")),
    ] {
        let mut value = response();
        *value.pointer_mut(pointer).unwrap() = bad;
        assert!(read_response(value, &draft()).is_err(), "{pointer}");
    }
}
#[test]
fn pending_failed_and_over_capacity_cannot_save_but_empty_scope_can() {
    let mut draft = draft();
    assert!(request(&draft).is_err());
    draft.load = Load::Failed;
    assert!(request(&draft).is_err());
    for count in [0, 4] {
        draft = ready();
        let Load::Ready(r) = &mut draft.load else {
            unreachable!()
        };
        r.configuration.target_copies = count;
        assert!(request(&draft).is_err());
    }
    draft = ready();
    let Load::Ready(r) = &mut draft.load else {
        unreachable!()
    };
    r.configuration.file_count = 0;
    let body = request(&draft).unwrap();
    assert_eq!(body["path"], "docs");
    assert_eq!(body["expected_scope_id"], "scope");
    assert_eq!(body["capture"], response()["capture"]);
    let mut value = response();
    value["configuration"]["max_copies"] = json!(1);
    draft.load = Load::Ready(Box::new(read_response(value, &draft).unwrap()));
    assert!(request(&draft).is_err());
    let Load::Ready(r) = &mut draft.load else {
        unreachable!()
    };
    r.configuration.target_copies = 1;
    assert!(request(&draft).is_ok());
}
#[test]
fn closed_recursive_read_revokes_only_its_current_session() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 7,
            ..Default::default()
        });
        complete_load(state, 7, "closed", Err(unauthorized()));
        assert!(!state.with_untracked(State::session_active));
        assert_eq!(state.with_untracked(|s| s.errors.len()), 1);
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 8,
            ..Default::default()
        });
        complete_load(state, 7, "closed", Err(unauthorized()));
        assert!(state.with_untracked(State::session_active));
        assert!(state.with_untracked(|s| s.errors.is_empty()));
    });
}
#[test]
fn replaced_recursive_load_discards_old_data_and_old_decode_errors() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 7,
            ..Default::default()
        });
        state.update(|s| {
            let mut d = draft();
            d.id = "new".into();
            s.files.recursive_backup = Some(d);
        });
        complete_load(
            state,
            7,
            "old",
            Ok(read_response(response(), &draft()).unwrap()),
        );
        complete_load(state, 7, "old", Err(invalid("old bad response")));
        state.with_untracked(|s| {
            assert!(matches!(
                s.files.recursive_backup.as_ref().unwrap().load,
                Load::Pending
            ));
            assert!(s.errors.is_empty());
        });
        complete_load(
            state,
            7,
            "new",
            Ok(read_response(response(), &draft()).unwrap()),
        );
        assert!(state.with_untracked(|s| {
            s.files
                .recursive_backup
                .as_ref()
                .unwrap()
                .response()
                .is_some()
        }));
    });
}
#[test]
fn failed_recursive_load_stays_unwritable_and_keeps_error_source() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 7,
            ..Default::default()
        });
        state.update(|s| s.files.recursive_backup = Some(draft()));
        complete_load(state, 7, "draft", Err(invalid("bad capture")));
        state.with_untracked(|s| {
            assert!(matches!(
                s.files.recursive_backup.as_ref().unwrap().load,
                Load::Failed
            ));
            assert!(request(s.files.recursive_backup.as_ref().unwrap()).is_err());
            assert_eq!(s.errors.len(), 1);
        });
    });
}
#[test]
fn pending_publication_blocks_close_reload_new_modes_count_changes_and_double_submit() {
    Owner::new().with(|| {
        for unknown in [false, true] {
            let state = RwSignal::new(State {
                authenticated: true,
                session_epoch: 7,
                ..Default::default()
            });
            state.update(|s| {
                s.management.storage = crate::management::Storage::Ready;
                s.files.writable = true;
                s.files.recursive_backup = Some(ready());
                if unknown {
                    s.files.operations.push(FileOperation::unknown("original"));
                } else {
                    s.files.mutation_pending = true;
                }
            });
            close(state);
            reload(state);
            start(state, "other".into(), "".into(), None);
            super::super::start(state, "other".into(), "".into());
            select_copies(state, "1".into());
            submit(state);
            state.with_untracked(|s| {
                let d = s.files.recursive_backup.as_ref().unwrap();
                assert_eq!(d.id, "draft");
                assert_eq!(request(d).unwrap()["target_copies"], 3);
                assert!(s.files.backup.is_none());
                assert!(s.files.publications.is_empty());
            });
        }
    });
}
#[test]
fn recursive_submit_respects_session_storage_and_scope_write_guards() {
    Owner::new().with(|| {
        for (authenticated, storage, writable) in [
            (false, crate::management::Storage::Ready, true),
            (true, crate::management::Storage::ReadOnly, true),
            (true, crate::management::Storage::Unavailable, true),
            (true, crate::management::Storage::Ready, false),
        ] {
            let state = RwSignal::new(State {
                authenticated,
                ..Default::default()
            });
            state.update(|s| {
                s.management.storage = storage;
                s.files.writable = writable;
                s.files.recursive_backup = Some(ready());
            });
            submit(state);
            select_copies(state, "1".into());
            state.with_untracked(|s| {
                assert!(!s.files.mutation_pending);
                assert!(s.files.publications.is_empty());
                assert_eq!(
                    request(s.files.recursive_backup.as_ref().unwrap()).unwrap()["target_copies"],
                    3
                );
            });
        }
    });
}
#[test]
fn count_changes_keep_original_capture_and_ignore_non_option_values() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            ..Default::default()
        });
        state.update(|s| {
            s.management.storage = crate::management::Storage::Ready;
            s.files.writable = true;
            s.files.recursive_backup = Some(ready());
        });
        for invalid in ["0", "4", "not-a-count"] {
            select_copies(state, invalid.into());
        }
        assert_eq!(
            state.with_untracked(
                |s| request(s.files.recursive_backup.as_ref().unwrap()).unwrap()["target_copies"]
                    .clone()
            ),
            3
        );
        select_copies(state, "2".into());
        state.with_untracked(|s| {
            let body = request(s.files.recursive_backup.as_ref().unwrap()).unwrap();
            assert_eq!(body["target_copies"], 2);
            assert_eq!(body["capture"], response()["capture"]);
        });
    });
}
#[test]
fn navigation_clears_recursive_draft_without_settling_unknown_operation() {
    for navigation in [
        Navigation::Root,
        Navigation::Disk("B01".into()),
        Navigation::Directory("elsewhere".into()),
    ] {
        let mut model = Model {
            recursive_backup: Some(ready()),
            operations: vec![FileOperation::unknown("original")],
            ..Default::default()
        };
        model.navigate(navigation);
        assert!(model.recursive_backup.is_none());
        assert_eq!(model.operations[0].id(), "original");
        assert_eq!(model.operations[0].status(), SaveStatus::Unknown);
    }
}
#[test]
fn definitive_publication_closes_the_recursive_panel_but_unknown_and_rejected_preserve_it() {
    for status in [
        SaveStatus::Published,
        SaveStatus::Degraded,
        SaveStatus::Rejected,
        SaveStatus::Unknown,
    ] {
        let mut model = Model {
            recursive_backup: Some(ready()),
            ..Default::default()
        };
        model.publications.insert(
            review::Kind::Backup,
            review::Publication {
                operation_id: Some("original".into()),
                error: String::new(),
            },
        );
        review::settle(&mut model, "other", SaveStatus::Published);
        assert!(model.recursive_backup.is_some());
        review::settle(&mut model, "original", status);
        assert_eq!(
            model.recursive_backup.is_none(),
            matches!(status, SaveStatus::Published | SaveStatus::Degraded)
        );
    }
}
