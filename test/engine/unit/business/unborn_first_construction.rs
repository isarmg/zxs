use super::*;
use crate::business::file_draft::unborn;
use crate::container_storage::PreparedUnbornFull;
use crate::{Config, Engine};
use name_policy::{Name, PortablePath};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use storage_contract::file_draft::{
    BirthRequest, DraftLocation, FileWorkingState, WorkingExpectation, WorkingSelection,
};
use storage_contract::{FileNodeId, Mode, OperationId, Principal, RepositoryId, Sequence};

type Inventory = BTreeMap<PathBuf, (u64, u64, Option<Vec<u8>>)>;

fn inventory(root: &Path) -> Inventory {
    fn visit(root: &Path, path: &Path, result: &mut Inventory) {
        let metadata = fs::symlink_metadata(path).unwrap();
        assert!(!metadata.file_type().is_symlink());
        let body = if metadata.is_file() { Some(fs::read(path).unwrap()) } else { None };
        result.insert(path.strip_prefix(root).unwrap().to_path_buf(),
            (metadata.dev(), metadata.ino(), body));
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), result);
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

fn birth(
    engine: &mut Engine, repository: RepositoryId, principal: &Principal, path: &str,
) -> DraftLocation {
    let repo = &engine.state().repositories[&repository.to_string()];
    let request = BirthRequest {
        repository, path: PortablePath::parse(path).unwrap(),
        expected_namespace_head: engine.state().branches[&repo.main_branch_id]
            .head_commit_id.parse().unwrap(),
    };
    let receipt = engine.create_unborn_file(
        &request, principal, OperationId::generate().unwrap(),
    ).unwrap();
    crate::persist::file_draft::birth::decode(&receipt).unwrap().target().clone()
}

fn save(engine: &mut Engine, target: &DraftLocation, principal: &Principal, bytes: &[u8]) {
    let mut writer = engine.begin_unborn_write(target, OperationId::generate().unwrap()).unwrap();
    assert_eq!(writer.write_at(&engine.planning_context(), 0, bytes).unwrap(), bytes.len());
    let prepared = Arc::new(writer.prepare(&engine.planning_context()).unwrap().unwrap());
    let request = prepared.request(target).unwrap();
    engine.save_unborn_working(&request, principal, Some(prepared)).unwrap();
}

fn read_working(engine: &Engine, target: &DraftLocation) -> Vec<u8> {
    let reader = engine.open_unborn_working(target).unwrap();
    let mut bytes = vec![0; reader.logical_size() as usize];
    assert_eq!(reader.read_at(&mut bytes, 0).unwrap(), bytes.len());
    bytes
}

#[test]
fn real_first_construction_accumulates_without_receipts_and_rejects_reused_allocations() {
    let temporary = tempfile::tempdir().unwrap();
    let system = temporary.path().join("system");
    let disk = temporary.path().join("primary");
    for path in [&system, &disk] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let config = Config::independent_roots(
        Mode::Development, [(Name::parse("Primary").unwrap(), disk)],
    ).unwrap();
    let slot = config.disks[0].slot_id.clone();
    let principal: Principal = "admin".parse().unwrap();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let repository = engine.create_disk_root(
        &slot, principal.as_str(), OperationId::generate().unwrap(),
    ).unwrap();
    let repository_id = repository.repository_id.parse().unwrap();
    let first = birth(&mut engine, repository_id, &principal, "first.txt");
    let second = birth(&mut engine, repository_id, &principal, "empty.txt");
    let untouched = birth(&mut engine, repository_id, &principal, "untouched.txt");
    save(&mut engine, &first, &principal, b"first saved material");
    save(&mut engine, &untouched, &principal, b"unselected stays a draft");
    assert!(engine.state().versions.is_empty());
    drop(engine);

    // All source rows and extents below came through real Birth/Save and cold
    // admission. The independent FULLs are preparations, never publications.
    let engine = Engine::open(&system).unwrap();
    let requests: Vec<_> = [&first, &second].into_iter().map(|target| WorkingSelection {
        target: target.clone(),
        expected: WorkingExpectation::from_working(
            &engine.state().file_containers[&target.file].working,
        ).unwrap(),
    }).collect();
    let selection = Arc::new(engine.select_working_sources(&requests).unwrap());
    let context = engine.planning_context();
    let full = [
        PreparedUnbornFull::prepare(&context, Arc::clone(&selection), 0).unwrap(),
        PreparedUnbornFull::prepare(&context, Arc::clone(&selection), 1).unwrap(),
    ];
    let operation = OperationId::generate().unwrap();
    let message = "one accumulated logical construction";
    let plans = requests.iter().zip(&full).map(|(request, material)| {
        FirstContentConstruction::prepare(
            &context, &request.target, request.expected.revision,
            message, &principal, operation, material,
        ).unwrap()
    }).collect::<Vec<_>>();
    let original = Arc::clone(context.state);
    let before_material = inventory(temporary.path());
    let untouched_record = original.file_containers[&untouched.file].clone();
    let original_head = original.branches[&repository.main_branch_id].head_commit_id.clone();
    let sequence = Sequence::new(context.next_slot);
    let mut next = unborn::begin(&original, sequence).unwrap();
    let mut prepared = Vec::new();
    let first_content = plans[0].append(&context, &mut next, &mut prepared).unwrap();
    let first_head = next.branches[&repository.main_branch_id].head_commit_id.clone();
    assert_eq!(next.commits[&first_head].parent_commit_ids, vec![original_head]);

    // The unchanged single-file wrapper and the extracted append retain the
    // same exact descriptor/trace projection. No byte codec is redefined here.
    let read = full[0].digest().unwrap();
    let single = unborn::project(
        &original, context.config, operation, sequence,
        unborn::Edit::FirstCommit(FirstCommit {
            target: &first, expected: requests[0].expected.revision,
            message, principal: &principal, content: &first_content, read: &read,
        }),
        context.config.max_entries, context.config.max_file_bytes,
    ).unwrap();
    let mut first_only = next.clone();
    crate::container_storage::capture_changed_names(&original, &mut first_only);
    assert_eq!(single.state(), &first_only);
    let second_content = plans[1].append(&context, &mut next, &mut prepared).unwrap();
    crate::container_storage::capture_changed_names(&original, &mut next);
    assert_eq!(prepared.len(), 2);
    let head = &next.commits[&next.branches[&repository.main_branch_id].head_commit_id];
    assert_eq!(head.parent_commit_ids, vec![first_head]);
    assert_eq!(head.entries.len(), 2);
    for (target, content) in [(&first, &first_content), (&second, &second_content)] {
        assert!(head.entries.iter().any(|entry| {
            entry.file_id == target.file.to_string() && entry.path == target.path
                && entry.version_id == content.version.version_id
        }));
        assert!(content.version.source_version_ids.is_empty());
        let trace = json_codec::to_value(&content.allocations).unwrap();
        let kinds = trace.as_array().unwrap().iter()
            .map(|row| row["kind"].as_str().unwrap()).collect::<Vec<_>>();
        assert_eq!(kinds, ["commit", "tree", "branch", "commit", "tree"]);

        let resolved = crate::business::file_draft::selection::resolve(&next, target).unwrap();
        assert_eq!(resolved.base.node_id(),
            FileNodeId::Version(content.version.version_id.parse().unwrap()));
        assert!(matches!(next.file_containers[&target.file].working,
            FileWorkingState::ManualClean { .. }));
    }
    assert_eq!(second_content.version.logical_size, 0);
    assert_eq!(next.file_containers[&untouched.file], untouched_record);
    assert_eq!(next.operations, original.operations);
    assert_eq!(next.operation_principals, original.operation_principals);
    assert_eq!(next.events, original.events);
    assert_eq!(next.namespace_directories, original.namespace_directories);
    // Two actual new independent FILE roots, each owning a commit of its
    // own and a distinct successor of the ordinary namespace root.
    assert_eq!(next.commits.len(), original.commits.len() + 4);
    assert_eq!(next.file_selections.len(), original.file_selections.len() + 2);

    WorkingNamespace::from_main_state(&next, repository_id, context.config.max_entries).unwrap();

    // Replay the real constructor facts into one accumulating State, with no
    // synthetic result/receipt and no material or writer minted from a trace.
    let namespace = WorkingNamespace::from_main_state(
        &original, repository_id, context.config.max_entries,
    ).unwrap();
    let mut replay = unborn::begin(&original, sequence).unwrap();
    for (index, content) in [&first_content, &second_content].into_iter().enumerate() {
        let read = full[index].digest().unwrap();
        unborn::first::project(
            &original, context.config, &namespace, &mut replay, operation,
            &FirstCommit {
                target: &requests[index].target, expected: requests[index].expected.revision,
                message, principal: &principal, content, read: &read,
            },
        ).unwrap();
    }
    crate::container_storage::capture_changed_names(&original, &mut replay);
    assert_eq!(replay, next);
    assert_eq!(inventory(temporary.path()), before_material);
    for (index, payload) in prepared.iter().enumerate() {
        let mut bytes = Vec::new();
        payload.source.open().unwrap().read_to_end(&mut bytes).unwrap();
        let expected: &[u8] = if index == 0 { b"first saved material" } else { b"" };
        assert_eq!(bytes, expected);
    }

    let read = full[1].digest().unwrap();
    let mut repeated = second_content.clone();
    repeated.allocations[0] = first_content.allocations[0];
    assert!(unborn::first::project(
        &original, context.config, &namespace, &mut first_only.clone(), operation,
        &FirstCommit {
            target: &second, expected: requests[1].expected.revision,
            message, principal: &principal, content: &repeated, read: &read,
        },
    ).is_err(), "a prior item's allocation cannot be reused");

    // A missing/replaced prepared FULL has no authority even with equal bytes.
    // The check precedes this append's Version allocation and local mutation.
    let parked = temporary.path().join("parked-empty-full");
    fs::rename(full[1].path(), &parked).unwrap();
    fs::copy(&parked, full[1].path()).unwrap();
    let fault_material = inventory(temporary.path());
    let mut rejected = first_only.clone();
    let mut rejected_prepared = Vec::new();
    assert!(plans[1].append(&context, &mut rejected, &mut rejected_prepared).is_err());
    assert_eq!(rejected, first_only);
    assert!(rejected_prepared.is_empty());
    assert_eq!(inventory(temporary.path()), fault_material);
    assert!(Arc::ptr_eq(&original, engine.planning_context().state));
    assert_eq!(engine.planning_context().next_slot, sequence.get());
    assert!(!engine.state().operations.contains_key(&operation));
    fs::remove_file(full[1].path()).unwrap();
    fs::rename(&parked, full[1].path()).unwrap();
    drop(read);
    drop(plans);
    drop(prepared);
    drop(full);
    drop(selection);
    drop(context);
    drop(engine);

    // The abandoned logical batch never clears the saved row. Reopening uses
    // only the actual published saves, not the orphan FULL names or test trace.
    let engine = Engine::open(&system).unwrap();
    assert!(engine.state().versions.is_empty());
    assert_eq!(engine.state().file_containers, original.file_containers);
    assert!(!engine.state().operations.contains_key(&operation));
    assert_eq!(read_working(&engine, &first), b"first saved material");
    assert_eq!(read_working(&engine, &second), b"");
    assert_eq!(read_working(&engine, &untouched), b"unselected stays a draft");
}

#[test]
fn unborn_container_blocks_portable_history_before_any_output_is_created() {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("state");
    let mut engine = Engine::initialize(&system, Config::development([
        (Name::parse("Primary").unwrap(), temp.path().join("disk")),
    ]).unwrap()).unwrap();
    let owner = "admin".parse::<Principal>().unwrap();
    let repo = engine.create_disk_root(
        "P01", owner.as_str(), OperationId::generate().unwrap(),
    ).unwrap();
    let target = birth(&mut engine, repo.repository_id.parse().unwrap(), &owner, "unborn.txt");
    save(&mut engine, &target, &owner, b"working bytes cannot be omitted");
    let before = inventory(temp.path());
    let destination = temp.path().join("new-parent/package");
    let error = engine.export_history_package(&repo.history_tree_id, &destination).unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::FileBranchConflict);
    let error = engine.prepare_backup_history(&repo.history_tree_id).err().unwrap();
    assert_eq!(error.code(), storage_contract::ErrorCode::FileBranchConflict);
    let operation_id = OperationId::generate().unwrap();
    let error = engine.configure_backups_scoped(crate::ScopedBackupConfigurationRequest {
        repository_id: &repo.repository_id,
        path: "",
        expected_scope_id: Some(&repo.repository_id),
        target_copies: 1,
        backup_prefixes: &BTreeMap::new(),
        expected_generation: 0,
        principal: owner.as_str(),
        operation_id,
    }).unwrap_err();
    assert!(matches!(error, crate::Error::FileDraft(crate::business::file_draft::DraftError::UnbornRequired)));
    assert!(!engine.state().operations.contains_key(&operation_id));
    assert_eq!(inventory(temp.path()), before);
    assert!(!destination.parent().unwrap().exists());
}
