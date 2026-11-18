//! PlannedOffline lends current metadata, not a disk or a hidden writable namespace.
use crate::{ApiError, Backend, ListingSnapshot, ListingVersion, ReadListing, auth::Identity,
    engine_backend::{EngineBackend, install_test_administrator}};
use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt, path::Path};
use storage_contract::{Config, DiskConfig, Entry, ErrorCode, OperationId, Repository};
use storage_engine::{CommitRequest, Engine, MembershipExitAction, ProtectionPolicyUpdate};

fn op() -> OperationId { OperationId::generate().unwrap() }

fn pool() -> (tempfile::TempDir, Engine, DiskConfig, Repository, Identity) {
    let root = crate::tests::private_tempdir();
    let mut config = Config::development(crate::test_disks(
        (0..4).map(|i| root.path().join(format!("disk-{i}"))),
    )).unwrap();
    // P01, B01 and C01 remain the voters. D04 is the independently registered
    // primary that will stop; it has no backup donor or downstream replica.
    let stopped = &mut config.disks[3];
    stopped.level = 1;
    stopped.upstream_slot_id = None;
    stopped.metadata_member = false;
    let stopped = stopped.clone();
    let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    let repo = engine.create_disk_root(&stopped.slot_id, "owner", op()).unwrap();
    let owner = install_test_administrator(&mut engine, "owner");
    (root, engine, stopped, repo, owner)
}

fn save(engine: &mut Engine, repo: &Repository, path: &str, body: &[u8]) -> Entry {
    let work = engine.create_workspace(&repo.main_branch_id, "owner").unwrap();
    engine.stage_bytes(&work.workspace_id, path, body).unwrap();
    engine.commit_workspace(CommitRequest {
        workspace_id: work.workspace_id, operation_id: op(), principal: "owner".into(),
        expected_head: work.base_head, fencing_token: work.fencing_token,
        message: "ordinary FILE".into(), request_digest: None,
    }).unwrap();
    engine.namespace_tree(&repo.main_branch_id).unwrap().get(path).unwrap().clone()
}

fn list(backend: &EngineBackend, identity: &Identity, slot: &str, path: &str)
    -> Result<ListingSnapshot, ApiError>
{
    let params = BTreeMap::from([("path".into(), path.into())]);
    backend.listing(identity, ReadListing::parse(slot.into(), params).unwrap())
}

const STOPPED: &[u8] = b"not a directory or a registered disk";
fn stop(backend: &EngineBackend, disk: &DiskConfig, temp: &Path) {
    let mut engine = backend.engine.lock().unwrap();
    engine.begin_disk_exit(&disk.disk_id, MembershipExitAction::PlannedOffline, "owner", op()).unwrap();
    let archive = engine.build_managed_membership_control_archive().unwrap();
    engine.enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new()).unwrap();
    engine.resume_membership_joint_decision().unwrap();
    engine.activate_pending_disk_exit().unwrap();
    assert_eq!(engine.registered_disk(&disk.disk_id).unwrap().registration_mode,
        storage_engine::RegistrationMode::Disabled);
    fs::rename(&disk.root, temp.join("original-stopped-disk")).unwrap();
    fs::write(&disk.root, STOPPED).unwrap();
}

#[test]
fn planned_offline_primary_keeps_exact_read_only_file_context_without_opening_the_stopped_disk() {
    let (root, mut engine, stopped, repo, owner) = pool();
    let entry = save(&mut engine, &repo, "desk/project.zxs", b"current ordinary FILE");
    let head = engine.state().branches[&repo.main_branch_id].head_commit_id.clone();

    // A separate, actual ordinary file has a real copy on B01. It must not
    // become a fallback namespace or a content source for the stopped FILE.
    let other = engine.create_disk_root("P01", "owner", op()).unwrap();
    engine.update_protection_policy(ProtectionPolicyUpdate {
        policy_id: &storage_contract::new_id("policy").unwrap(),
        repository_id: &other.repository_id, path_prefix: "", target_copies: 2,
        expected_generation: 0, principal: "owner", operation_id: op(),
    }).unwrap();
    let replica = save(&mut engine, &other, "replica.bin", b"other origin");
    assert!(engine.state().placements[&replica.version_id].route_slot_ids.iter().any(|s| s == "B01"));
    assert_eq!(engine.state().placements[&entry.version_id].route_slot_ids, [stopped.slot_id.clone()]);
    let mut backend = EngineBackend::new(engine);
    let enabled = list(&backend, &owner, &stopped.slot_id, "desk").unwrap();
    assert!(enabled.writable && !enabled.backup);
    assert_eq!(enabled.entries[0].entry_id, entry.entry_id);

    // While Enabled, a perfectly ordinary same-named replacement directory
    // at the owner's physical address does not authorize even the cached row.
    let common = stopped.root.join("common-data");
    let held = root.path().join("original-common");
    fs::rename(&common, &held).unwrap();
    fs::create_dir(&common).unwrap();
    fs::set_permissions(&common, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(list(&backend, &owner, &stopped.slot_id, "desk").is_err());
    fs::rename(&common, root.path().join("wrong-common")).unwrap();
    fs::rename(&held, &common).unwrap();
    assert!(list(&backend, &owner, &stopped.slot_id, "desk").unwrap().writable);

    stop(&backend, &stopped, root.path());
    for cold in [false, true] {
        if cold {
            drop(backend);
            backend = EngineBackend::new(Engine::open(&root.path().join("system")).unwrap());
        }
        let before = storage_contract::canonical_json(backend.engine.lock().unwrap().state()).unwrap();
        let page = list(&backend, &owner, &stopped.slot_id, "desk").unwrap();
        assert!(!page.writable && !page.backup);
        assert_eq!(page.path, "desk");
        assert_eq!(page.slot_id, stopped.slot_id);
        let context = page.context.as_ref().expect("registered ordinary context");
        assert_eq!(context.repository_id, repo.repository_id);
        assert_eq!(context.repository_path, page.path);
        assert_eq!(context.branch_id, repo.main_branch_id);
        assert_eq!(context.base_head, head);
        assert_eq!((page.directory_file_count, page.total_entries), (1, 1));
        let [row] = page.entries.as_slice() else { panic!("one admitted file") };
        assert_eq!(row.entry_id, entry.entry_id);
        assert_eq!(row.file_id, entry.file_id);
        assert_eq!(row.name, "project.zxs");
        assert_eq!(row.path, entry.path.as_str());
        assert_eq!(row.size, entry.size);
        assert!(!row.backup);
        assert_eq!(row.read_only, Some(true));
        let ListingVersion::File { content, backup_version: None } = &row.version else {
            panic!("an ordinary published FILE, not a backup or tentative row");
        };
        assert_eq!(content.version_id.as_ref().or(content.source_version_id.as_ref()), Some(&entry.version_id));
        let source = row.location.as_ref().expect("ordinary FILE owner");
        assert_eq!(source.repository_id, repo.repository_id);
        assert_eq!(source.repository_path.as_deref(), Some("desk/project.zxs"));

        let backup = list(&backend, &owner, "B01", "P01").unwrap();
        assert!(backup.backup && !backup.writable && backup.context.is_none());
        let [actual] = backup.entries.as_slice() else { panic!("one independently copied FILE") };
        assert!(actual.backup && actual.read_only == Some(true));
        assert_eq!(actual.file_id, replica.file_id);
        assert!(matches!(&actual.version, ListingVersion::File { backup_version: Some(id), .. }
            if id == &replica.version_id));
        let wrong = list(&backend, &owner, "P01", "desk").err().expect("not this primary's path");
        assert_eq!(wrong.code, ErrorCode::DirectoryNotFound);
        let engine = backend.engine.lock().unwrap();
        assert!(engine.ordinary_root_for_slot(&stopped.slot_id).is_err());
        assert!(engine.open_content(&entry.version_id).is_err());
        assert_eq!(fs::read(&stopped.root).unwrap(), STOPPED);
        assert_eq!(storage_contract::canonical_json(engine.state()).unwrap(), before);
    }
}

#[test]
fn planned_offline_primary_with_unborn_source_refuses_an_incomplete_listing() {
    let (root, mut engine, stopped, repo, owner) = pool();
    let request = storage_contract::file_draft::BirthRequest {
        repository: repo.repository_id.parse().unwrap(),
        path: name_policy::PortablePath::parse("pending.zxs").unwrap(),
        expected_namespace_head: engine.state().branches[&repo.main_branch_id].head_commit_id.parse().unwrap(),
    };
    engine.create_unborn_file(&request, &"owner".parse().unwrap(), op()).unwrap();
    let mut backend = EngineBackend::new(engine);
    let page = list(&backend, &owner, &stopped.slot_id, "").unwrap();
    assert!(page.writable && !page.backup);
    let [row] = page.entries.as_slice() else { panic!("one actual Unborn before stop") };
    assert!(matches!(&row.version, ListingVersion::Unborn { target }
        if target.repository == request.repository && target.path == request.path));
    stop(&backend, &stopped, root.path());
    for cold in [false, true] {
        if cold {
            drop(backend);
            backend = EngineBackend::new(Engine::open(&root.path().join("system")).unwrap());
        }
        let failure = list(&backend, &owner, &stopped.slot_id, "").err()
            .expect("no proven working-row metadata; never present an empty or historical listing");
        assert_eq!(failure.code, ErrorCode::UnsupportedOperation);
        assert_eq!(fs::read(&stopped.root).unwrap(), STOPPED);
    }
}
