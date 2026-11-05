use super::*;
use storage_contract::storage_directories::{
    DirectoryProblem, DiscoveredDiskRole, DiskRoleEvidence,
};

fn id(prefix: &str, number: u8) -> String {
    format!("{prefix}_{number:032x}")
}
fn source() -> DirectorySource {
    let candidate = super::super::tests::candidate("/mnt/foreign");
    DirectorySource {
        path: candidate.root_path,
        discovery_identity: candidate.discovery_identity,
        identity: candidate.root_identity,
    }
}
fn identity() -> ForeignDiskIdentity {
    ForeignDiskIdentity {
        pool_id: id("pool", 1),
        disk_id: id("disk", 1),
        slot_id: "P01".into(),
        binding_generation: 1,
    }
}
fn preview() -> ForeignDiskPreview {
    ForeignDiskPreview {
        source: source(),
        identity: identity(),
        source_level: 2,
        role_evidence: DiskRoleEvidence::InitialDeclaration,
        seal_digest: "a".repeat(64),
        review_digest: "b".repeat(64),
        checkpoint: ForeignDiskCheckpoint {
            candidate_hash: "c".repeat(64),
            publication_hash: "d".repeat(64),
            sequence: 9007199254740993,
            authority_hash: None,
            unpublished_tail: true,
        },
        histories: vec![ForeignHistorySummary {
            group_id: "e".repeat(64),
            history_tree_id: id("history", 1),
            repository_name: "Original repository".into(),
            origin: identity(),
            manifest_hash: "f".repeat(64),
            branches: 1,
            directory_branches: 1,
            directory_trees: 2,
            commit_records: 3,
            versions: 4,
            verified_local_versions: 1,
            temporarily_unavailable_versions: vec![id("ver", 1)],
            recorded_purged_versions: 1,
            recorded_migrated_versions: 1,
        }],
        execution: ForeignExecutionCapability::Unavailable,
    }
}
#[test]
fn explicit_foreign_review_preserves_source_history_and_unavailable_contents() {
    let expected = preview();
    let actual = read(json!(expected), &source()).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual.checkpoint.sequence, 9007199254740993);
    assert_eq!(
        actual.histories[0].temporarily_unavailable_versions,
        vec![id("ver", 1)]
    );
    assert_eq!(actual.histories[0].recorded_purged_versions, 1);
}
#[test]
fn foreign_preview_binds_exact_observation_and_has_no_execution_capability() {
    for change in 0..9 {
        let mut value = json!(preview());
        match change {
            0 => value["source"]["path"] = "/mnt/other".into(),
            1 => value["source"]["identity"]["inode"] = "9".into(),
            2 => value["identity"]["disk_id"] = "bad".into(),
            3 => value["source_level"] = 0.into(),
            4 => value["execution"] = "ready".into(),
            5 => value["review_digest"] = "bad".into(),
            6 => value["checkpoint"]["sequence"] = 1.into(),
            7 => value["checkpoint"]
                .as_object_mut()
                .unwrap()
                .remove("authority_hash")
                .map(|_| ())
                .unwrap(),
            8 => value["extra"] = true.into(),
            _ => unreachable!(),
        }
        assert!(read(value, &source()).is_err(), "change {change}");
    }
}
#[test]
fn foreign_history_summary_counts_are_complete_bounded_and_unambiguous() {
    for change in 0..7 {
        let mut value = preview();
        match change {
            0 => value.histories[0].versions += 1,
            1 => value.histories[0]
                .temporarily_unavailable_versions
                .push(id("ver", 1)),
            2 => value.histories[0].temporarily_unavailable_versions[0] = "bad".into(),
            3 => value.histories.push(value.histories[0].clone()),
            4 => value.histories[0].verified_local_versions = usize::MAX,
            5 => value.histories[0].origin.binding_generation = 0,
            6 => value.histories[0].history_tree_id = "bad".into(),
            _ => unreachable!(),
        }
        assert!(read(json!(value), &source()).is_err(), "change {change}");
    }
}
#[test]
fn only_discovered_source_capabilities_enable_explicit_inspection() {
    assert!(
        candidate_source(&DiscoveredDiskState::Empty {
            selection: super::super::tests::candidate("/mnt/empty")
        })
        .is_none()
    );
    assert!(
        candidate_source(&DiscoveredDiskState::Unavailable {
            problem: DirectoryProblem::SymbolicLink
        })
        .is_none()
    );
    let ordinary = DiscoveredDiskState::Unsupported {
        code: storage_contract::ErrorCode::FormatUnsupported,
        message: "inspect explicitly".into(),
        source: Some(source()),
    };
    assert_eq!(candidate_source(&ordinary), Some(&source()));
    let zxs = DiscoveredDiskState::Zxs {
        disk_id: id("disk", 1),
        slot_id: "P01".into(),
        role: DiscoveredDiskRole {
            level: 1,
            upstream_slot_id: None,
            metadata_member: true,
            evidence: DiskRoleEvidence::InitialDeclaration,
        },
        source: source(),
    };
    assert_eq!(candidate_source(&zxs), Some(&source()));
}
#[test]
fn clearing_read_only_review_invalidates_old_response_generation() {
    let mut model = Model {
        preview: Some(preview()),
        reading: true,
        generation: 7,
        ..Default::default()
    };
    model.invalidate();
    assert_eq!(model.generation, 8);
    assert!(!model.reading);
    assert!(model.preview.is_none());
    let ui = include_str!("../../../../src/web/src/admin/storage/foreign.rs");
    assert!(!ui.contains("/foreign/commit"));
    assert!(ui.contains("data-foreign-preview-start"));
    assert!(!ui.contains("operation_id"));
}
