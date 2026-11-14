use super::*;
use crate::{Config, Engine, NamespaceOperation, OperationId};
use name_policy::PortablePath;
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    os::unix::{
        ffi::OsStringExt,
        fs::{MetadataExt, PermissionsExt, symlink},
    },
    path::{Path, PathBuf},
};

fn make_source(root: &Path) -> PathBuf {
    let source = root.join("ordinary");
    fs::create_dir(&source).unwrap();
    fs::create_dir_all(source.join("旅行/empty")).unwrap();
    fs::write(source.join("旅行/notes.txt"), "你好\ncontents").unwrap();
    fs::write(source.join("binary.bin"), [0, 255, 17, 19]).unwrap();
    fs::write(source.join("zero"), []).unwrap();
    source
}
fn engine(root: &Path) -> Engine {
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    Engine::initialize(
        &root.join("system"),
        Config::development([root.join("primary")]).unwrap(),
    )
    .unwrap()
}
fn inspect(path: &Path) -> PreparedOrdinaryDirectory {
    PreparedOrdinaryDirectory::inspect(fs::File::open(path).unwrap(), path).unwrap()
}
fn operation() -> OperationId {
    OperationId::generate().unwrap()
}
fn confirmation(plan: &PreparedDirectoryImport) -> DirectoryConversionConfirmation {
    DirectoryConversionConfirmation::contents_only(plan.preview().digest)
}
fn contents(root: &Path) -> BTreeMap<PathBuf, (u32, u64, Vec<u8>)> {
    fn walk(base: &Path, path: &Path, output: &mut BTreeMap<PathBuf, (u32, u64, Vec<u8>)>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let meta = entry.metadata().unwrap();
            output.insert(
                entry.path().strip_prefix(base).unwrap().to_owned(),
                (
                    meta.mode(),
                    meta.ino(),
                    if meta.is_file() {
                        fs::read(entry.path()).unwrap()
                    } else {
                        Vec::new()
                    },
                ),
            );
            if meta.is_dir() {
                walk(base, &entry.path(), output);
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result);
    result
}
fn branch(engine: &Engine) -> String {
    engine
        .state()
        .repositories
        .values()
        .find(|repo| repo.file_manager_root)
        .unwrap()
        .main_branch_id
        .clone()
}

#[test]
fn source_preview_precedes_new_system_and_roundtrips_all_contents() {
    let temp = tempfile::tempdir().unwrap();
    let source = make_source(temp.path());
    let original = contents(&source);
    let held = inspect(&source);
    assert_eq!(held.preview().files, 3);
    assert_eq!(held.preview().directories, 2);
    assert_eq!(contents(&source), original);
    let mut engine = engine(temp.path());
    let plan = held.prepare_new_disk_import(&engine, "P01").unwrap();
    assert!(engine.state().repositories.is_empty());
    let digest = confirmation(&plan);
    let id = operation();
    let result = plan.execute(&mut engine, "admin", id, digest).unwrap();
    assert_eq!(result.operation_id, id);
    assert!(engine.state().operations.contains_key(&id));
    let entries = engine.list_tree(&branch(&engine)).unwrap();
    assert_eq!(entries.len(), 5);
    assert!(entries.iter().any(|entry| entry.path() == "旅行/empty"));
    for entry in entries
        .iter()
        .filter(|entry| entry.kind == crate::NamespaceEntryKind::File)
    {
        let mut opened = engine.open_content(&entry.version_id).unwrap();
        let mut bytes = Vec::new();
        opened.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, fs::read(source.join(entry.path())).unwrap());
    }
    assert_eq!(contents(&source), original);
    let before_system = contents(engine.root());
    let before_disk = contents(&engine.config().disks[0].root);
    let output = temp.path().join("export");
    let plan = engine
        .prepare_directory_export(&branch(&engine), None, &output)
        .unwrap();
    assert!(!output.exists());
    let digest = DirectoryConversionConfirmation::contents_only(plan.preview().digest);
    plan.execute(&engine, digest).unwrap();
    for (relative, (_, _, bytes)) in original {
        if source.join(&relative).is_file() {
            assert_eq!(fs::read(output.join(relative)).unwrap(), bytes);
        } else {
            assert!(output.join(relative).is_dir());
        }
    }
    assert_eq!(contents(engine.root()), before_system);
    assert_eq!(contents(&engine.config().disks[0].root), before_disk);
}

#[test]
fn empty_source_creates_an_empty_primary_and_permanent_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("empty");
    fs::create_dir(&source).unwrap();
    let mut engine = engine(temp.path());
    let plan = inspect(&source)
        .prepare_new_disk_import(&engine, "P01")
        .unwrap();
    let confirmation = confirmation(&plan);
    let id = operation();
    plan.execute(&mut engine, "admin", id, confirmation)
        .unwrap();
    assert!(engine.list_tree(&branch(&engine)).unwrap().is_empty());
    assert!(engine.state().operations.contains_key(&id));
}

#[test]
fn confirmation_source_and_target_rechecks_precede_writes() {
    let temp = tempfile::tempdir().unwrap();
    let source = make_source(temp.path());
    let mut engine = engine(temp.path());
    let plan = inspect(&source)
        .prepare_new_disk_import(&engine, "P01")
        .unwrap();
    let wrong = DirectoryConversionConfirmation::contents_only("0".repeat(64).parse().unwrap());
    let failure = plan
        .execute(&mut engine, "admin", operation(), wrong)
        .unwrap_err();
    assert_eq!(failure.cause().code(), ErrorCode::PlanInvalid);
    assert!(failure.workspace_id().is_none());
    assert!(engine.state().repositories.is_empty());
    let plan = inspect(&source)
        .prepare_new_disk_import(&engine, "P01")
        .unwrap();
    let digest = confirmation(&plan);
    fs::write(source.join("binary.bin"), b"changed").unwrap();
    let failure = plan
        .execute(&mut engine, "admin", operation(), digest)
        .unwrap_err();
    assert!(failure.workspace_id().is_none());
    assert!(engine.state().repositories.is_empty());
    let plan = inspect(&source)
        .prepare_new_disk_import(&engine, "P01")
        .unwrap();
    let digest = confirmation(&plan);
    engine
        .create_disk_root("P01", "admin", operation())
        .unwrap();
    let failure = plan
        .execute(&mut engine, "admin", operation(), digest)
        .unwrap_err();
    assert_eq!(failure.cause().code(), ErrorCode::PlanStale);
    assert!(failure.workspace_id().is_none());
}

#[test]
fn existing_destination_and_backup_targets_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let source = make_source(temp.path());
    let mut engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development([temp.path().join("primary"), temp.path().join("backup")]).unwrap(),
    )
    .unwrap();
    assert!(
        inspect(&source)
            .prepare_new_disk_import(&engine, "B01")
            .is_err()
    );
    let repo = engine
        .create_disk_root("P01", "admin", operation())
        .unwrap();
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::Mkdir {
                path: PortablePath::parse("Existing").unwrap(),
            },
            "admin",
            operation(),
        )
        .unwrap();
    assert!(
        inspect(&source)
            .prepare_new_disk_import(&engine, "P01")
            .is_err()
    );
    assert!(
        engine
            .prepare_directory_import(
                fs::File::open(&source).unwrap(),
                &source,
                &repo.main_branch_id,
                &PortablePath::parse("existing").unwrap()
            )
            .is_err()
    );
    let plan = engine
        .prepare_disk_directory_import(
            fs::File::open(&source).unwrap(),
            &source,
            "P01",
            &PortablePath::parse("new/copy").unwrap(),
        )
        .unwrap();
    assert_eq!(plan.preview().directories, 4);
    let digest = confirmation(&plan);
    plan.execute(&mut engine, "admin", operation(), digest)
        .unwrap();
    assert!(
        engine
            .list_tree(&repo.main_branch_id)
            .unwrap()
            .iter()
            .any(|entry| entry.path() == "new/copy/旅行/notes.txt")
    );
    assert!(
        engine
            .list_tree(&repo.main_branch_id)
            .unwrap()
            .iter()
            .any(|entry| entry.path() == "Existing")
    );
}

#[test]
fn refuses_links_special_files_bad_names_and_case_collisions() {
    for case in [
        "symlink",
        "hardlink",
        "fifo",
        "nonutf",
        "collision",
        "traversal",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("file"), b"kept").unwrap();
        match case {
            "symlink" => symlink("file", source.join("link")).unwrap(),
            "hardlink" => fs::hard_link(source.join("file"), source.join("link")).unwrap(),
            "fifo" => {
                rustix::fs::mknodat(
                    rustix::fs::CWD,
                    source.join("fifo"),
                    rustix::fs::FileType::Fifo,
                    rustix::fs::Mode::RUSR,
                    0,
                )
                .unwrap();
            }
            "nonutf" => fs::write(
                source.join(std::ffi::OsString::from_vec(vec![255])),
                b"invalid",
            )
            .unwrap(),
            "collision" => fs::write(source.join("FILE"), b"collision").unwrap(),
            "traversal" => fs::write(source.join("a\\b"), b"invalid").unwrap(),
            _ => unreachable!(),
        }
        assert!(
            PreparedOrdinaryDirectory::inspect(fs::File::open(&source).unwrap(), &source).is_err(),
            "{case}"
        );
        assert_eq!(fs::read(source.join("file")).unwrap(), b"kept");
    }
}

#[test]
fn rejects_zxs_sources_and_source_root_replacements() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(temp.path());
    assert!(
        PreparedOrdinaryDirectory::inspect(
            fs::File::open(&engine.config().disks[0].root).unwrap(),
            &engine.config().disks[0].root
        )
        .is_err()
    );
    let source = make_source(temp.path());
    let plan = inspect(&source)
        .prepare_new_disk_import(&engine, "P01")
        .unwrap();
    let digest = confirmation(&plan);
    fs::rename(&source, temp.path().join("moved")).unwrap();
    fs::create_dir(&source).unwrap();
    let failure = plan
        .execute(&mut engine, "admin", operation(), digest)
        .unwrap_err();
    assert!(failure.workspace_id().is_none());
    assert!(engine.state().repositories.is_empty());
    assert_eq!(
        fs::read(temp.path().join("moved/binary.bin")).unwrap(),
        [0, 255, 17, 19]
    );
}

#[test]
fn export_refuses_overwrite_and_corrupt_source_before_output_creation() {
    let temp = tempfile::tempdir().unwrap();
    let source = make_source(temp.path());
    let mut engine = engine(temp.path());
    let plan = inspect(&source)
        .prepare_new_disk_import(&engine, "P01")
        .unwrap();
    let digest = confirmation(&plan);
    plan.execute(&mut engine, "admin", operation(), digest)
        .unwrap();
    let branch = branch(&engine);
    let output = temp.path().join("export");
    let plan = engine
        .prepare_directory_export(&branch, None, &output)
        .unwrap();
    let digest = DirectoryConversionConfirmation::contents_only(plan.preview().digest);
    fs::create_dir(&output).unwrap();
    fs::write(output.join("keep"), b"keep").unwrap();
    assert!(plan.execute(&engine, digest).is_err());
    assert_eq!(fs::read(output.join("keep")).unwrap(), b"keep");
    let version = engine
        .list_tree_ref(&branch)
        .unwrap()
        .iter()
        .find(|entry| entry.kind == crate::NamespaceEntryKind::File)
        .unwrap();
    let version = engine.state().versions.get(&version.version_id).unwrap();
    let directory = crate::disk::version_dir(
        &engine.config().disks[0],
        &version.history_tree_id,
        &version.version_id,
    );
    let file = crate::container_storage::content_path(&directory, version).unwrap();
    let destination = temp.path().join("corrupt-export");
    let prepared = engine
        .prepare_directory_export(&branch, None, &destination)
        .unwrap();
    let confirmed = DirectoryConversionConfirmation::contents_only(prepared.preview().digest);
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&file, b"corrupt").unwrap();
    assert!(prepared.execute(&engine, confirmed).is_err());
    assert!(!destination.exists());
    assert!(
        engine
            .prepare_directory_export(&branch, None, &destination)
            .is_err()
    );
    assert!(!destination.exists());
}

#[test]
fn ordinary_disk_json_and_limits_are_handled_without_target_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let source = make_source(temp.path());
    fs::write(source.join("disk.json"), br#"{"ordinary":"document"}"#).unwrap();
    let source_plan = inspect(&source);
    let mut config = Config::development([temp.path().join("primary")]).unwrap();
    config.max_entries = 2;
    let engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    assert!(source_plan.prepare_new_disk_import(&engine, "P01").is_err());
    assert!(engine.state().repositories.is_empty());
}

#[test]
fn corrupt_or_partial_zxs_material_is_never_an_ordinary_source() {
    for (name, bytes) in [
        ("disk.json", b"{corrupt".as_slice()),
        ("disk.json", br#"{"disk_id":"disk_unknown"}"#.as_slice()),
        ("metadata/pool-membership/config.json", b"{}".as_slice()),
        ("metadata/application-log/current", b"partial".as_slice()),
        ("metadata/objects/unknown", b"partial".as_slice()),
        ("metadata/pool-membership", b"corrupt".as_slice()),
        ("metadata/application-log", b"corrupt".as_slice()),
        ("metadata/objects", b"corrupt".as_slice()),
        ("histories/history_unknown/partial", b"partial".as_slice()),
    ] {
        for prefix in ["", "archive/nested"] {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("source");
            fs::create_dir(&source).unwrap();
            let path = source.join(prefix).join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, bytes).unwrap();
            let original = contents(&source);
            assert!(
                PreparedOrdinaryDirectory::inspect(fs::File::open(&source).unwrap(), &source)
                    .is_err(),
                "{prefix}/{name}"
            );
            assert_eq!(contents(&source), original);
        }
    }
}

#[test]
fn progress_failure_stops_before_root_creation_or_content_staging() {
    for stop in [1, 2, 3, 4, 5] {
        let temp = tempfile::tempdir().unwrap();
        let source = make_source(temp.path());
        let original = contents(&source);
        let mut engine = engine(temp.path());
        let plan = inspect(&source)
            .prepare_new_disk_import(&engine, "P01")
            .unwrap();
        let digest = confirmation(&plan);
        let id = operation();
        let mut records = Vec::new();
        let failure = plan
            .execute_with_progress(&mut engine, "admin", id, digest, |progress| {
                records.push(progress.clone());
                if records.len() == stop {
                    return Err(Error::new(
                        ErrorCode::IoError,
                        "simulated durable progress write failure",
                    ));
                }
                Ok(())
            })
            .unwrap_err();
        assert_eq!(failure.operation_id(), id);
        assert_eq!(records.len(), stop);
        assert_eq!(records[0].operation_id(), id);
        assert!(records[0].root_operation_id().is_none());
        assert!(records[0].workspace_id().is_none());
        assert!(!engine.state().operations.contains_key(&id));
        assert_eq!(contents(&source), original);
        if stop < 3 {
            assert!(engine.state().repositories.is_empty());
            assert!(failure.workspace_id().is_none());
            if stop == 2 {
                assert!(failure.root_operation_id().is_some());
            }
        } else {
            let root = failure.root_operation_id().unwrap();
            assert!(engine.state().operations.contains_key(&root));
            assert!(engine.list_tree(&branch(&engine)).unwrap().is_empty());
            if stop == 3 {
                assert_eq!(
                    engine
                        .workspace(failure.workspace_id().unwrap())
                        .unwrap_err()
                        .code(),
                    ErrorCode::WorkspaceNotFound
                );
                assert!(engine.workspaces.is_empty());
                assert!(failure.request().is_none());
                continue;
            }
            let workspace = engine.workspace(failure.workspace_id().unwrap()).unwrap();
            if stop == 4 {
                assert!(workspace.files.is_empty());
                assert!(workspace.namespace_operations.is_empty());
                assert!(failure.request().is_none());
            } else {
                assert_eq!(workspace.files.len(), 3);
                assert!(!workspace.namespace_operations.is_empty());
                let request = failure.request().unwrap();
                assert_eq!(request.workspace_id, workspace.workspace_id);
                assert_eq!(request.operation_id, id);
                assert_eq!(
                    engine
                        .native_workspace_commit_completed(request)
                        .unwrap_err()
                        .code(),
                    ErrorCode::OperationNotFound
                );
                assert_eq!(
                    records.last().unwrap().request().unwrap().workspace_id,
                    request.workspace_id
                );
            }
        }
    }
}

#[test]
fn progress_captures_exact_native_receipt_binding() {
    let temp = tempfile::tempdir().unwrap();
    let source = make_source(temp.path());
    let mut engine = engine(temp.path());
    let plan = inspect(&source)
        .prepare_new_disk_import(&engine, "P01")
        .unwrap();
    let digest = confirmation(&plan);
    let id = operation();
    let mut retained = None;
    plan.execute_with_progress(&mut engine, "admin", id, digest, |progress| {
        if let Some(request) = progress.request() {
            retained = Some(request.clone());
        }
        Ok(())
    })
    .unwrap();
    let request = retained.unwrap();
    assert!(engine.native_workspace_commit_completed(&request).unwrap());
    let mut wrong = request.clone();
    wrong.message = "Another request".into();
    assert!(!engine.native_workspace_commit_completed(&wrong).unwrap());
    let system = engine.root().to_owned();
    drop(engine);
    let engine = Engine::open(&system).unwrap();
    assert!(engine.native_workspace_commit_completed(&request).unwrap());
    assert_eq!(engine.list_tree(&branch(&engine)).unwrap().len(), 5);
}

#[test]
fn source_change_while_persisting_publication_intent_never_publishes() {
    let temp = tempfile::tempdir().unwrap();
    let source = make_source(temp.path());
    let mut engine = engine(temp.path());
    let plan = inspect(&source)
        .prepare_new_disk_import(&engine, "P01")
        .unwrap();
    let digest = confirmation(&plan);
    let id = operation();
    let failure = plan
        .execute_with_progress(&mut engine, "admin", id, digest, |progress| {
            if progress.request().is_some() {
                fs::write(source.join("binary.bin"), b"concurrently changed")?;
            }
            Ok(())
        })
        .unwrap_err();
    assert!(failure.request().is_some());
    assert!(failure.workspace_id().is_some());
    assert_eq!(failure.cause().code(), ErrorCode::PlanInvalid);
    assert!(!engine.state().operations.contains_key(&id));
    assert!(engine.list_tree(&branch(&engine)).unwrap().is_empty());
    assert_eq!(
        fs::read(source.join("binary.bin")).unwrap(),
        b"concurrently changed"
    );
}

#[test]
fn nested_ordinary_metadata_and_git_objects_are_not_storage_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let source = make_source(temp.path());
    for (name, bytes) in [
        (
            "project/.git/objects/blob",
            b"ordinary git object".as_slice(),
        ),
        ("notes/metadata/readme", b"ordinary metadata".as_slice()),
        ("notes/histories/overview", b"ordinary histories".as_slice()),
        (
            "archive/disk.json",
            br#"{"ordinary":"document"}"#.as_slice(),
        ),
    ] {
        let path = source.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    let original = contents(&source);
    assert_eq!(inspect(&source).preview().files, 7);
    assert_eq!(contents(&source), original);
}

#[test]
fn nested_storage_appearing_after_preview_never_creates_a_destination_root() {
    let temp = tempfile::tempdir().unwrap();
    let source = make_source(temp.path());
    fs::create_dir(source.join("archive")).unwrap();
    let mut engine = engine(temp.path());
    let plan = inspect(&source)
        .prepare_new_disk_import(&engine, "P01")
        .unwrap();
    let digest = confirmation(&plan);
    fs::write(
        source.join("archive/disk.json"),
        br#"{"disk_id":"disk_unknown"}"#,
    )
    .unwrap();
    let original = contents(&source);
    let failure = plan
        .execute(&mut engine, "admin", operation(), digest)
        .unwrap_err();
    assert!(failure.workspace_id().is_none());
    assert!(failure.root_operation_id().is_none());
    assert!(engine.state().repositories.is_empty());
    assert_eq!(contents(&source), original);
}

#[path = "stop_tests.rs"]
mod stop_tests;

#[test]
fn fresh_target_budget_rejects_raw_long_names_without_touching_either_tree() {
    let temp = tempfile::tempdir().unwrap();
    let source = make_source(temp.path());
    let target = temp.path().join("empty-target");
    fs::create_dir(&target).unwrap();
    let held = fs::File::open(&target).unwrap();
    assert_eq!(rustix::fs::fstatvfs(&held).unwrap().f_namemax, 255);
    for length in [240, 241] {
        let relative = PathBuf::from("旅行").join("a".repeat(length));
        fs::write(source.join(&relative), b"must remain original").unwrap();
        let before = contents(&source);
        let result = PreparedOrdinaryDirectory::inspect_for_target(
            fs::File::open(&source).unwrap(), &source, &held,
        );
        match result {
            Err(Error::ConversionNameBudget(NameBudgetError::ComponentTooLong {
                path, bytes, allowed,
            })) => {
                assert_eq!(path, relative);
                assert_eq!(bytes, length);
                assert_eq!(allowed, 239);
            }
            _ => panic!("raw target budget must run before PortablePath limits"),
        }
        assert_eq!(contents(&source), before);
        assert!(contents(&target).is_empty());
        fs::remove_file(source.join(relative)).unwrap();
    }
    let accepted = format!("{}ab", "界".repeat(79));
    fs::write(source.join("旅行").join(accepted), b"239 UTF-8 bytes").unwrap();
    let before = contents(&source);
    let prepared = PreparedOrdinaryDirectory::inspect_for_target(
        fs::File::open(&source).unwrap(), &source, &held,
    ).unwrap();
    assert_eq!(prepared.preview().files, 4);
    assert_eq!(contents(&source), before);
    assert!(contents(&target).is_empty());
}

#[test]
fn registered_import_budget_rejects_source_and_destination_parent_before_writes() {
    let temp = tempfile::tempdir().unwrap();
    let source = make_source(temp.path());
    let mut engine = engine(temp.path());
    let long = format!("{}AAA", "界".repeat(79));
    fs::create_dir(source.join("旅行").join(&long)).unwrap();
    let original = contents(&source);
    let disk_before = contents(&engine.config().disks[0].root);
    let state_before = json_codec::to_vec(engine.state()).unwrap();
    let result = inspect(&source).prepare_new_disk_import(&engine, "P01");
    assert!(matches!(
        result,
        Err(Error::ConversionNameBudget(NameBudgetError::ComponentTooLong {
            bytes: 240, allowed: 239, ..
        }))
    ));
    assert_eq!(contents(&source), original);
    assert_eq!(contents(&engine.config().disks[0].root), disk_before);
    assert_eq!(json_codec::to_vec(engine.state()).unwrap(), state_before);

    fs::remove_dir(source.join("旅行").join(&long)).unwrap();
    let root = engine.create_disk_root("P01", "admin", operation()).unwrap();
    let disk_before = contents(&engine.config().disks[0].root);
    let state_before = json_codec::to_vec(engine.state()).unwrap();
    let destination = PortablePath::parse(&format!("{long}/child")).unwrap();
    let result = engine.prepare_disk_directory_import(
        fs::File::open(&source).unwrap(), &source, "P01", &destination,
    );
    assert!(matches!(
        result,
        Err(Error::ConversionNameBudget(NameBudgetError::ComponentTooLong {
            bytes: 240, allowed: 239, ..
        }))
    ));
    assert_eq!(contents(&engine.config().disks[0].root), disk_before);
    assert_eq!(json_codec::to_vec(engine.state()).unwrap(), state_before);
    // This existing namespace component is retained, not renamed or introduced.
    engine.file_namespace(
        &root.repository_id,
        NamespaceOperation::Mkdir { path: PortablePath::parse(&long).unwrap() },
        "admin",
        operation(),
    ).unwrap();
    let parent = engine.namespace_tree(&root.main_branch_id).unwrap()
        .get(&long).unwrap().clone();
    let source_before = contents(&source);
    let before = contents(temp.path());
    let state_before = json_codec::to_vec(engine.state()).unwrap();
    for path in [
        format!("{long}/{long}"),
        format!("{long}/{long}/child"),
        format!("{}/child", long.to_lowercase()),
    ] {
        let rejected = PortablePath::parse(&path).unwrap();
        let result = engine.prepare_disk_directory_import(
            fs::File::open(&source).unwrap(), &source, "P01", &rejected,
        );
        assert!(matches!(
            result,
            Err(Error::ConversionNameBudget(NameBudgetError::ComponentTooLong {
                path, bytes: 240, allowed: 239,
            })) if path == PathBuf::from(rejected.as_str())
        ));
        assert_eq!(contents(temp.path()), before);
        assert_eq!(json_codec::to_vec(engine.state()).unwrap(), state_before);
    }

    let accepted = format!("{}ab", "界".repeat(79));
    let destination = PortablePath::parse(&format!("{long}/{accepted}")).unwrap();
    let plan = engine.prepare_disk_directory_import(
        fs::File::open(&source).unwrap(), &source, "P01", &destination,
    ).unwrap();
    assert_eq!(contents(temp.path()), before);
    assert_eq!(json_codec::to_vec(engine.state()).unwrap(), state_before);
    let digest = confirmation(&plan);
    let operation = operation();
    let result = plan.execute(&mut engine, "admin", operation, digest).unwrap();
    assert_eq!(result.operation_id, operation);
    let namespace = engine.namespace_tree(&root.main_branch_id).unwrap();
    assert_eq!(namespace.get(&long), Some(&parent));
    assert!(namespace.get(destination.as_str()).is_some());
    for path in ["旅行", "旅行/empty", "旅行/notes.txt", "binary.bin", "zero"] {
        assert!(namespace.get(&format!("{}/{path}", destination.as_str())).is_some());
    }
    assert_eq!(contents(&source), source_before);
    let after = json_codec::to_vec(engine.state()).unwrap();
    drop(namespace);
    drop(engine);
    let reopened = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(json_codec::to_vec(reopened.state()).unwrap(), after);
    assert_eq!(
        reopened.namespace_tree(&root.main_branch_id).unwrap().get(&long),
        Some(&parent),
    );
    assert_eq!(contents(&source), source_before);
}

#[test]
fn registered_target_preview_checks_raw_budget_without_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let source = make_source(temp.path());
    let engine = engine(temp.path());
    let target = engine.registered_disks().next().unwrap().identity();
    let disk = &engine.config().disks[0].root;
    assert_eq!(
        rustix::fs::fstatvfs(&fs::File::open(disk).unwrap())
            .unwrap()
            .f_namemax,
        255
    );
    let disk_before = contents(disk);
    let state_before = json_codec::to_vec(engine.state()).unwrap();
    for suffix in ["", "x"] {
        let name = format!("{}{suffix}", "界".repeat(80));
        let relative = PathBuf::from("旅行").join(&name);
        fs::write(source.join(&relative), b"retain source bytes").unwrap();
        let source_before = contents(&source);
        let captured = PreparedOrdinaryDirectory::capture_registered_target(&engine, &target)
            .unwrap();
        let result = captured.inspect(fs::File::open(&source).unwrap(), &source);
        assert!(matches!(
            result,
            Err(Error::ConversionNameBudget(NameBudgetError::ComponentTooLong {
                path, bytes, allowed: 239,
            })) if path == relative && bytes == name.len()
        ));
        assert_eq!(contents(&source), source_before);
        assert_eq!(contents(disk), disk_before);
        assert_eq!(json_codec::to_vec(engine.state()).unwrap(), state_before);
        fs::remove_file(source.join(relative)).unwrap();
    }
    let name = format!("{}ab", "界".repeat(79));
    fs::write(source.join("旅行").join(name), b"239 UTF-8 bytes").unwrap();
    let source_before = contents(&source);
    let original = inspect(&source).preview().digest;
    let captured = PreparedOrdinaryDirectory::capture_registered_target(&engine, &target).unwrap();
    let prepared = captured
        .inspect(fs::File::open(&source).unwrap(), &source)
        .unwrap();
    assert_eq!(prepared.preview().digest, original);
    prepared
        .prepare_new_disk_import(&engine, &target.slot_id)
        .unwrap();
    assert_eq!(contents(&source), source_before);
    assert_eq!(contents(disk), disk_before);
    assert_eq!(json_codec::to_vec(engine.state()).unwrap(), state_before);
}

#[test]
fn registered_target_inventory_cannot_bind_another_registration() {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let source = make_source(temp.path());
    let mut config =
        Config::development([temp.path().join("first"), temp.path().join("second")]).unwrap();
    config.disks[1].level = 1;
    config.disks[1].upstream_slot_id = None;
    let engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let identities = engine
        .registered_disks()
        .map(|disk| disk.identity())
        .collect::<Vec<_>>();
    let expected = &identities[0];
    let before = contents(temp.path());
    let state_before = json_codec::to_vec(engine.state()).unwrap();
    assert_ne!(expected.disk_id, identities[1].disk_id);
    assert_ne!(expected.slot_id, identities[1].slot_id);
    // Both registrations are eligible primaries; a backup-role rejection would
    // never exercise the exact cross-registration inventory binding below.
    PreparedOrdinaryDirectory::capture_registered_target(&engine, &identities[1]).unwrap();
    let mut changed = expected.clone();
    changed.binding_generation += 1;
    assert!(matches!(
        PreparedOrdinaryDirectory::capture_registered_target(&engine, &changed),
        Err(Error::ConversionNameBudget(
            NameBudgetError::TargetRegistrationChanged
        ))
    ));
    let captured = PreparedOrdinaryDirectory::capture_registered_target(&engine, expected).unwrap();
    assert_eq!(contents(temp.path()), before);
    assert_eq!(json_codec::to_vec(engine.state()).unwrap(), state_before);
    // Capturing planning evidence must not retain Engine ownership during source I/O.
    drop(engine);
    let engine = Engine::open(&temp.path().join("system")).unwrap();
    let before = contents(temp.path());
    let state_before = json_codec::to_vec(engine.state()).unwrap();
    let prepared = captured
        .inspect(fs::File::open(&source).unwrap(), &source)
        .unwrap();
    assert!(matches!(
        prepared.prepare_new_disk_import(&engine, &identities[1].slot_id),
        Err(Error::ConversionNameBudget(
            NameBudgetError::TargetRegistrationChanged
        ))
    ));
    assert_eq!(contents(temp.path()), before);
    assert_eq!(json_codec::to_vec(engine.state()).unwrap(), state_before);
}
