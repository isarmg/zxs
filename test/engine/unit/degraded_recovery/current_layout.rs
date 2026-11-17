//! One current-format fixture for the existing two-ACCEPT loss regression.
//! Other legacy fixtures are deliberately not silently rewritten by this helper.
use crate::container_storage::PreparedContainerEntities;
use crate::disk::{VersionMaterial, MAX_METADATA};
use crate::journal::{Candidate, VersionEvidence, log_path};
use crate::persist::layout;
use crate::{
    AutomaticBackupConfigurationRequest, CommitRequest, Config, DiskHistoryMaintenanceState,
    DiskRegistrationIdentity, Engine, Error, ErrorCode, RecoveryPlan,
};
use name_policy::Name;
use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use storage_contract::{CommitState, Generation, OperationId};

const LOST: &[u8] = b"permanently lost bytes";
const HEALTHY: &[u8] = b"retained full bytes";

fn expected(evidence: &VersionEvidence) -> &'static [u8] {
    match evidence.version.creation_name.as_str() {
        "lost.txt" => LOST,
        "healthy.txt" => HEALTHY,
        name => panic!("unexpected real fixture file: {name}"),
    }
}

pub(super) fn mixed_pending() -> (tempfile::TempDir, Engine, OperationId, Candidate) {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        (Name::parse("Primary").unwrap(), temp.path().join("p")),
        (Name::parse("Near archive").unwrap(), temp.path().join("b")),
        (Name::parse("Far archive").unwrap(), temp.path().join("c")),
    ])
    .unwrap();
    assert_eq!(config.disks.iter().filter(|disk| disk.metadata_member).count(), 3);
    let mut engine = Engine::initialize(&temp.path().join("state"), config).unwrap();
    let repository = engine.create_disk_root(
        "P01",
        "alice",
        OperationId::generate().unwrap(),
    ).unwrap();
    let actor = "alice".parse().unwrap();
    let configured = engine.configure_automatic_backups(AutomaticBackupConfigurationRequest {
        repository_id: repository.repository_id.parse().unwrap(),
        path: None,
        target_copies: 3,
        expected_generation: Generation::new(0),
        principal: &actor,
        operation_id: OperationId::generate().unwrap(),
    }).unwrap();
    assert_eq!(configured.commit_state, CommitState::Published);

    let workspace = engine.create_workspace(&repository.main_branch_id, "alice").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "lost.txt", LOST).unwrap();
    engine.stage_bytes(&workspace.workspace_id, "healthy.txt", HEALTHY).unwrap();
    let operation = OperationId::generate().unwrap();
    engine.inject_fault_once("after_accept_2");
    let error = engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: operation,
        principal: "alice".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "actual mixed copies".into(),
        request_digest: None,
    }).unwrap_err();
    assert!(matches!(error, Error::Context {
        code: ErrorCode::InDoubt, source, ..
    } if source.code() == ErrorCode::InjectedFailure));
    let (candidate, original, _, accepts) = engine.pending_recovery_candidate(
        operation, None,
    ).unwrap();
    assert_eq!(accepts.len(), 2);
    assert_eq!(candidate.operation_id, operation);
    assert_eq!(candidate.versions.len(), 2);
    let sources = PreparedContainerEntities::observe_candidate(
        &engine.planning_context(), &candidate, &original,
    ).unwrap();
    let mut lost_paths = Vec::new();
    let mut body_identities = BTreeSet::new();
    for evidence in &candidate.versions {
        assert_eq!(evidence.placement.target_copies, 3);
        assert_eq!(evidence.placement.route_slot_ids.len(), 3);
        for slot in &evidence.placement.route_slot_ids {
            let disk = engine.disk(slot).unwrap();
            let scope = sources.scope(
                evidence.version.file_id.parse().unwrap(), disk.disk_id.parse().unwrap(),
            ).unwrap();
            let material = VersionMaterial::current(scope, &evidence.version).unwrap();
            // A route is not proof of a copy. Verify each actual target FD and
            // read all bytes before making this deliberate fault.
            let mut verified = material.verify_payload().unwrap();
            let metadata = verified.metadata().unwrap();
            let path = material.body_path().unwrap();
            let named = fs::metadata(&path).unwrap();
            assert_eq!((metadata.dev(), metadata.ino()), (named.dev(), named.ino()));
            assert!(body_identities.insert((metadata.dev(), metadata.ino())));
            let mut bytes = Vec::new();
            verified.seek(SeekFrom::Start(0)).unwrap();
            verified.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, expected(evidence));
            if evidence.version.creation_name == "lost.txt" {
                lost_paths.push(path);
            }
        }
    }
    assert_eq!(body_identities.len(), 6);
    assert_eq!(lost_paths.len(), 3);
    let lost = candidate.versions.iter().find(|evidence| {
        evidence.version.creation_name == "lost.txt"
    }).unwrap();
    let staged = engine.workspaces[&workspace.workspace_id].files.values().filter(|source| {
        source.content_hash == lost.version.content_hash
    }).collect::<Vec<_>>();
    assert_eq!(staged.len(), 1);
    fs::remove_file(&staged[0].source_path).unwrap();
    fs::remove_file(&lost_paths[0]).unwrap();
    drop(sources);
    drop(original);

    let logs = engine.members().iter().map(|disk| {
        let path = log_path(disk);
        let bytes = fs::read(&path).unwrap();
        (path, bytes)
    }).collect::<Vec<_>>();
    assert_eq!(engine.prepare_degraded_recovery(
        operation, "alice", "one replica is missing but two complete sources remain",
    ).unwrap_err().code(), ErrorCode::RecoveryRequired);
    for (path, bytes) in logs {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    // Only losing all copies of this one version makes its loss permanent.
    // The other file retains three independently verified bodies.
    for path in &lost_paths[1..] {
        fs::remove_file(path).unwrap();
    }
    (temp, engine, operation, candidate)
}

pub(super) fn published_proof(
    engine: &Engine, evidence: &VersionEvidence, slot: &str,
) -> Vec<u8> {
    let disk = engine.disk(slot).unwrap();
    let scope = crate::engine::fresh_layout::containers::observe_replica(
        &engine.config, &engine.disk_resources, engine.state(),
        evidence.version.file_id.parse().unwrap(), disk,
    ).unwrap();
    let material = VersionMaterial::current(&scope, &evidence.version).unwrap();
    let path = material.directory().unwrap().join(layout::VERSION_PUBLICATION_PROOF.as_path());
    material.root().unwrap().read_bytes_observed(&path, MAX_METADATA, &mut |_| Ok(())).unwrap()
}

pub(super) fn assert_published_materials(engine: &Engine, candidate: &Candidate) {
    let mut healthy = 0;
    let mut missing = 0;
    for evidence in &candidate.versions {
        for slot in &evidence.placement.route_slot_ids {
            let disk = engine.disk(slot).unwrap();
            let scope = crate::engine::fresh_layout::containers::observe_replica(
                &engine.config, &engine.disk_resources, engine.state(),
                evidence.version.file_id.parse().unwrap(), disk,
            ).unwrap();
            let material = VersionMaterial::current(&scope, &evidence.version).unwrap();
            if evidence.version.creation_name == "lost.txt" {
                let error = fs::metadata(material.body_path().unwrap()).unwrap_err();
                assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
                assert!(material.verify_payload().is_err());
                missing += 1;
            } else {
                let mut verified = material.verify_payload().unwrap();
                let metadata = verified.metadata().unwrap();
                let named = fs::metadata(material.body_path().unwrap()).unwrap();
                assert_eq!((metadata.dev(), metadata.ino()), (named.dev(), named.ino()));
                let mut bytes = Vec::new();
                verified.seek(SeekFrom::Start(0)).unwrap();
                verified.read_to_end(&mut bytes).unwrap();
                assert_eq!(bytes, HEALTHY);
                healthy += 1;
            }
        }
    }
    assert_eq!((missing, healthy), (3, 3));
}

pub(super) fn assert_loss_plan(candidate: &Candidate, plan: &RecoveryPlan) {
    assert_eq!(plan.version_protection.len(), 2);
    assert_eq!(plan.loss_details.len(), 3);
    for evidence in &candidate.versions {
        let lost = evidence.version.creation_name == "lost.txt";
        let health = plan.version_protection.iter().find(|health| {
            health["version_id"] == evidence.version.version_id
        }).unwrap();
        let slots = health["slots"].as_array().unwrap();
        assert_eq!(slots.len(), 3);
        for slot in slots {
            let expected = if lost { "UNAVAILABLE" } else { "COMPLETE_VERIFIED" };
            assert_eq!(slot["state"], expected);
        }
        let details = plan.loss_details.iter().filter(|detail| {
            detail["version_id"] == evidence.version.version_id
        }).collect::<Vec<_>>();
        assert_eq!(details.len(), if lost { 3 } else { 0 });
        assert!(details.iter().all(|detail| detail["cause"] == "MISSING"));
    }
}

pub(super) fn assert_recovered_snapshot_once(
    engine: &Engine, candidate: &Candidate, decoded: usize,
) {
    assert!(engine.config.generation().is_none());
    assert!(!engine.is_read_only());
    assert_eq!(engine.state().sequence, candidate.sequence);
    assert_eq!(engine.snapshot.as_ref(), Some(&candidate.snapshot_hash));
    let histories = engine.disk_history_statuses();
    assert_eq!(histories.len(), 3);
    assert_eq!(histories.len(), engine.config.disks.len());
    let mut history_decodes = 0;
    for disk in &engine.config.disks {
        let identity = DiskRegistrationIdentity {
            pool_id: engine.config.pool_id.clone(),
            disk_id: disk.disk_id.clone(),
            slot_id: disk.slot_id.clone(),
            binding_generation: disk.binding_generation,
        };
        let matching = histories.iter().filter(|status| status.identity == identity)
            .collect::<Vec<_>>();
        assert_eq!(matching.len(), 1);
        let status = matching[0];
        assert_eq!(status.observed_sequence, candidate.sequence);
        let DiskHistoryMaintenanceState::Tracked {
            persisted_through_sequence,
            observed_through_sequence,
            observed_publications,
            ..
        } = &status.state else {
            panic!("normal open did not finish the exact disk history: {status:?}");
        };
        assert_eq!(*persisted_through_sequence, candidate.sequence);
        assert_eq!(*observed_through_sequence, candidate.sequence);
        assert!(*observed_publications > 0);
        // Genesis visit_sources decodes every original snapshot exactly once;
        // Prefix::finish binds its last hash to engine.snapshot. Only a fully
        // installed, Tracked prefix permits accounting for this candidate once.
        // Failed, excluded, partial or unrelated histories cannot be deducted.
        history_decodes += 1;
    }
    assert!(decoded >= history_decodes);
    assert_eq!(
        decoded - history_decodes,
        1,
        "candidate {} was decoded {decoded} times, including {history_decodes} complete \
         disk histories; total startup decoded {} snapshots",
        candidate.snapshot_hash,
        crate::journal::SNAPSHOT_LOAD_COUNT.with(|count| count.get()),
    );
}
