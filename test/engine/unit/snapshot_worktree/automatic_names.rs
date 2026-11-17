//! Real root, original publication and sparse instances. No mock receipt index
//! or cloned State is installed to make an automatic claim appear occupied.
use super::{allocate_worktree, allocation_plan};
use crate::directory_worktree::session::ReadMaterial;
use crate::persist::snapshot_worktree::{self as codec, Record};
use crate::snapshot_worktree::{RegistrationError, create_worktree, publication, with_worktree};
use crate::{CommitRequest, Engine};
use name_policy::{LookupPath, Name};
use std::{fs, io::Read, sync::Arc};
use storage_contract::snapshot_worktree::WorktreeHandle;
use storage_contract::snapshot_worktree::creation::{
    AllocateWorktree, WorktreeCreationRequest, WorktreeNaming,
};
use storage_contract::snapshot_worktree::outcome::WorktreeCreationOutcome;
use storage_contract::{OperationId, Principal};

#[path = "draft_gateway/fixture.rs"]
mod fixture;
use fixture::{FIRST_ORIGINAL, Fixture, inventory};

fn principal() -> Principal {
    "admin".parse().unwrap()
}

fn request(fixture: &Fixture, engine: &Engine) -> AllocateWorktree {
    let context = engine
        .snapshot_root_context(fixture.targets[0].repository)
        .unwrap();
    AllocateWorktree {
        source: fixture.source,
        parent_directory_id: context.host.directory_id,
        expected_root_generation: context.host.expected_root_generation,
        operation_id: OperationId::generate().unwrap(),
        naming: WorktreeNaming::Automatic,
    }
}

fn assert_original(
    engine: &Engine,
    source: storage_contract::snapshot_worktree::SnapshotHandle,
    target: WorktreeHandle,
    name: &str,
) {
    let index = engine.snapshot_receipts().unwrap();
    let original = index.reopen_node(engine, source).unwrap();
    let instance = original
        .worktrees_directory()
        .path()
        .join(crate::persist::layout::worktree::instance(target.worktree_id).as_path());
    assert_eq!(fs::read_dir(instance).unwrap().count(), 0);
    assert_eq!(
        engine
            .directory_worktree_status(target)
            .unwrap()
            .name
            .as_str(),
        name
    );
    with_worktree(engine, target, |session| {
        let view = session.read(ReadMaterial::Original).unwrap();
        let mut file = view
            .open_file(&LookupPath::parse("first.txt").unwrap())
            .unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, FIRST_ORIGINAL);
        Ok(())
    })
    .unwrap();
}

#[test]
fn mount_inventory_requires_each_selected_instance_even_after_a_successful_read() {
    let (fixture, mut engine) = Fixture::new();
    let repository = fixture.targets[0].repository;
    let first = request(&fixture, &engine);
    let one = engine.allocate_directory_worktree(&first, &principal()).unwrap();
    let second = request(&fixture, &engine);
    let two = engine.allocate_directory_worktree(&second, &principal()).unwrap();
    let first_status = engine.directory_worktree_status(one).unwrap();
    let second_status = engine.directory_worktree_status(two).unwrap();
    let expected = vec![first_status.clone(), second_status.clone()];
    assert_eq!(engine.directory_worktree_mounts(repository).unwrap(), expected);

    let instance = {
        let index = engine.snapshot_receipts().unwrap();
        let node = index.reopen_node(&engine, fixture.source).unwrap();
        node.worktrees_directory().path().join(
            crate::persist::layout::worktree::instance(two.worktree_id).as_path(),
        )
    };
    let displaced = fixture.temporary.path().join("displaced-instance");
    fs::rename(&instance, &displaced).unwrap();
    fs::create_dir(&instance).unwrap();
    fs::set_permissions(&instance, fs::metadata(&displaced).unwrap().permissions()).unwrap();
    let media = fixture.media();
    assert_eq!(engine.directory_worktree_mounts_bounded(
        repository, std::num::NonZeroU16::new(1).unwrap(),
    ).unwrap_err().code(), storage_contract::ErrorCode::ResourceLimitExceeded);
    assert!(engine.directory_worktree_mounts(repository).is_err());
    assert!(engine.directory_worktree_status(two).is_err());
    assert!(engine.directory_worktree_mount(repository, &second_status.name).is_err());
    let unaffected = engine.directory_worktree_mount(repository, &first_status.name).unwrap();
    assert_eq!(unaffected, Some(first_status));
    assert_eq!(fixture.media(), media);

    fs::remove_dir(&instance).unwrap();
    fs::rename(&displaced, &instance).unwrap();
    drop(engine);
    let engine = Engine::open(&fixture.system).unwrap();
    assert_eq!(engine.directory_worktree_mounts(repository).unwrap(), expected);
}

#[test]
fn automatic_worktrees_allocate_real_parent_names_and_cold_retry_original_intent() {
    let (fixture, mut engine) = Fixture::new();
    let first = request(&fixture, &engine);
    let one = engine
        .allocate_directory_worktree(&first, &principal())
        .unwrap();
    assert_original(&engine, fixture.source, one, "worktree-main-001");
    let second = request(&fixture, &engine);
    let two = engine
        .allocate_directory_worktree(&second, &principal())
        .unwrap();
    assert_ne!(one.worktree_id, two.worktree_id);
    assert_original(&engine, fixture.source, two, "worktree-main-002");

    // A real ordinary container shares the same displayed root and occupies 003.
    let repository =
        engine.state().repositories[&fixture.targets[0].repository.to_string()].clone();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "WORKTREE-main-003",
            b"ordinary occupant",
        )
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "ordinary root occupancy".into(),
            request_digest: None,
        })
        .unwrap();
    let third = request(&fixture, &engine);
    let four = engine
        .allocate_directory_worktree(&third, &principal())
        .unwrap();
    assert_original(&engine, fixture.source, four, "worktree-main-004");
    let original = engine.operation(first.operation_id).unwrap();
    let completed = engine.operation(one.publication).unwrap();
    assert_eq!(original.kind, codec::WORKTREE_ALLOCATE);
    let receipt = engine
        .directory_worktree_creation_receipt(
            fixture.targets[0].repository,
            first.operation_id,
            &principal(),
        )
        .unwrap();
    assert_eq!(
        receipt.request,
        WorktreeCreationRequest::Automatic(first.clone())
    );
    assert_eq!(
        receipt.result,
        WorktreeCreationOutcome::Complete { target: one }
    );
    let Record::ReserveAllocatedWorktree(plan) = codec::decode(&original).unwrap().unwrap().record
    else {
        panic!("actual original automatic allocation");
    };
    assert_eq!(plan.request, first);
    assert_eq!(plan.resolved.request.name.as_str(), "worktree-main-001");
    let manual = plan.resolved.request;
    assert_eq!(
        json_codec::to_value(WorktreeCreationRequest::Named(manual.clone())).unwrap(),
        json_codec::to_value(&manual).unwrap(),
        "named request encoding stays unchanged"
    );
    let before = fixture.media();
    let state = Arc::clone(&engine.state);
    assert_eq!(
        engine
            .allocate_directory_worktree(&first, &principal())
            .unwrap(),
        one
    );
    assert!(
        matches!(create_worktree(&mut engine, &manual, &principal()),
        Err(RegistrationError::Digest(operation)) if operation == first.operation_id)
    );
    assert!(allocate_worktree(&mut engine, &first, &"other".parse().unwrap()).is_err());
    assert!(Arc::ptr_eq(&state, &engine.state));
    assert_eq!(fixture.media(), before);
    let snapshot = inventory(&fixture.snapshot_path());
    drop(engine);

    let mut engine = Engine::open(&fixture.system).unwrap();
    let before = fixture.media();
    let state = Arc::clone(&engine.state);
    assert_eq!(
        engine
            .allocate_directory_worktree(&first, &principal())
            .unwrap(),
        one
    );
    assert!(Arc::ptr_eq(&state, &engine.state));
    assert_eq!(engine.operation(first.operation_id).unwrap(), original);
    assert_eq!(engine.operation(one.publication).unwrap(), completed);
    assert_eq!(
        engine
            .directory_worktree_creation_receipt(
                fixture.targets[0].repository,
                first.operation_id,
                &principal(),
            )
            .unwrap(),
        receipt
    );
    assert_eq!(fixture.media(), before);
    assert_eq!(inventory(&fixture.snapshot_path()), snapshot);
}

#[test]
fn ordinary_publication_cannot_shadow_a_completed_worktree_mount() {
    reject_ordinary_shadow(false);
}

#[test]
fn ordinary_publication_cannot_shadow_a_pending_worktree_mount() {
    reject_ordinary_shadow(true);
}

fn reject_ordinary_shadow(pending: bool) {
    let (fixture, mut engine) = Fixture::new();
    let original = request(&fixture, &engine);
    if pending {
        let record = {
            let receipts = engine.snapshot_receipt_source().unwrap();
            let index = receipts.index().unwrap();
            let (_, plan) = allocation_plan(&engine, &index, &original).unwrap();
            Record::ReserveAllocatedWorktree(plan)
        };
        // A real SYSTEM decision owns this name before the instance mkdir.
        publication::publish_new(
            &mut engine, original.operation_id, &principal(), &record,
        ).unwrap();
    } else {
        engine.allocate_directory_worktree(&original, &principal()).unwrap();
    }
    let repository = fixture.targets[0].repository;
    let repo = engine.state().repositories[&repository.to_string()].clone();
    let state = Arc::clone(&engine.state);
    let media = fixture.media();
    for path in ["WORKTREE-main-001", "worktree-main-001/child"] {
        let operation = OperationId::generate().unwrap();
        let error = engine.file_namespace(
            &repo.repository_id,
            storage_contract::NamespaceOperation::Mkdir {
                path: name_policy::PortablePath::parse(path).unwrap(),
            },
            "admin", operation,
        ).unwrap_err();
        assert_eq!(error.code(), storage_contract::ErrorCode::NameCollision);
        assert!(!engine.state().operations.contains_key(&operation));
        assert!(Arc::ptr_eq(&state, &engine.state));
        assert_eq!(fixture.media(), media);
    }
    let birth = storage_contract::file_draft::BirthRequest {
        repository,
        path: name_policy::PortablePath::parse("WORKTREE-MAIN-001").unwrap(),
        expected_namespace_head: engine.state().branches[&repo.main_branch_id]
            .head_commit_id.parse().unwrap(),
    };
    let operation = OperationId::generate().unwrap();
    assert_eq!(engine.create_unborn_file(&birth, &principal(), operation).unwrap_err().code(),
        storage_contract::ErrorCode::NameCollision);
    assert!(!engine.state().operations.contains_key(&operation));
    assert!(Arc::ptr_eq(&state, &engine.state));
    assert_eq!(fixture.media(), media);

    let workspace = engine.create_workspace(&repo.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "worktree-main-001", FIRST_ORIGINAL).unwrap();
    let before_commit = fixture.media();
    let operation = OperationId::generate().unwrap();
    assert_eq!(engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: operation,
        principal: "admin".into(), expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "not an overwrite of the logical mount".into(), request_digest: None,
    }).unwrap_err().code(), storage_contract::ErrorCode::NameCollision);
    assert!(!engine.state().operations.contains_key(&operation));
    assert!(Arc::ptr_eq(&state, &engine.state));
    // Retaining the original workspace request may change SYSTEM, but no
    // ordinary container, snapshot or worktree on either data disk may change.
    assert_eq!(&fixture.media()[1..], &before_commit[1..]);

    let mut unrelated = birth;
    unrelated.path = name_policy::PortablePath::parse("worktree-main-009").unwrap();
    engine.create_unborn_file(&unrelated, &principal(), OperationId::generate().unwrap()).unwrap();
    let original_receipt = engine.operation(original.operation_id).unwrap();
    drop(engine);

    let mut engine = Engine::open(&fixture.system).unwrap();
    let state = Arc::clone(&engine.state);
    let media = fixture.media();
    let operation = OperationId::generate().unwrap();
    let birth = storage_contract::file_draft::BirthRequest {
        repository,
        path: name_policy::PortablePath::parse("worktree-MAIN-001").unwrap(),
        expected_namespace_head: engine.state().branches[&repo.main_branch_id]
            .head_commit_id.parse().unwrap(),
    };
    assert_eq!(engine.create_unborn_file(&birth, &principal(), operation).unwrap_err().code(),
        storage_contract::ErrorCode::NameCollision);
    assert!(Arc::ptr_eq(&state, &engine.state));
    assert_eq!(engine.operation(original.operation_id).unwrap(), original_receipt);
    assert_eq!(fixture.media(), media);
}

#[test]
fn automatic_pending_claim_is_not_reallocated_or_mistaken_for_a_named_request() {
    let (fixture, mut engine) = Fixture::new();
    let first = request(&fixture, &engine);
    let plan = {
        let receipts = engine.snapshot_receipt_source().unwrap();
        let index = receipts.index().unwrap();
        let (source, plan) = allocation_plan(&engine, &index, &first).unwrap();
        drop(source);
        plan
    };
    let record = Record::ReserveAllocatedWorktree(plan.clone());
    let mut different_plan = plan.clone();
    different_plan.resolved.request.name = Name::parse("worktree-main-999").unwrap();
    let different_plan = Record::ReserveAllocatedWorktree(different_plan);
    assert_eq!(
        record.digest(&principal()).unwrap(),
        different_plan.digest(&principal()).unwrap()
    );
    assert_ne!(
        record.commitment().unwrap(),
        different_plan.commitment().unwrap()
    );
    // Stop immediately after the real SYSTEM reservation, before any instance
    // preparation or independent completion. This is a genuine pending claim.
    publication::publish_new(&mut engine, first.operation_id, &principal(), &record).unwrap();
    let original = engine.operation(first.operation_id).unwrap();
    let mut oversized = original.clone();
    oversized.result["snapshot_worktree"]["record"]["resolved"]["host"]["directories"] =
        json_codec::Value::Array(vec![
            json_codec::Value::Null;
            crate::working_tree::admission::MAX_DIRECTORY_DEPTH + 1
        ]);
    assert!(matches!(
        codec::decode(&oversized),
        Err(codec::CodecError::Budget)
    ));
    let pending = engine
        .directory_worktree_creation_receipt(
            fixture.targets[0].repository,
            first.operation_id,
            &principal(),
        )
        .unwrap();
    assert_eq!(
        pending.request,
        WorktreeCreationRequest::Automatic(first.clone())
    );
    assert_eq!(pending.result, WorktreeCreationOutcome::Pending {});
    let before = fixture.media();
    let state = Arc::clone(&engine.state);
    assert!(
        matches!(allocate_worktree(&mut engine, &first, &principal()),
        Err(RegistrationError::Pending(operation)) if operation == first.operation_id)
    );
    assert!(
        matches!(create_worktree(&mut engine, &plan.resolved.request, &principal()),
        Err(RegistrationError::Digest(operation)) if operation == first.operation_id)
    );
    assert!(Arc::ptr_eq(&state, &engine.state));
    assert_eq!(fixture.media(), before);
    let second = request(&fixture, &engine);
    let two = engine
        .allocate_directory_worktree(&second, &principal())
        .unwrap();
    assert_original(&engine, fixture.source, two, "worktree-main-002");
    assert_eq!(engine.operation(first.operation_id).unwrap(), original);
    drop(engine);

    let mut engine = Engine::open(&fixture.system).unwrap();
    let before = fixture.media();
    assert!(
        matches!(allocate_worktree(&mut engine, &first, &principal()),
        Err(RegistrationError::Pending(operation)) if operation == first.operation_id)
    );
    assert_eq!(
        engine
            .directory_worktree_creation_receipt(
                fixture.targets[0].repository,
                first.operation_id,
                &principal(),
            )
            .unwrap(),
        pending
    );
    assert_eq!(engine.operation(first.operation_id).unwrap(), original);
    assert!(
        !engine
            .state()
            .operations
            .contains_key(&plan.resolved.allocated.completion)
    );
    assert_eq!(fixture.media(), before);
}
