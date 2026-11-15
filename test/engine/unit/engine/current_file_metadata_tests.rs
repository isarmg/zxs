use super::verify;
use crate::*;
use json_codec::json;
use std::{fs, sync::Arc};

fn op() -> OperationId {
    OperationId::generate().unwrap()
}
fn note(value: &str) -> storage_contract::Note {
    storage_contract::Note::parse(value.into()).unwrap()
}
fn name(value: &str) -> storage_contract::BranchName {
    storage_contract::BranchName::parse(value).unwrap()
}
fn save(engine: &mut Engine, branch: &str, value: &[u8]) -> Entry {
    let workspace = engine.create_workspace(branch, "admin").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "dir/file.txt", value).unwrap();
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: op(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "metadata plan fixture".into(),
        request_digest: None,
    }).unwrap();
    engine.namespace_tree(branch).unwrap().get("dir/file.txt").unwrap().clone()
}

struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    entry: Entry,
    old: Entry,
    branch: String,
    excluded: std::path::PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut config = Config::development(
            (0..4).map(|index| temp.path().join(format!("disk-{index}"))),
        ).unwrap();
        config.disks[3].level = 1;
        config.disks[3].upstream_slot_id = None;
        config.disks[3].metadata_member = false;
        let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
        let repository = engine.create_repository(
            &storage_contract::RepositoryName::parse("metadata-plan").unwrap(),
            "D04", 1, "admin", op(),
        ).unwrap();
        let rule = new_id("rule").unwrap();
        engine.update_placement_rule(PlacementRuleUpdate {
            rule_id: &rule,
            repository_id: &repository.repository_id,
            path_prefix: "dir",
            primary_slot_id: "P01",
            expected_generation: 0,
            principal: "admin",
            operation_id: op(),
        }).unwrap();
        let old = save(&mut engine, &repository.main_branch_id, b"old");
        engine.update_placement_rule(PlacementRuleUpdate {
            rule_id: &rule,
            repository_id: &repository.repository_id,
            path_prefix: "dir",
            primary_slot_id: "D04",
            expected_generation: 1,
            principal: "admin",
            operation_id: op(),
        }).unwrap();
        let entry = save(&mut engine, &repository.main_branch_id, b"retained");
        assert_eq!(entry.file_id, old.file_id);
        let branch = engine.create_file_branch(
            &entry.file_id, &entry.version_id, &name("source"), "admin", op(),
        ).unwrap().branch_id.unwrap();
        engine.begin_disk_exit(
            &config.disks[0].disk_id, MembershipExitAction::PlannedOffline, "admin", op(),
        ).unwrap();
        let archive = engine.build_managed_membership_control_archive().unwrap();
        engine.enroll_pending_members(
            &archive.archive_path, &archive.root_hash, Vec::new(),
        ).unwrap();
        engine.resume_membership_joint_decision().unwrap();
        engine.activate_pending_disk_exit().unwrap();
        let excluded = config.disks[0].root.clone();
        fs::rename(&excluded, temp.path().join("parked-original")).unwrap();
        fs::write(&excluded, b"excluded root must not open").unwrap();
        Self { temp, engine, entry, old, branch, excluded }
    }

    fn edit(&mut self, route: usize, operation: OperationId) -> OperationResult {
        match route {
            0 => self.engine.set_file_note(
                &self.entry.file_id, None, &note("file note"), "admin", operation,
            ).unwrap(),
            1 => self.engine.set_file_note(
                &self.old.file_id, Some(&self.old.version_id),
                &note("version note"), "admin", operation,
            ).unwrap(),
            2 => self.engine.rename_file_branch(
                &self.entry.file_id, &self.branch, &name("renamed"), "admin", operation,
            ).unwrap(),
            _ => panic!("fixture route"),
        }
    }
}

fn published_candidate(engine: &Engine, sequence: u64) -> crate::journal::Candidate {
    let online = engine.config.metadata_members().cloned().collect::<Vec<_>>();
    let disk = &online[0];
    let root = &engine.disk_resources.get(disk).unwrap().root;
    let scan = crate::journal::scan_configured_at(root, disk, &engine.config).unwrap();
    let (record, _) = scan.records.into_iter().find(|(record, _)| {
        record.record_type == crate::journal::RecordType::Publication && record.sequence == sequence
    }).unwrap();
    crate::publication_evidence::verified_object_at(
        &engine.disk_resources, &online, "candidate", &record.candidate_hash,
    ).unwrap()
}

#[test]
fn metadata_three_routes_reject_receipt_and_closed_write_set_forgery() {
    let mut fixture = Fixture::new();
    for route in 0..3 {
        let previous = Arc::clone(&fixture.engine.state);
        let view = fixture.engine.current_namespaces.as_ref().unwrap().clone();
        let result = fixture.edit(route, op());
        let next = fixture.engine.state.as_ref().clone();
        // A completed publication's captured names are an idempotent value
        // property, even though its immutable Arc can carry a skip receipt.
        let mut captured = previous.as_ref().clone();
        crate::container_storage::capture_names(&mut captured);
        assert_eq!(captured.file_names, previous.file_names);
        assert_eq!(captured.file_locations, previous.file_locations);
        // Capture fills holes; it never replaces a retained current name with
        // a historical main-HEAD spelling. This is not a pool-format migration.
        captured.file_names.insert(fixture.entry.file_id.clone(), "retained-name".into());
        captured.file_locations.insert(fixture.entry.file_id.clone(), "retained-path".into());
        crate::container_storage::capture_names(&mut captured);
        assert_eq!(captured.file_names[&fixture.entry.file_id], "retained-name");
        assert_eq!(captured.file_locations[&fixture.entry.file_id], "retained-path");
        verify(&previous, &next, Some(&view), Some(&result)).unwrap();
        view.advance(&previous, &next).unwrap();
        let mut candidate = published_candidate(&fixture.engine, result.sequence);
        assert!(candidate.versions.is_empty());
        let current_state = Arc::clone(&fixture.engine.state);
        let current_view = fixture.engine.current_namespaces.clone();
        fixture.engine.state = Arc::clone(&previous);
        fixture.engine.current_namespaces = Some(Arc::clone(&view));
        fixture.engine.verify_current_recovery_candidate(&candidate, &next).unwrap();
        candidate.versions.push(crate::journal::VersionEvidence {
            version: next.versions[&fixture.entry.version_id].clone(),
            placement: next.placements[&fixture.entry.version_id].clone(),
        });
        assert_eq!(
            fixture.engine.verify_current_recovery_candidate(&candidate, &next)
                .unwrap_err().code(),
            ErrorCode::PublicationInvalid,
        );
        fixture.engine.state = current_state;
        fixture.engine.current_namespaces = current_view;
        for attack in 0..13 {
            let mut forged = next.clone();
            match attack {
                0 => { forged.business_generation += 1; }
                1 => { forged.operation_principals.insert(result.operation_id, "other".into()); }
                2 => {
                    forged.operations.get_mut(&result.operation_id).unwrap().kind = "noop".into();
                }
                3 => {
                    forged.operations.get_mut(&result.operation_id).unwrap().request_digest.clear();
                }
                4 => { forged.operations.get_mut(&result.operation_id).unwrap().sequence += 1; }
                5 => {
                    forged.operations.get_mut(&result.operation_id).unwrap()
                        .result["extra"] = json!(1);
                }
                6 => { forged.events.last_mut().unwrap().object_id = "wrong".into(); }
                7 => {
                    forged.file_notes.insert(fixture.entry.file_id.clone(), "unrequested".into());
                }
                8 => {
                    forged.version_notes.insert(
                        fixture.old.version_id.clone(), "unrequested".into(),
                    );
                }
                9 => {
                    forged.branches.get_mut(&fixture.branch).unwrap().name = "unrequested".into();
                }
                10 => {
                    forged.versions.get_mut(&fixture.entry.version_id).unwrap().logical_size += 1;
                }
                11 => { forged.operations.remove(&result.operation_id); }
                12 => {
                    forged.operations.get_mut(&result.operation_id).unwrap()
                        .result["version_protection"] = json!([{}]);
                }
                _ => panic!("fixture attack"),
            }
            for state in [
                forged.clone(),
                json_codec::from_slice::<State>(&json_codec::to_vec(&forged).unwrap()).unwrap(),
            ] {
                assert!(view.advance(&previous, &state).is_err(), "route {route}, attack {attack}");
            }
        }
        assert_eq!(fs::read(&fixture.excluded).unwrap(), b"excluded root must not open");
    }
}

#[test]
fn metadata_empty_same_value_and_current_receipt_bytes_survive_cold_open() {
    let mut fixture = Fixture::new();
    let mut receipts = Vec::new();
    for route in 0..3 {
        let result = fixture.edit(route, op());
        let request = match route {
            0 => json!({"file_id":fixture.entry.file_id,"version_id":null,"note":"file note"}),
            1 => json!({"file_id":fixture.old.file_id,
                "version_id":fixture.old.version_id,"note":"version note"}),
            2 => json!({
                "file_id":fixture.entry.file_id,"branch_id":fixture.branch,"name":"renamed",
            }),
            _ => panic!("fixture route"),
        };
        assert_eq!(result.request_digest,
            Engine::native_request_digest(&result.kind, "admin", &request).unwrap());
        let mut expected = request;
        if route == 2 {
            expected["history_tree_id"] = json!(result.history_tree_id);
            expected["previous_name"] = json!("source");
            expected["no_change"] = json!(false);
            expected["branch_generation"] = result.result["branch_generation"].clone();
        }
        expected["timestamp"] = result.result["timestamp"].clone();
        expected["membership_epoch"] = result.result["membership_epoch"].clone();
        expected["version_protection"] = json!([]);
        assert_eq!(
            json_codec::to_vec(&result.result).unwrap(), json_codec::to_vec(&expected).unwrap(),
        );
        if route != 2 {
            // Real permanent receipts enter the same raw mirror as cold replay.
            // Empty and the full UTF-8 byte bound remain valid without changing
            // their text. A malformed note preserves the semantic parser cause.
            for text in [String::new(), "é".repeat(64)] {
                let mut encoded = result.clone();
                encoded.result["note"] = json!(text.as_str());
                let decoded = crate::persist::file_metadata::decode(&encoded).unwrap();
                match decoded.request() {
                    crate::business::file_metadata::Request::Note { note, .. } => {
                        assert_eq!(note.as_str(), text);
                    }
                    crate::business::file_metadata::Request::Rename { .. } => {
                        panic!("note receipt decoded as a rename")
                    }
                }
            }
            for text in ["x".repeat(129), "line\nbreak".into()] {
                let mut encoded = result.clone();
                encoded.result["note"] = json!(text.as_str());
                let error = crate::persist::file_metadata::decode(&encoded).err().unwrap();
                assert_eq!(error.code(), ErrorCode::PublicationInvalid);
                match error {
                    Error::FileMetadata(FileMetadataError::Note(source)) => {
                        assert_eq!(source.code(), ErrorCode::InvalidRequest);
                    }
                    error => panic!("note parser cause was lost: {error}"),
                }
            }
        }
        let sequence = fixture.engine.state.sequence;
        assert_eq!(fixture.edit(route, result.operation_id), result);
        assert_eq!(fixture.engine.state.sequence, sequence);
        let repeated = fixture.edit(route, op());
        if route == 2 { assert_eq!(repeated.result["no_change"], json!(true)); }
        receipts.push(result);
    }
    for version in [None, Some(fixture.old.version_id.as_str())] {
        fixture.engine.set_file_note(
            &fixture.entry.file_id, version, &note(""), "admin", op(),
        ).unwrap();
    }
    assert!(!fixture.engine.state.file_notes.contains_key(&fixture.entry.file_id));
    assert!(!fixture.engine.state.version_notes.contains_key(&fixture.old.version_id));
    let state = fixture.engine.state.as_ref().clone();
    drop(fixture.engine);
    let reopened = Engine::open(&fixture.temp.path().join("system")).unwrap();
    assert_eq!(reopened.state.sequence, state.sequence);
    for result in receipts {
        assert_eq!(reopened.state.operations[&result.operation_id], result);
    }
    assert_eq!(reopened.state.file_notes, state.file_notes);
    assert_eq!(reopened.state.version_notes, state.version_notes);
    assert_eq!(reopened.state.branches[&fixture.branch].name, "renamed");
    assert_eq!(fs::read(&fixture.excluded).unwrap(), b"excluded root must not open");
}

#[test]
fn metadata_replay_revalidates_version_and_branch_source() {
    let mut fixture = Fixture::new();
    let previous = Arc::clone(&fixture.engine.state);
    let view = fixture.engine.current_namespaces.as_ref().unwrap().clone();
    let result = fixture.edit(1, op());
    let mut next = fixture.engine.state.as_ref().clone();
    let wrong = storage_contract::VersionId::generate().unwrap().to_string();
    let receipt = next.operations.get_mut(&result.operation_id).unwrap();
    receipt.result["version_id"] = json!(wrong);
    receipt.request_digest = Engine::native_request_digest(
        "version.metadata", "admin",
        &json!({"file_id":fixture.entry.file_id,"version_id":wrong,"note":"version note"}),
    ).unwrap();
    assert!(view.advance(&previous, &next).is_err());
    let previous = Arc::clone(&fixture.engine.state);
    let view = fixture.engine.current_namespaces.as_ref().unwrap().clone();
    let result = fixture.edit(2, op());
    let next = fixture.engine.state.as_ref().clone();
    let mut wrong_generation = next.clone();
    wrong_generation.operations.get_mut(&result.operation_id).unwrap()
        .result["branch_generation"] = json!("999");
    assert!(view.advance(&previous, &wrong_generation).is_err());
    let mut wrong_source = previous.as_ref().clone();
    wrong_source.branches.get_mut(&fixture.branch).unwrap().head_commit_id =
        new_id("commit").unwrap();
    assert!(verify(&wrong_source, &next, Some(&view), None).is_err());
}
