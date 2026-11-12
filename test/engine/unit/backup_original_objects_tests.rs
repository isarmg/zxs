//! Temporary ordinary-entity/database fixtures. These are not native Backup
//! authority, all-member selection, selector cutover or writer handoff tests.
use super::*;
use crate::{
    CommitRequest, Config, Engine, OperationId, directory_git::DirectoryGitInitializeBody,
    history_package::RawState,
};
use std::{fs, os::unix::fs::MetadataExt, path::Path};

struct Fixture {
    _temporary: tempfile::TempDir,
    _engine: Engine,
    root: DiskRoot,
    original: AdmittedState,
    published: AdmittedState,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let mut engine = Engine::initialize(
            &temporary.path().join("system"),
            Config::development([temporary.path().join("primary")]).unwrap(),
        )
        .unwrap();
        let repository = engine
            .create_disk_root("P01", "admin", OperationId::generate().unwrap())
            .unwrap();
        let workspace = engine
            .create_workspace(&repository.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(
                &workspace.workspace_id,
                "nested/file.txt",
                b"original full body",
            )
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate().unwrap(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "original object fixture".into(),
                request_digest: None,
            })
            .unwrap();
        for path in ["", "nested"] {
            let head = engine.state().branches[&repository.main_branch_id]
                .head_commit_id
                .clone();
            engine
                .initialize_directory_git(
                    &repository.repository_id,
                    &DirectoryGitInitializeBody {
                        path: path.into(),
                        expected_namespace_head: head,
                        operation_id: OperationId::generate().unwrap(),
                    },
                    "admin",
                )
                .unwrap();
        }
        engine
            .reconcile_directory_layout(&repository.repository_id)
            .unwrap();
        let original = admit(engine.state());
        // Storage-only next image: does not claim a native Publication. All
        // retained graph identities and source routes remain exactly original.
        let mut next = engine.state().clone();
        next.sequence += 1;
        let published = admit(&next);
        let handle = engine.disk_resources.get(&engine.config.disks[0]).unwrap();
        let root = handle.root.private_child(handle.root.path()).unwrap();
        Self {
            _temporary: temporary,
            _engine: engine,
            root,
            original,
            published,
        }
    }
    fn plan(&self) -> Result<Plan> {
        Plan::capture(
            &self.root,
            "P01",
            &self.original,
            &self.published,
            &AtomicBool::new(false),
        )
    }
}
fn admit(state: &crate::State) -> AdmittedState {
    let raw: RawState = storage_contract::decode_strict(&canonical_json(state).unwrap()).unwrap();
    AdmittedState::try_from(raw).unwrap()
}

#[test]
fn scoped_backup_database_snapshot_preserves_transition_projection_bytes() {
    let fixture = Fixture::new();
    let stop = AtomicBool::new(false);
    for admitted in [&fixture.original, &fixture.published] {
        let prepared = backup_databases::PreparedDatabases::capture(admitted, &stop).unwrap();
        assert_eq!(prepared.file_count(), 1);
        assert_eq!(prepared.directory_count(), 2);
        // The transition remains an independent reference for the original
        // capture contract; production only needs this same immutable State.
        let transition = crate::state_projection::ProjectionTransition::new(
            admitted.state(),
            admitted.state(),
            None,
        )
        .unwrap();
        for database in prepared.into_databases() {
            let (scope, actual) = database.into_parts();
            let expected = match scope {
                backup_databases::Scope::File { history, file } => {
                    file_database::prepare(transition.previous(), &file, &history).unwrap()
                }
                backup_databases::Scope::Directory { directory, .. } => {
                    directory_database::prepare(admitted.state(), &directory).unwrap()
                }
            };
            assert_eq!(actual, expected.bytes);
        }
    }
}

fn database_path(object: &Object) -> PathBuf {
    object.entity.root.path().join(object.entity.leaf())
}
fn stored(root: &Path, identity: &FileStamp) -> Vec<PathBuf> {
    let mut result = Vec::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        let metadata = fs::symlink_metadata(&path).unwrap();
        if metadata.is_dir() {
            result.extend(stored(&path, identity));
        } else if metadata.dev() == identity.device && metadata.ino() == identity.inode {
            result.push(path);
        }
    }
    result
}

#[test]
fn full_original_file_and_initialized_directory_databases_preserve_old_inodes() {
    let fixture = Fixture::new();
    let plan = fixture.plan().unwrap();
    assert!(
        plan.evidence
            .objects
            .iter()
            .any(|object| matches!(object.entity.scope, backup_databases::Scope::File { .. }))
    );
    assert_eq!(
        plan.evidence
            .objects
            .iter()
            .filter(|object| matches!(
                object.entity.scope,
                backup_databases::Scope::Directory { .. }
            ))
            .count(),
        2
    );
    let recipe = plan.recipe_hash().to_owned();
    let previous = plan
        .evidence
        .objects
        .iter()
        .map(|object| {
            let old = HeldRegular::capture(
                &object.entity.root,
                object.entity.leaf(),
                object.entity.maximum(),
            )
            .unwrap()
            .unwrap();
            (
                database_path(object),
                old.stamp().clone(),
                old.bytes().to_vec(),
            )
        })
        .collect::<Vec<_>>();
    plan.require_capacity(InstallAdmission::NewOperation, 0, &AtomicBool::new(false))
        .unwrap();
    let completed = plan.apply_preserving(&AtomicBool::new(false)).unwrap();
    let cold = fixture.plan().unwrap();
    assert_eq!(cold.recipe_hash(), recipe);
    assert_eq!(
        cold.verify(&AtomicBool::new(false)).unwrap().bytes(),
        completed.bytes()
    );
    for (active, stamp, bytes) in previous {
        let retained = stored(fixture.root.path(), &stamp);
        assert_eq!(retained.len(), 1);
        assert_ne!(retained[0], active);
        assert_eq!(fs::read(&retained[0]).unwrap(), bytes);
    }
}

#[test]
fn partial_original_object_exchange_resumes_the_same_recipe_without_body_copy() {
    let fixture = Fixture::new();
    let mut plan = fixture.plan().unwrap();
    let recipe = plan.recipe_hash().to_owned();
    let file = plan
        .evidence
        .objects
        .iter()
        .find(|object| matches!(object.entity.scope, backup_databases::Scope::File { .. }))
        .unwrap();
    let body = fs::read_dir(file.entity.root.path().join("versions"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let descriptor = fs::File::open(&body).unwrap();
    let before = descriptor.metadata().unwrap();
    plan.installs
        .remove(0)
        .apply(&plan.evidence.root, &mut |_| Ok(()))
        .unwrap();
    drop(plan);
    let cold = fixture.plan().unwrap();
    assert_eq!(cold.recipe_hash(), recipe);
    assert!(cold.verify(&AtomicBool::new(false)).is_err());
    cold.apply_preserving(&AtomicBool::new(false)).unwrap();
    let after = fs::metadata(&body).unwrap();
    assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
    assert_eq!(fs::read(body).unwrap(), b"original full body");
}

#[test]
fn completed_missing_database_is_not_repaired_by_read_only_verification() {
    let fixture = Fixture::new();
    let plan = fixture.plan().unwrap();
    let missing = database_path(&plan.evidence.objects[0]);
    plan.apply_preserving(&AtomicBool::new(false)).unwrap();
    fs::remove_file(&missing).unwrap();
    if let Ok(cold) = fixture.plan() {
        assert!(cold.verify(&AtomicBool::new(false)).is_err());
    }
    assert!(!missing.exists());
}

#[test]
fn equal_database_replacement_changes_completion_not_recipe() {
    let fixture = Fixture::new();
    let plan = fixture.plan().unwrap();
    let recipe = plan.recipe_hash().to_owned();
    let path = database_path(&plan.evidence.objects[0]);
    let complete = plan.apply_preserving(&AtomicBool::new(false)).unwrap();
    let held = fs::File::open(&path).unwrap();
    let bytes = fs::read(&path).unwrap();
    fixture.root.write_atomic(&path, &bytes, true).unwrap();
    assert_ne!(
        held.metadata().unwrap().ino(),
        fs::metadata(&path).unwrap().ino()
    );
    let cold = fixture.plan().unwrap();
    assert_eq!(cold.recipe_hash(), recipe);
    assert_ne!(
        cold.verify(&AtomicBool::new(false)).unwrap().bytes(),
        complete.bytes()
    );
}

#[test]
fn sqlite_sidecars_and_unknown_database_names_are_retained_before_any_exchange() {
    for suffix in ["-wal", "-shm", "-journal", ".unknown"] {
        let fixture = Fixture::new();
        let plan = fixture.plan().unwrap();
        let path = database_path(&plan.evidence.objects[0]);
        let old = fs::read(&path).unwrap();
        let sidecar = path.with_file_name(format!(
            "{}{suffix}",
            plan.evidence.objects[0].entity.leaf()
        ));
        drop(plan);
        fixture
            .root
            .write_atomic(&sidecar, b"unknown bytes", false)
            .unwrap();
        assert!(fixture.plan().is_err());
        assert_eq!(fs::read(&path).unwrap(), old);
        assert_eq!(fs::read(&sidecar).unwrap(), b"unknown bytes");
    }
}

#[test]
fn corrupted_old_sqlite_is_not_overwritten_as_a_derived_cache() {
    let fixture = Fixture::new();
    let plan = fixture.plan().unwrap();
    let path = database_path(&plan.evidence.objects[0]);
    drop(plan);
    fixture
        .root
        .write_atomic(&path, b"not a scoped SQLite image", true)
        .unwrap();
    assert!(fixture.plan().is_err());
    assert_eq!(fs::read(path).unwrap(), b"not a scoped SQLite image");
}

#[test]
fn missing_original_entity_is_not_reconstructed_from_complete_published_state() {
    let fixture = Fixture::new();
    let plan = fixture.plan().unwrap();
    let path = plan.evidence.objects[0].entity.root.path().to_path_buf();
    let retained = fixture.root.path().join("preserved-original-entity");
    drop(plan);
    fs::rename(&path, &retained).unwrap();
    assert!(fixture.plan().is_err());
    assert!(!path.exists());
    assert!(retained.is_dir());
}

#[test]
fn missing_initialized_directory_locator_is_not_inferred_from_a_path() {
    let fixture = Fixture::new();
    let plan = fixture.plan().unwrap();
    let object = plan
        .evidence
        .objects
        .iter()
        .find(|object| {
            matches!(
                object.entity.scope,
                backup_databases::Scope::Directory { .. }
            )
        })
        .unwrap();
    let directory = match &object.entity.scope {
        backup_databases::Scope::Directory { directory, .. } => directory.clone(),
        _ => unreachable!(),
    };
    let path = object
        .entity
        .history
        .join(".zxs-layout/directories")
        .join(format!("{directory}.json"));
    drop(plan);
    fs::remove_file(&path).unwrap();
    assert!(fixture.plan().is_err());
    assert!(!path.exists());
}

#[test]
fn capacity_overflow_and_cancellation_do_not_install_a_database() {
    let fixture = Fixture::new();
    let plan = fixture.plan().unwrap();
    let path = database_path(&plan.evidence.objects[0]);
    let old = fs::read(&path).unwrap();
    assert!(
        plan.require_capacity(
            InstallAdmission::NewOperation,
            u64::MAX,
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert!(plan.apply_preserving(&AtomicBool::new(true)).is_err());
    assert_eq!(fs::read(path).unwrap(), old);
}

#[test]
fn original_route_selection_includes_terminal_versions_without_requiring_a_live_head() {
    let fixture = Fixture::new();
    let mut original = fixture.original.state().clone();
    let version = original.versions.values().next().unwrap().clone();
    original
        .content_states
        .insert(version.version_id.clone(), crate::ContentState::Retired);
    // Pure source-scope selection only, not admission of a fabricated native
    // retirement operation. It must never filter by content-state or live HEAD.
    let selected =
        selection::capture(&fixture.root, "P01", &original, &AtomicBool::new(false)).unwrap();
    assert!(selected.contains_key(&Key::File(version.history_tree_id, version.file_id)));
}

#[test]
fn same_bytes_on_a_replaced_directory_locator_change_the_recipe() {
    let fixture = Fixture::new();
    let plan = fixture.plan().unwrap();
    let recipe = plan.recipe_hash().to_owned();
    let object = plan
        .evidence
        .objects
        .iter()
        .find(|object| {
            matches!(
                object.entity.scope,
                backup_databases::Scope::Directory { .. }
            )
        })
        .unwrap();
    let directory = match &object.entity.scope {
        backup_databases::Scope::Directory { directory, .. } => directory,
        _ => unreachable!(),
    };
    let path = object
        .entity
        .history
        .join(".zxs-layout/directories")
        .join(format!("{directory}.json"));
    let bytes = fs::read(&path).unwrap();
    fixture.root.write_atomic(&path, &bytes, true).unwrap();
    assert!(plan.verify(&AtomicBool::new(false)).is_err());
    assert_ne!(fixture.plan().unwrap().recipe_hash(), recipe);
}
