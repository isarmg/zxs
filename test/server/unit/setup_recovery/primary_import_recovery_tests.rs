use super::*;
use std::{io::Read, os::unix::fs::MetadataExt};
use storage_contract::{CommitRequest, ErrorCode, OperationId, WorkspaceState};

#[derive(Debug, PartialEq, Eq)]
struct Witness {
    device: u64,
    inode: u64,
    mode: u32,
    bytes: Vec<u8>,
}

fn witness(path: &Path) -> Witness {
    let metadata = fs::metadata(path).unwrap();
    assert!(metadata.is_file() || metadata.is_dir());
    Witness {
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.mode(),
        bytes: if metadata.is_file() {
            fs::read(path).unwrap()
        } else {
            Vec::new()
        },
    }
}

fn tree(root: &Path) -> BTreeMap<PathBuf, Witness> {
    let mut pending = vec![root.to_path_buf()];
    let mut result = BTreeMap::new();
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            for entry in fs::read_dir(&path).unwrap() {
                pending.push(entry.unwrap().path());
            }
        }
        result.insert(
            path.strip_prefix(root).unwrap().to_path_buf(),
            witness(&path),
        );
        assert!(result.len() < 100, "bounded phase-one fixture");
    }
    result
}

fn restarted(service: &SetupRecovery) -> SetupRecovery {
    SetupRecovery::new(
        service.data_root.clone(),
        service.management_root.clone(),
        BTreeMap::new(),
    )
    .unwrap()
    .with_discovery_root(service.discovery_root.clone())
}

fn recovered_primary() -> (Fixture, Engine) {
    let f = fixture_with_disks(&["one", "untouched"]);
    initialize(&f.service);
    let error = f
        .service
        .activate(
            &admin(),
            request(&f.service, &["one"]),
            |_| Ok(()),
            |_| {
                Err(ApiError::conflict(
                    ErrorCode::RecoveryRequired,
                    "test interruption after genesis",
                ))
            },
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::RecoveryRequired);
    assert_eq!(
        f.service.activation_admitted().unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
    let config = fs::read(f.service.data_root.join("config.json")).unwrap();
    let seal = witness(&f.service.disks["one"].join("disk.json"));
    let unused = witness(&f.service.disks["untouched"]);
    let cold = restarted(&f.service);
    let mut installed = None;
    cold.complete_activation(
        &admin(),
        |_| Ok(()),
        |engine| {
            installed = Some(engine);
            Ok(())
        },
    )
    .unwrap();
    cold.activation_admitted().unwrap();
    let engine = installed.unwrap();
    assert_eq!(engine.config().disks.len(), 1);
    assert_eq!(engine.config().metadata_quorum(), 1);
    let disk = &engine.config().disks[0];
    assert_eq!(disk.level, 1);
    assert!(disk.upstream_slot_id.is_none());
    assert_eq!(
        fs::read(f.service.data_root.join("config.json")).unwrap(),
        config
    );
    assert_eq!(witness(&f.service.disks["one"].join("disk.json")), seal);
    assert_eq!(witness(&f.service.disks["untouched"]), unused);
    assert!(
        fs::read_dir(&f.service.disks["untouched"])
            .unwrap()
            .next()
            .is_none()
    );
    (f, engine)
}

fn stage_import(f: &Fixture, engine: &mut Engine) -> (PathBuf, CommitRequest) {
    let source = f._temp.path().join("ordinary-source");
    fs::create_dir_all(source.join("旅行/empty")).unwrap();
    fs::write(source.join("旅行/notes.txt"), "一期文件\n").unwrap();
    fs::write(source.join("binary.bin"), [0, 255, 17, 19]).unwrap();
    fs::write(source.join("zero"), []).unwrap();
    let slot = engine.config().disks[0].slot_id.clone();
    let prepared = engine
        .prepare_ordinary_directory(fs::File::open(&source).unwrap(), &source)
        .unwrap()
        .prepare_new_disk_import(engine, &slot)
        .unwrap();
    let confirmation =
        storage_engine::DirectoryConversionConfirmation::contents_only(prepared.preview().digest);
    let import = OperationId::generate().unwrap();
    // Stop only after the real importer has created its root/workspace and
    // staged every source entry, so the later fault cannot hit root creation.
    let failure = prepared
        .execute_with_progress(
            engine,
            &admin().principal,
            import,
            confirmation,
            |progress| {
                if progress.request().is_some() {
                    return Err(storage_engine::Error::from(std::io::Error::other(
                        "retain exact import request",
                    )));
                }
                Ok(())
            },
        )
        .unwrap_err();
    assert!(failure.root_operation_id().is_some());
    let request = failure.request().unwrap().clone();
    assert_eq!(request.operation_id, import);
    assert_eq!(Some(request.workspace_id.as_str()), failure.workspace_id());
    (source, request)
}

fn native_requests(engine: &Engine, request: &CommitRequest) {
    for directory in [
        engine.root().join("workspaces").join(&request.workspace_id),
        engine.config().disks[0]
            .root
            .join("staging/workspaces")
            .join(&request.workspace_id),
    ] {
        let saved: CommitRequest = read_control(&directory.join("commit-request.json")).unwrap();
        assert_eq!(
            json_codec::to_vec(&saved).unwrap(),
            json_codec::to_vec(request).unwrap()
        );
    }
}

fn injected_indeterminate(error: &storage_engine::Error, point: &str) {
    assert_eq!(error.code(), ErrorCode::InDoubt, "{point}");
    // Context owns its typed boxed cause; transparent message wrappers need
    // not appear as an unboxed Engine Error in the dyn Error::source chain.
    let storage_engine::Error::Context { source, .. } = error else {
        panic!("indeterminate publication must retain its concrete cause: {point}")
    };
    assert_eq!(source.code(), ErrorCode::InjectedFailure, "{point}");
    assert!(error.is_retryable());
}

fn published_contents(engine: &Engine, request: &CommitRequest, source: &Path) {
    let workspace = engine.workspace(&request.workspace_id).unwrap();
    let entries = engine.list_tree(&workspace.branch_id).unwrap();
    use storage_contract::NamespaceEntryKind::{Directory, File};
    let actual = entries
        .iter()
        .map(|entry| (entry.path().to_owned(), entry.kind))
        .collect::<BTreeMap<_, _>>();
    let expected = BTreeMap::from([
        ("binary.bin".to_owned(), File),
        ("zero".to_owned(), File),
        ("旅行".to_owned(), Directory),
        ("旅行/empty".to_owned(), Directory),
        ("旅行/notes.txt".to_owned(), File),
    ]);
    assert_eq!(actual, expected);
    for entry in entries
        .iter()
        .filter(|entry| entry.kind == storage_contract::NamespaceEntryKind::File)
    {
        let mut opened = engine.open_content(&entry.version_id).unwrap();
        let mut bytes = Vec::new();
        opened.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, fs::read(source.join(entry.path())).unwrap());
    }
}

#[test]
fn single_primary_initialization_and_import_recover_without_replacing_identities() {
    for point in ["after_accept_1", "after_finalize", "after_publication_1"] {
        for lose_activation_receipt_after_import in [false, true] {
            let (f, mut engine) = recovered_primary();
            let seal = witness(&f.service.disks["one"].join("disk.json"));
            let config = fs::read(f.service.data_root.join("config.json")).unwrap();
            let activation = fs::read(f.service.data_root.join(ACTIVATION_COMPLETE)).unwrap();
            let (source, request) = stage_import(&f, &mut engine);
            let original = tree(&source);
            let before = engine.workspace(&request.workspace_id).unwrap();
            let bodies = before
                .files
                .values()
                .map(|file| (file.source_path.clone(), witness(&file.source_path)))
                .collect::<BTreeMap<_, _>>();
            assert_eq!(bodies.len(), 3);
            let sequence = engine.state().sequence;
            engine.inject_fault_once(point);
            injected_indeterminate(
                &engine.commit_workspace(request.clone()).unwrap_err(),
                point,
            );
            assert!(engine.is_read_only());
            assert_eq!(engine.state().sequence, sequence);
            assert!(
                !engine
                    .state()
                    .operations
                    .contains_key(&request.operation_id)
            );
            native_requests(&engine, &request);
            assert_eq!(tree(&source), original);
            drop(engine);

            let mut engine = Engine::open(&f.service.data_root).unwrap();
            assert_eq!(engine.state().sequence, sequence + 1);
            assert!(
                engine.native_workspace_commit_completed(&request).unwrap(),
                "{point}"
            );
            let result = engine.operation(request.operation_id).unwrap();
            assert_eq!(result.kind, "workspace.commit");
            assert_eq!(
                engine.workspace(&request.workspace_id).unwrap().state,
                WorkspaceState::Committed
            );
            assert_eq!(
                engine
                    .workspace(&request.workspace_id)
                    .unwrap()
                    .fencing_token,
                before.fencing_token + 1
            );
            published_contents(&engine, &request, &source);
            native_requests(&engine, &request);
            let sequence = engine.state().sequence;
            assert_eq!(engine.commit_workspace(request.clone()).unwrap(), result);
            assert_eq!(engine.state().sequence, sequence);
            // This is a subsequent, independent lost activation projection:
            // the original import has already been recovered and verified.
            if lose_activation_receipt_after_import {
                drop(engine);
                fs::remove_file(f.service.data_root.join(ACTIVATION_COMPLETE)).unwrap();
                let cold = restarted(&f.service);
                assert_eq!(
                    cold.activation_admitted().unwrap_err().code,
                    ErrorCode::RecoveryRequired
                );
                let mut installed = None;
                cold.complete_activation(
                    &admin(),
                    |_| Ok(()),
                    |recovered| {
                        installed = Some(recovered);
                        Ok(())
                    },
                )
                .unwrap();
                cold.activation_admitted().unwrap();
                engine = installed.unwrap();
            } else {
                f.service
                    .complete_activation(
                        &admin(),
                        |_| Ok(()),
                        |_| panic!("completed activation must not reinstall an engine"),
                    )
                    .unwrap();
            }
            assert_eq!(engine.state().sequence, sequence);
            assert!(engine.native_workspace_commit_completed(&request).unwrap());
            assert_eq!(engine.operation(request.operation_id).unwrap(), result);
            assert_eq!(
                fs::read(f.service.data_root.join(ACTIVATION_COMPLETE)).unwrap(),
                activation
            );
            assert_eq!(
                fs::read(f.service.data_root.join("config.json")).unwrap(),
                config
            );
            assert_eq!(witness(&f.service.disks["one"].join("disk.json")), seal);
            assert_eq!(tree(&source), original);
            for (path, body) in &bodies {
                assert_eq!(witness(path), *body);
            }
            published_contents(&engine, &request, &source);
            assert!(
                fs::read_dir(&f.service.disks["untouched"])
                    .unwrap()
                    .next()
                    .is_none()
            );
        }
    }
}

#[test]
fn missing_import_requests_preserve_published_data_without_fabricating_completion() {
    let (f, mut engine) = recovered_primary();
    let (source, request) = stage_import(&f, &mut engine);
    let original = tree(&source);
    let before = engine.workspace(&request.workspace_id).unwrap();
    let primary = engine.config().disks[0].root.clone();
    let bodies = before
        .files
        .values()
        .map(|file| (file.source_path.clone(), witness(&file.source_path)))
        .collect::<BTreeMap<_, _>>();
    engine.inject_fault_once("after_finalize");
    injected_indeterminate(
        &engine.commit_workspace(request.clone()).unwrap_err(),
        "after_finalize",
    );
    native_requests(&engine, &request);
    drop(engine);
    let directories = [
        f.service
            .data_root
            .join("workspaces")
            .join(&request.workspace_id),
        primary
            .join("staging/workspaces")
            .join(&request.workspace_id),
    ];
    // This fixture simulates loss of both original request records, never user
    // payloads. A caller's request alone must not recreate missing authority.
    for directory in &directories {
        fs::remove_file(directory.join("commit-request.json")).unwrap();
        fs::File::open(directory).unwrap().sync_all().unwrap();
    }
    let mut engine = Engine::open(&f.service.data_root).unwrap();
    assert_eq!(
        engine.operation(request.operation_id).unwrap().kind,
        "workspace.commit"
    );
    let retained = engine.workspace(&request.workspace_id).unwrap();
    assert_eq!(retained.state, WorkspaceState::Open);
    assert_eq!(retained.fencing_token, before.fencing_token);
    assert!(!engine.native_workspace_commit_completed(&request).unwrap());
    published_contents(&engine, &request, &source);
    let state = json_codec::to_vec(engine.state()).unwrap();
    let staging = directories
        .each_ref()
        .map(|directory| tree(directory.parent().unwrap()));
    assert_eq!(
        engine.commit_workspace(request.clone()).unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(json_codec::to_vec(engine.state()).unwrap(), state);
    for (directory, expected) in directories.iter().zip(staging) {
        assert_eq!(tree(directory.parent().unwrap()), expected);
    }
    assert_eq!(tree(&source), original);
    for (path, body) in bodies {
        assert_eq!(witness(&path), body);
    }
    assert!(
        fs::read_dir(&f.service.disks["untouched"])
            .unwrap()
            .next()
            .is_none()
    );
}
