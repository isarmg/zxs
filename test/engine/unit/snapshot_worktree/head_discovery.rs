//! Discovery uses genuine completed or pending registrations. It never feeds
//! a decoded description back as a publication, selected branch or root source.
use super::*;
use crate::persist::snapshot_worktree::NodeAllocation;
use crate::snapshot_worktree::{prepare_namespace_host, publication};
use crate::snapshot_worktree::drafts::DirectoryDraftIntent;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use storage_contract::snapshot_worktree::{
    InitializeSnapshot, NamespaceHostAddress, SnapshotHandle,
};
use storage_contract::snapshot_worktree::discovery::SnapshotBranchSelection;
use storage_contract::snapshot_worktree::draft_commit::{
    DirectoryDraftLookup, DirectoryDraftOutcome,
};
use storage_contract::working_tree::{SnapshotBranchId, SnapshotNodeId, SnapshotRepositoryId};
use storage_contract::{OperationId, RepositoryId};

#[path = "draft_gateway/fixture.rs"]
mod fixture;
use fixture::{Fixture, inventory, selection, write_draft};

fn request(engine: &Engine, repository: RepositoryId) -> SnapshotHeadRequest {
    let context = engine.snapshot_root_context(repository).unwrap();
    SnapshotHeadRequest {
        namespace: NamespaceHostAddress {
            repository_id: repository,
            branch_id: context.source.branch_id,
            directory: NamespaceDirectoryId::Repository(repository),
        },
        host: context.host,
        branch: SnapshotBranchSelection::Initial,
    }
}

fn observe(
    fixture: &Fixture, engine: &Engine, request: SnapshotHeadRequest,
) -> Result<SnapshotHeadDiscovery> {
    let state = Arc::clone(&engine.state);
    let slot = engine.next_slot;
    let snapshot = engine.snapshot.clone();
    let media = fixture.media();
    let result = engine.discover_directory_snapshot(request);
    assert!(Arc::ptr_eq(&state, &engine.state));
    assert_eq!(engine.next_slot, slot);
    assert_eq!(engine.snapshot, snapshot);
    assert_eq!(fixture.media(), media);
    result
}

fn head(value: &SnapshotHeadDiscovery) -> SnapshotHandle {
    let SnapshotHeadDiscovery::Head { snapshot, .. } = value else {
        panic!("the completed initial/designated branch must have its registered head");
    };
    *snapshot
}

#[test]
fn actual_snapshot_head_discovery_follows_publication_and_cold_root_binding() {
    let (fixture, mut engine) = Fixture::new();
    let principal = "admin".parse().unwrap();
    let initial = request(&engine, fixture.targets[0].repository);
    let first = observe(&fixture, &engine, initial).unwrap();
    assert_eq!(head(&first), fixture.source);
    let exact = SnapshotHeadRequest {
        branch: SnapshotBranchSelection::Exact {
            repository: fixture.source.repository_id, branch: fixture.source.branch_id,
        },
        ..initial
    };
    assert_eq!(observe(&fixture, &engine, exact).unwrap(), first);

    write_draft(&mut engine, &fixture.targets[0], fixture::FIRST_DRAFT);
    let selected = [selection(&engine, &fixture.targets[0])];
    let intent = fixture.intent(&selected, &principal);
    let operation = intent.operation;
    engine.commit_directory_working_set(intent).unwrap();
    let result = engine.directory_draft_receipt(DirectoryDraftLookup {
        source: fixture.source, operation_id: operation,
    }, &principal).unwrap();
    let DirectoryDraftOutcome::Changed { snapshot: changed } = result.result else {
        panic!("actual selected changed draft did not publish a node");
    };
    let changed_head = observe(&fixture, &engine, initial).unwrap();
    assert_eq!(head(&changed_head), changed);
    assert_ne!(changed.node_id, fixture.source.node_id);
    assert_eq!(observe(&fixture, &engine, exact).unwrap(), changed_head);

    // A newer truthful Unchanged receipt cannot win a 'latest operation' scan.
    // Its actual branch head remains the earlier changed completion identity.
    write_draft(&mut engine, &fixture.targets[0], fixture::FIRST_DRAFT);
    let selected = [selection(&engine, &fixture.targets[0])];
    let operation = OperationId::generate().unwrap();
    engine.commit_directory_working_set(DirectoryDraftIntent {
        source: changed, selected: &selected, operation, principal: &principal,
        message: "identical selected body retains the existing directory head",
    }).unwrap();
    let result = engine.directory_draft_receipt(DirectoryDraftLookup {
        source: changed, operation_id: operation,
    }, &principal).unwrap();
    assert_eq!(result.result, DirectoryDraftOutcome::Unchanged { snapshot: changed });
    assert_eq!(observe(&fixture, &engine, initial).unwrap(), changed_head);
    let nodes = inventory(&fixture.snapshot_path());
    drop(engine);

    let engine = Engine::open(&fixture.system).unwrap();
    assert_eq!(observe(&fixture, &engine, initial).unwrap(), changed_head);
    assert_eq!(observe(&fixture, &engine, exact).unwrap(), changed_head);
    assert_eq!(inventory(&fixture.snapshot_path()), nodes);

    // An already-held root cannot be silently reissued from a same-name
    // replacement. The query itself must not repair, recreate or initialize it.
    let common = fixture.snapshot_path().parent().unwrap().to_path_buf();
    let parked = fixture.temporary.path().join("parked-registered-common");
    fs::rename(&common, &parked).unwrap();
    fs::create_dir(&common).unwrap();
    fs::set_permissions(&common, fs::Permissions::from_mode(0o700)).unwrap();
    let error = observe(&fixture, &engine, initial).unwrap_err();
    assert!(matches!(error.source, RegistrationError::Publication(_)));
    drop(engine);
    fs::remove_dir(&common).unwrap();
    fs::rename(&parked, &common).unwrap();
    let engine = Engine::open(&fixture.system).unwrap();
    assert_eq!(observe(&fixture, &engine, initial).unwrap(), changed_head);
    assert_eq!(inventory(&fixture.snapshot_path()), nodes);
}

#[test]
fn actual_snapshot_discovery_separates_pending_and_absent_without_cross_root_adoption() {
    let (fixture, mut engine) = Fixture::new();
    let principal = "admin".parse().unwrap();
    let first = request(&engine, fixture.targets[0].repository);
    let slot = engine.config.disks[1].slot_id.clone();
    let repository = engine.create_disk_root(
        &slot, "admin", OperationId::generate().unwrap(),
    ).unwrap();
    let second = request(&engine, repository.repository_id.parse().unwrap());
    assert_eq!(observe(&fixture, &engine, second).unwrap(), SnapshotHeadDiscovery::NotInitialized);
    let wrong_root = SnapshotHeadRequest { host: second.host, ..first };
    assert!(matches!(observe(&fixture, &engine, wrong_root).unwrap_err().source,
        RegistrationError::PublicationChanged));
    let wrong_repository = SnapshotHeadRequest {
        branch: SnapshotBranchSelection::Exact {
            repository: SnapshotRepositoryId::generate().unwrap(),
            branch: fixture.source.branch_id,
        },
        ..first
    };
    assert!(matches!(observe(&fixture, &engine, wrong_repository).unwrap_err().source,
        RegistrationError::Source));
    let mut stale = first;
    stale.host.expected_root_generation = stale.host.expected_root_generation.next().unwrap();
    assert!(matches!(observe(&fixture, &engine, stale).unwrap_err().source,
        RegistrationError::PublicationChanged));

    // Stop after the real permanent reserve and full physical preparation.
    // Local .zxs bytes are deliberately complete but not independently published.
    let context = engine.snapshot_root_context(second.namespace.repository_id).unwrap();
    let initialize = InitializeSnapshot {
        host: context.host, source: context.source, operation_id: OperationId::generate().unwrap(),
    };
    let host = prepare_namespace_host(
        &mut engine, &initialize.source, initialize.host.root_id,
        initialize.host.expected_root_generation, initialize.operation_id,
    ).unwrap();
    let allocated = NodeAllocation {
        repository: SnapshotRepositoryId::generate().unwrap(),
        branch: SnapshotBranchId::generate().unwrap(),
        node: SnapshotNodeId::generate().unwrap(),
        completion: OperationId::generate().unwrap(),
    };
    let (reserved, manifest) = publication::reserve(
        &mut engine, &host, &initialize, allocated, &principal,
    ).unwrap();
    let prepared = host.prepare_node(&mut engine, reserved, manifest).unwrap();
    prepared.require_engine(&engine).unwrap();
    let pending = SnapshotHeadDiscovery::Pending { operation: initialize.operation_id };
    assert_eq!(observe(&fixture, &engine, second).unwrap(), pending);
    let exact = SnapshotHeadRequest {
        branch: SnapshotBranchSelection::Exact {
            repository: allocated.repository, branch: allocated.branch,
        },
        ..second
    };
    assert_eq!(observe(&fixture, &engine, exact).unwrap(), pending);
    let cross = SnapshotHeadRequest { branch: exact.branch, ..first };
    assert!(matches!(observe(&fixture, &engine, cross).unwrap_err().source,
        RegistrationError::Source));
    drop(prepared);
    drop(engine);

    let engine = Engine::open(&fixture.system).unwrap();
    assert!(!engine.state().operations.contains_key(&allocated.completion));
    assert_eq!(observe(&fixture, &engine, second).unwrap(), pending);
    assert_eq!(observe(&fixture, &engine, exact).unwrap(), pending);
    assert_eq!(head(&observe(&fixture, &engine, first).unwrap()), fixture.source);
}
