use super::*;
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

pub(super) struct Fixture {
    pub(super) _temp: tempfile::TempDir,
    pub(super) engine: Engine,
    pub(super) source: PathBuf,
}
fn save(engine: &mut Engine, name: &str, slot: &str, copies: u32) {
    let repository = engine
        .create_repository(
            &RepositoryName::parse(name).unwrap(),
            slot,
            copies,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "document.txt", name.as_bytes())
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap();
}
pub(super) fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let mut config =
        Config::development((0..3).map(|index| temp.path().join(format!("disk-{index}")))).unwrap();
    config.disks[1].level = 1;
    config.disks[1].upstream_slot_id = None;
    config.disks[2].level = 2;
    config.disks[2].upstream_slot_id = Some("P01".into());
    let source = config.disks[2].root.clone();
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    save(&mut engine, "first-origin", "P01", 2);
    save(&mut engine, "second-origin", "B01", 1);
    Fixture {
        _temp: temp,
        engine,
        source,
    }
}
pub(super) fn inspect(path: &Path) -> Result<ForeignImportPreview> {
    ForeignImportPreview::inspect(
        File::open(path).unwrap(),
        path.into(),
        ForeignImportLimits::default(),
    )
}
fn copy_tree(source: &Path, target: &Path) {
    fs::create_dir(target).unwrap();
    fs::set_permissions(target, fs::Permissions::from_mode(0o700)).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn foreign_preview_keeps_full_histories_and_marks_other_primary_body_unavailable() {
    let f = fixture();
    let before = fs::read(f.source.join("disk.json")).unwrap();
    let preview = inspect(&f.source).unwrap();
    assert_eq!(preview.source_level(), 2);
    assert_eq!(preview.histories().len(), 2);
    let first = preview
        .histories()
        .iter()
        .find(|history| history.origin().slot_id() == "P01")
        .unwrap();
    let second = preview
        .histories()
        .iter()
        .find(|history| history.origin().slot_id() == "B01")
        .unwrap();
    assert_eq!(first.origin().disk_id(), f.engine.config().disks[0].disk_id);
    assert_eq!(
        second.origin().disk_id(),
        f.engine.config().disks[1].disk_id
    );
    assert!(
        first
            .availability()
            .values()
            .all(|state| *state == ForeignVersionAvailability::VerifiedLocal)
    );
    assert!(
        second
            .availability()
            .values()
            .all(|state| *state == ForeignVersionAvailability::TemporarilyUnavailable)
    );
    for history in preview.histories() {
        assert_eq!(history.manifest().commits.len(), 2);
        assert!(!history.manifest().branches.is_empty());
        assert!(
            history
                .manifest()
                .content_states
                .values()
                .all(|state| *state == ContentState::Available)
        );
        assert_eq!(
            history.origin_publications().len(),
            history.manifest().versions.len()
        );
    }
    assert_eq!(preview.original_seal(), before);
    assert_eq!(fs::read(f.source.join("disk.json")).unwrap(), before);
}

#[test]
fn a_moved_foreign_copy_keeps_logical_identity_without_admitting_the_old_physical_seal() {
    let f = fixture();
    let relocated = f._temp.path().join("relocated");
    copy_tree(&f.source, &relocated);
    let original = inspect(&f.source).unwrap();
    let moved = inspect(&relocated).unwrap();
    assert_eq!(original.source(), moved.source());
    assert_eq!(original.digest(), moved.digest());
    assert_ne!(original.local_binding(), moved.local_binding());
    assert_eq!(original.original_seal(), moved.original_seal());
    assert!(recognize_disk(File::open(&relocated).unwrap(), relocated).is_err());
}

#[test]
fn missing_body_retains_history_but_corrupt_body_rejects_without_repair() {
    let f = fixture();
    let version = f
        .engine
        .state()
        .versions
        .values()
        .find(|version| version.creation_primary_slot_id == "P01")
        .unwrap();
    let disk = f
        .engine
        .config()
        .disks
        .iter()
        .find(|disk| disk.root == f.source)
        .unwrap();
    let directory = version_dir(disk, &version.history_tree_id, &version.version_id);
    let path = crate::container_storage::content_path(&directory, version).unwrap();
    let body = fs::read(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&path, b"corrupt").unwrap();
    assert!(inspect(&f.source).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"corrupt");
    fs::write(&path, &body).unwrap();
    fs::remove_file(&path).unwrap();
    let preview = inspect(&f.source).unwrap();
    assert!(preview.histories().iter().all(|history| {
        history
            .manifest()
            .content_states
            .values()
            .all(|state| *state == ContentState::Available)
    }));
    assert!(
        preview
            .histories()
            .iter()
            .flat_map(|history| history.availability().values())
            .all(|state| *state == ForeignVersionAvailability::TemporarilyUnavailable)
    );
    assert!(!path.exists());
}

#[test]
fn absent_bound_history_pages_and_changed_configuration_fail_closed() {
    let f = fixture();
    let preview = inspect(&f.source).unwrap();
    let disk = f
        .engine
        .config()
        .disks
        .iter()
        .find(|disk| disk.root == f.source)
        .unwrap();
    let hash = preview.histories()[0].manifest_hash();
    let path = crate::journal::object_path(disk, "history-manifest", hash);
    let bytes = fs::read(&path).unwrap();
    fs::remove_file(&path).unwrap();
    assert!(inspect(&f.source).is_err());
    assert!(!path.exists());
    fs::write(&path, bytes).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(inspect(&f.source).is_ok());
    let anchor = f.source.join("metadata/pool-membership/config.json");
    let mut config = f.engine.config().clone();
    config.pool_id = crate::new_id("pool").unwrap();
    fs::set_permissions(&anchor, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&anchor, canonical_json(&config).unwrap()).unwrap();
    assert!(inspect(&f.source).is_err());
}

#[test]
fn inspection_budgets_never_turn_a_partial_history_into_success() {
    let f = fixture();
    for limits in [
        ForeignImportLimits::new(1, 1024 * 1024, 100, 1024).unwrap(),
        ForeignImportLimits::new(1024 * 1024, 1, 100, 1024).unwrap(),
        ForeignImportLimits::new(1024 * 1024, 1024 * 1024, 1, 1024).unwrap(),
        ForeignImportLimits::new(1024 * 1024, 1024 * 1024, 100, 0).unwrap(),
    ] {
        assert!(
            ForeignImportPreview::inspect(File::open(&f.source).unwrap(), f.source.clone(), limits)
                .is_err()
        );
    }
}

#[test]
fn mappings_bind_each_origin_and_refuse_collisions_unknown_groups_and_backup_targets() {
    let f = fixture();
    let preview = inspect(&f.source).unwrap();
    let target = tempfile::tempdir().unwrap();
    let config =
        Config::development([target.path().join("primary"), target.path().join("backup")]).unwrap();
    let engine = Engine::initialize(&target.path().join("system"), config).unwrap();
    let names = preview
        .histories()
        .iter()
        .enumerate()
        .map(|(index, history)| (history.group_id().to_owned(), format!("origin-{index}")))
        .collect::<BTreeMap<_, _>>();
    let before = engine.state().sequence;
    let mapping = preview
        .map_to_primary(&engine, "P01", None, &names)
        .unwrap();
    assert_eq!(mapping.histories().len(), 2);
    assert_eq!(mapping.target_sequence(), before);
    assert_eq!(mapping.source_digest(), preview.digest());
    assert_eq!(engine.state().sequence, before);
    assert!(engine.state().repositories.is_empty());
    assert!(
        preview
            .map_to_primary(&engine, "B01", None, &names)
            .is_err()
    );
    let mut missing = names.clone();
    missing.pop_first();
    assert!(
        preview
            .map_to_primary(&engine, "P01", None, &missing)
            .is_err()
    );
    let mut collision = names;
    for name in collision.values_mut() {
        *name = "same".into();
    }
    assert!(
        preview
            .map_to_primary(&engine, "P01", None, &collision)
            .is_err()
    );
}

#[test]
fn foreign_preview_never_reads_saved_paths_of_other_disks() {
    let Fixture {
        _temp,
        engine,
        source,
    } = fixture();
    let siblings = engine
        .config()
        .disks
        .iter()
        .filter(|disk| disk.root != source)
        .map(|disk| disk.root.clone())
        .collect::<Vec<_>>();
    drop(engine);
    for (index, path) in siblings.iter().enumerate() {
        fs::rename(path, _temp.path().join(format!("detached-{index}"))).unwrap();
        fs::create_dir(path).unwrap();
        fs::write(
            path.join("unrelated-user-data"),
            b"must not be read or modified",
        )
        .unwrap();
    }
    let preview = inspect(&source).unwrap();
    assert_eq!(preview.histories().len(), 2);
    for path in siblings {
        assert_eq!(
            fs::read(path.join("unrelated-user-data")).unwrap(),
            b"must not be read or modified"
        );
        assert_eq!(fs::read_dir(path).unwrap().count(), 1);
    }
}

#[test]
fn stale_journal_anchor_and_replaced_named_root_invalidate_the_review() {
    let f = fixture();
    let preview = inspect(&f.source).unwrap();
    let journal = fs::read(&preview.journal_path).unwrap();
    let mut torn = journal.clone();
    torn.extend_from_slice(b"torn");
    fs::write(&preview.journal_path, &torn).unwrap();
    assert!(preview.revalidate_source().is_err());
    assert!(inspect(&f.source).is_err());
    assert_eq!(fs::read(&preview.journal_path).unwrap(), torn);
    fs::write(&preview.journal_path, journal).unwrap();
    preview.revalidate_source().unwrap();
    let anchor = f.source.join("metadata/pool-membership/config.json");
    let bytes = fs::read(&anchor).unwrap();
    fs::set_permissions(&anchor, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&anchor, b"{}").unwrap();
    assert!(preview.revalidate_source().is_err());
    fs::write(&anchor, bytes).unwrap();
    preview.revalidate_source().unwrap();
    let generation = f.source.join("metadata/pool-membership/generation.json");
    fs::write(&generation, b"unpromoted").unwrap();
    assert!(preview.revalidate_source().is_err());
    assert!(inspect(&f.source).is_err());
    fs::remove_file(generation).unwrap();
    preview.revalidate_source().unwrap();
    fs::rename(&f.source, f._temp.path().join("removed-source")).unwrap();
    fs::create_dir(&f.source).unwrap();
    fs::set_permissions(&f.source, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(preview.revalidate_source().is_err());
    assert_eq!(fs::read_dir(&f.source).unwrap().count(), 0);
}

#[test]
fn authenticated_pages_do_not_admit_a_dangling_branch_graph() {
    let f = fixture();
    let preview = inspect(&f.source).unwrap();
    let disk = f
        .engine
        .config()
        .disks
        .iter()
        .find(|disk| disk.root == f.source)
        .unwrap();
    let history = &preview.histories()[0];
    let original =
        load_authenticated_object(disk, "history-manifest", history.manifest_hash()).unwrap();
    let mut root = crate::history_package::decode_history_root(&original).unwrap();
    let mut branches = history.manifest().branches.clone();
    branches.values_mut().next().unwrap().head_commit_id = new_id("commit").unwrap();
    let json_codec::Value::Object(branches) = json_codec::to_value(branches).unwrap() else {
        panic!("branch map")
    };
    let mut encoder = crate::metadata_map::Encoder::new(MAX_METADATA);
    root.maps
        .insert("branches".into(), encoder.map(branches).unwrap());
    let hash = encoder.object("history-manifest", &root).unwrap();
    let objects = encoder
        .finish()
        .into_iter()
        .map(|object| ((object.kind(), object.hash.clone()), object))
        .collect::<BTreeMap<_, _>>();
    let result = crate::history_package::read_bound_foreign_manifest(&hash, |kind, hash| {
        if let Some(object) = objects.get(&(kind, hash.to_owned())) {
            AuthenticatedMetadataObject::authenticate(kind, hash, decode_strict(object.bytes())?)
        } else {
            load_authenticated_object(disk, kind, hash)
        }
    });
    assert!(result.is_err());
    assert_eq!(inspect(&f.source).unwrap().digest(), preview.digest());
}

#[test]
fn present_body_requires_native_proof_and_symlinks_are_not_missing_content() {
    let f = fixture();
    let version = f
        .engine
        .state()
        .versions
        .values()
        .find(|version| version.creation_primary_slot_id == "P01")
        .unwrap();
    let disk = f
        .engine
        .config()
        .disks
        .iter()
        .find(|disk| disk.root == f.source)
        .unwrap();
    let directory = version_dir(disk, &version.history_tree_id, &version.version_id);
    let proof = directory.join("commit-proof.json");
    let proof_bytes = fs::read(&proof).unwrap();
    fs::remove_file(&proof).unwrap();
    assert!(inspect(&f.source).is_err());
    fs::write(&proof, proof_bytes).unwrap();
    fs::set_permissions(&proof, fs::Permissions::from_mode(0o600)).unwrap();
    inspect(&f.source).unwrap();
    let path = crate::container_storage::content_path(&directory, version).unwrap();
    let outside = f._temp.path().join("outside-body");
    fs::write(&outside, fs::read(&path).unwrap()).unwrap();
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&outside, &path).unwrap();
    assert!(inspect(&f.source).is_err());
    assert!(fs::symlink_metadata(path).unwrap().file_type().is_symlink());
    assert_eq!(fs::read(&outside).unwrap(), b"first-origin");
}

#[test]
fn mappings_require_an_empty_existing_parent_and_bind_the_target_checkpoint() {
    let f = fixture();
    let preview = inspect(&f.source).unwrap();
    let target = tempfile::tempdir().unwrap();
    let config =
        Config::independent_roots(Mode::Development, [target.path().join("primary")]).unwrap();
    let mut engine = Engine::initialize(&target.path().join("system"), config).unwrap();
    let repository = engine
        .create_disk_root("D01", "admin", OperationId::generate().unwrap())
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .namespace_operation(
            &workspace.workspace_id,
            NamespaceOperation::Mkdir {
                path: name_policy::PortablePath::parse("Restored").unwrap(),
            },
        )
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "existing.txt", b"existing data")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap();
    let names = preview
        .histories()
        .iter()
        .enumerate()
        .map(|(index, history)| (history.group_id().to_owned(), format!("origin-{index}")))
        .collect::<BTreeMap<_, _>>();
    assert!(
        preview
            .map_to_primary(&engine, "D01", None, &names)
            .is_err()
    );
    for missing in ["not-created", "existing.txt"] {
        assert!(
            preview
                .map_to_primary(
                    &engine,
                    "D01",
                    Some(&name_policy::PortablePath::parse(missing).unwrap()),
                    &names
                )
                .is_err()
        );
    }
    let parent = name_policy::PortablePath::parse("Restored").unwrap();
    let before = engine.state().sequence;
    let mapping = preview
        .map_to_primary(&engine, "D01", Some(&parent), &names)
        .unwrap();
    assert_eq!(mapping.target_sequence(), before);
    assert!(
        mapping
            .histories()
            .iter()
            .all(|history| history.path().as_str().starts_with("Restored/"))
    );
    assert_eq!(engine.state().sequence, before);
    let mut collision = names.clone();
    let mut values = collision.values_mut();
    *values.next().unwrap() = "Folder".into();
    *values.next().unwrap() = "folder".into();
    assert!(
        preview
            .map_to_primary(&engine, "D01", Some(&parent), &collision)
            .is_err()
    );
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "Restored/existing.txt",
            b"new data",
        )
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap();
    assert!(
        preview
            .map_to_primary(&engine, "D01", Some(&parent), &names)
            .is_err()
    );
}

#[test]
fn unsupported_empty_origin_never_becomes_a_partial_success() {
    let mut f = fixture();
    f.engine
        .create_repository(
            &RepositoryName::parse("empty-origin").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let before = fs::read(f.source.join("disk.json")).unwrap();
    let error = inspect(&f.source)
        .err()
        .expect("empty history lacks a version birth proof");
    assert_eq!(error.code(), ErrorCode::FormatUnsupported);
    assert_eq!(fs::read(f.source.join("disk.json")).unwrap(), before);
    assert_eq!(f.engine.state().histories.len(), 3);
}
