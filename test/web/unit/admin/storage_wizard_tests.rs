use super::*;
use storage_contract::storage_directories::{DirectoryProblem, DirectorySource};

fn empty(name: &str) -> DiscoveredDisk {
    let path = format!("/mnt/{name}");
    let mut selection = super::super::tests::candidate(&path);
    selection.selection_id = format!("directory_{}", "a".repeat(64));
    DiscoveredDisk {
        name: name.into(),
        path,
        state: DiscoveredDiskState::Empty { selection },
    }
}
fn discovery(candidates: Vec<DiscoveredDisk>) -> DiscoveredDisks {
    DiscoveredDisks {
        discovery_root: "/mnt".into(),
        candidates,
        truncated: false,
        skipped_non_utf8: 0,
    }
}
fn model() -> super::super::Model {
    super::super::Model {
        snapshot: Some(Snapshot {
            state: StorageState::Pending,
            _disk_ids: vec![],
            completion_available: false,
            allow_development: false,
            terminal: None,
        }),
        ..Default::default()
    }
}
fn unsupported() -> DiscoveredDisk {
    let candidate = empty("ordinary");
    let DiscoveredDiskState::Empty { selection } = candidate.state else {
        unreachable!()
    };
    DiscoveredDisk {
        state: DiscoveredDiskState::Unsupported {
            code: storage_contract::ErrorCode::DirectoryNotEmpty,
            message: "directory has ordinary files".into(),
            source: Some(DirectorySource {
                path: candidate.path.clone(),
                discovery_identity: selection.discovery_identity,
                identity: selection.root_identity,
            }),
        },
        ..candidate
    }
}
fn failure() -> api::Error {
    api::Error {
        code: storage_contract::ErrorCode::DiskUnavailable,
        message: "discovery failed".into(),
        status: 503,
        retryable: true,
        request_id: Some("discovery-read".into()),
        source: None,
    }
}

pub(in super::super) fn open_discovered_source(
    model: &mut super::super::Model,
    source: DirectorySource,
) {
    let path = source.path.clone();
    let candidate = DiscoveredDisk {
        name: path.strip_prefix("/mnt/").unwrap().into(),
        path: path.clone(),
        state: DiscoveredDiskState::Unsupported {
            code: storage_contract::ErrorCode::FormatUnsupported,
            message: "Explicit inspection required".into(),
            source: Some(source),
        },
    };
    let discovery = read(json!(discovery(vec![candidate, unsupported()]))).unwrap();
    finish(model, Ok(discovery));
    choose(model, &path);
    assert!(model.wizard.selected(&path));
}

#[test]
fn discovered_wire_preserves_explicit_capabilities_and_bounded_inventory_flags() {
    let mut source = unsupported();
    let DiscoveredDiskState::Unsupported {
        source: observed, ..
    } = &source.state
    else {
        unreachable!()
    };
    source.state = DiscoveredDiskState::Zxs {
        disk_id: "disk_00000000000000000000000000000001".into(),
        slot_id: "B01".into(),
        role: storage_contract::storage_directories::DiscoveredDiskRole {
            level: 2,
            upstream_slot_id: Some("P01".into()),
            metadata_member: false,
            evidence: storage_contract::storage_directories::DiskRoleEvidence::InitialDeclaration,
        },
        source: observed.clone().unwrap(),
    };
    let mut value = discovery(vec![
        empty("new"),
        source,
        DiscoveredDisk {
            name: "blocked".into(),
            path: "/mnt/blocked".into(),
            state: DiscoveredDiskState::Unavailable {
                problem: DirectoryProblem::PermissionDenied,
            },
        },
    ]);
    value.truncated = true;
    value.skipped_non_utf8 = 1;
    assert_eq!(read(json!(value)).unwrap(), value);
    assert!(read(json!(discovery(vec![unsupported()]))).is_ok());
}

#[test]
fn malformed_mixed_or_fabricated_candidate_evidence_is_rejected() {
    let valid = json!(discovery(vec![empty("new")]));
    for key in [
        "discovery_root",
        "candidates",
        "truncated",
        "skipped_non_utf8",
    ] {
        let mut value = valid.clone();
        value.as_object_mut().unwrap().remove(key);
        assert!(read(value).is_err());
    }
    for (field, bad) in [
        ("root_path", json!("/mnt/other")),
        ("discovery_root", json!("/elsewhere")),
        ("selection_id", json!("directory_not-a-digest")),
    ] {
        let mut value = valid.clone();
        value["candidates"][0]["state"]["selection"][field] = bad;
        assert!(read(value).is_err());
    }
    for path in ["/mnt/new/child", "/outside/new", "/mnt/../new"] {
        let mut value = valid.clone();
        value["candidates"][0]["path"] = json!(path);
        assert!(read(value).is_err());
    }
    let mut value = valid.clone();
    value["candidates"][0]["state"]["kind"] = json!("zxs");
    assert!(
        read(value).is_err(),
        "an empty selection must not impersonate a recognized source"
    );
    assert!(read(json!(discovery(vec![empty("same"), empty("same")]))).is_err());
    let mut value = valid;
    value["discovery_root"] = json!("/mnt/../mnt");
    assert!(read(value).is_err());
}

#[test]
fn discovery_and_candidate_clicks_never_admit_or_change_selection_without_review() {
    let mut model = model();
    finish(&mut model, Ok(discovery(vec![empty("new"), unsupported()])));
    assert!(model.selections.is_empty());
    choose(&mut model, "/mnt/not-discovered");
    assert!(model.wizard.choice.is_none());
    choose(&mut model, "/mnt/ordinary");
    add(&mut model);
    assert!(
        model.selections.is_empty(),
        "read-only sources cannot become initialization proofs"
    );
    choose(&mut model, "/mnt/new");
    assert!(model.selections.is_empty());
    add(&mut model);
    assert_eq!(model.selections.len(), 1);
    assert!(model.wizard.choice.is_none());
    assert!(!model.can_activate());
    choose(&mut model, "/mnt/new");
    add(&mut model);
    assert_eq!(
        model.selections.len(),
        1,
        "repeated staging cannot duplicate admission"
    );
    model.confirmed = true;
    assert!(model.can_activate());
    remove(&mut model, &format!("directory_{}", "a".repeat(64)));
    assert!(model.selections.is_empty() && !model.confirmed);
}

#[test]
fn changed_candidates_invalidate_staged_identity_and_confirmation_but_same_proof_survives() {
    let mut model = model();
    let initial = discovery(vec![empty("new")]);
    finish(&mut model, Ok(initial.clone()));
    choose(&mut model, "/mnt/new");
    add(&mut model);
    finish(&mut model, Ok(initial.clone()));
    assert_eq!(model.selections.len(), 1);
    assert!(!model.wizard.changed);
    let mut changed = initial;
    let DiscoveredDiskState::Empty { selection } = &mut changed.candidates[0].state else {
        unreachable!()
    };
    selection.root_identity.inode += 1;
    finish(&mut model, Ok(changed));
    assert!(model.selections.is_empty() && model.wizard.changed);
    choose(&mut model, "/mnt/new");
    finish(&mut model, Ok(discovery(vec![])));
    assert!(model.wizard.choice.is_none());
}

#[test]
fn uncertain_admission_and_failed_discovery_keep_evidence_and_block_new_changes() {
    let mut model = model();
    finish(&mut model, Ok(discovery(vec![empty("new")])));
    choose(&mut model, "/mnt/new");
    add(&mut model);
    model.confirmed = true;
    model.unknown = true;
    assert!(!model.can_activate() && model.reload_blocked());
    remove(&mut model, &format!("directory_{}", "a".repeat(64)));
    assert_eq!(model.selections.len(), 1);
    model.unknown = false;
    finish(&mut model, Err(failure()));
    assert!(model.wizard.discovery.is_some());
    assert_eq!(
        model.wizard.error.as_ref().unwrap().request_id.as_deref(),
        Some("discovery-read")
    );
    assert!(!model.editable() && !model.can_activate());
    remove(&mut model, &format!("directory_{}", "a".repeat(64)));
    assert_eq!(model.selections.len(), 1);
}

#[test]
fn wizard_cannot_replace_server_proven_interruption_or_terminal_history() {
    let mut model = model();
    model.snapshot.as_mut().unwrap().completion_available = true;
    assert!(!discoverable(&model) && !model.editable());
    assert!(model.can_complete());
    model.snapshot.as_mut().unwrap().completion_available = false;
    model.snapshot.as_mut().unwrap().state = StorageState::TerminalManagement;
    assert!(discoverable(&model));
    model
        .selections
        .push(super::super::tests::candidate("/mnt/new"));
    model.confirmed = true;
    assert!(
        !model.can_activate(),
        "terminal history never becomes a first activation"
    );
}

#[test]
fn zxs_role_evidence_remains_read_only_and_invalid_roles_are_rejected() {
    use storage_contract::storage_directories::{DiscoveredDiskRole, DiskRoleEvidence};
    let mut candidate = unsupported();
    let DiscoveredDiskState::Unsupported { source, .. } = candidate.state else {
        unreachable!()
    };
    candidate.state = DiscoveredDiskState::Zxs {
        disk_id: "disk_00000000000000000000000000000001".into(),
        slot_id: "B01".into(),
        source: source.unwrap(),
        role: DiscoveredDiskRole {
            level: 2,
            upstream_slot_id: Some("P01".into()),
            metadata_member: true,
            evidence: DiskRoleEvidence::GenerationEvidence,
        },
    };
    let valid = discovery(vec![candidate.clone()]);
    assert!(read(json!(valid.clone())).is_ok());
    for change in 0..4 {
        let mut changed = valid.clone();
        let DiscoveredDiskState::Zxs { role, .. } = &mut changed.candidates[0].state else {
            unreachable!()
        };
        match change {
            0 => role.level = 0,
            1 => role.level = 1,
            2 => role.upstream_slot_id = None,
            3 => role.upstream_slot_id = Some("B01".into()),
            _ => unreachable!(),
        }
        assert!(read(json!(changed)).is_err());
    }
    let mut model = model();
    finish(&mut model, Ok(valid));
    choose(&mut model, &candidate.path);
    add(&mut model);
    assert!(
        model.selections.is_empty(),
        "recognized ZXS role is not empty-target permission"
    );
    assert!(!model.can_activate());
}
