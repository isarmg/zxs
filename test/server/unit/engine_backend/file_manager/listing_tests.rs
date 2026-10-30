//! Compact-summary projection only. Synthetic in-memory metadata is not a real
//! file import, and branch count is fixed at one: unlimited branches remain a
//! separate, unpaginated response dimension.
use super::*;
use storage_contract::{DirectoryBranch, Entry};

#[test]
fn one_branch_summary_does_not_serialize_its_growing_entry_graph() {
    let mut baseline_bytes = None;
    for size in [1_000, 10_000, 100_000] {
        let branch = DirectoryBranch {
            directory_branch_id: "branch-fixture".into(),
            repository_id: "repo-fixture".into(),
            directory_id: "directory-fixture".into(),
            path: "docs".into(),
            name: "main".into(),
            parent_branch_id: None,
            head_tree_id: "tree-fixture".into(),
            commit_id: "commit-fixture".into(),
            entries: (0..size)
                .map(|index| {
                    let name = format!("file-{index:06}.txt");
                    let path = format!("docs/{name}");
                    Entry {
                        entry_id: format!("entry-{index:06}"),
                        path: name_policy::PortablePath::parse(&path).unwrap(),
                        kind: storage_contract::NamespaceEntryKind::File,
                        file_id: format!("file-{index:06}"),
                        version_id: "version-fixture".into(),
                        size: 1024,
                    }
                })
                .collect(),
            file_branches: [("file-fixture".into(), "file-branch-fixture".into())].into(),
            file_contexts: Default::default(),
            inherited_files: [("file-fixture".into(), "source-fixture".into())].into(),
            directory_refs: Default::default(),
            materialized: true,
            sequence: 42,
        };
        let summary = json_codec::to_value(
            crate::backend_directory_scopes::DirectoryChoiceRecord::from(
                crate::DirectoryBranchChoice::capture(&branch, true),
            ),
        )
        .unwrap();
        assert_eq!(summary["directory_branch_id"], "branch-fixture");
        assert_eq!(summary["file_count"], size.to_string());
        assert_eq!(summary["current"], true);
        assert_eq!(summary["sequence"], "42");
        for omitted in [
            "entries",
            "file_contexts",
            "file_branches",
            "inherited_files",
            "directory_refs",
        ] {
            assert!(summary.get(omitted).is_none(), "summary leaked {omitted}");
        }
        let bytes = json_codec::to_vec(&json!({"directory_branches":[summary]}))
            .unwrap()
            .len();
        let adjusted = bytes - size.to_string().len();
        assert_eq!(
            *baseline_bytes.get_or_insert(adjusted),
            adjusted,
            "fixed-branch summary size may grow only by file-count digits"
        );
        println!(
            "synthetic_branch_entries={size}; branch_count=1; summary_response_bytes={bytes}; physical_files=0"
        );
    }
}

fn listing_fixture() -> (
    tempfile::TempDir,
    std::sync::Arc<crate::engine_backend::EngineBackend>,
    Repository,
    Identity,
) {
    listing_fixture_with_copies(1)
}

fn listing_fixture_with_copies(
    copies: u32,
) -> (
    tempfile::TempDir,
    std::sync::Arc<crate::engine_backend::EngineBackend>,
    Repository,
    Identity,
) {
    let root = crate::tests::private_tempdir();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        storage_engine::Config::development(
            (1..=copies).map(|index| root.path().join(format!("d{index}"))),
        ),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Files").unwrap(),
            "P01",
            copies,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    save_listing_files(&mut engine, &repo, &["alpha.txt", "beta.txt"]);
    let identity = crate::engine_backend::install_test_administrator(&mut engine, "owner");
    (
        root,
        std::sync::Arc::new(crate::engine_backend::EngineBackend::new(engine)),
        repo,
        identity,
    )
}

fn save_listing_files(engine: &mut Engine, repo: &Repository, names: &[&str]) {
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    for name in names {
        engine
            .stage_bytes(&workspace.workspace_id, name, b"body")
            .unwrap();
    }
    engine
        .commit_workspace(storage_engine::CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "listing fixture".into(),
            request_digest: None,
        })
        .unwrap();
}

fn request(cursor: Option<String>) -> crate::ReadListing {
    let mut query = BTreeMap::from([
        ("path".into(), "Files".into()),
        ("limit".into(), "1".into()),
    ]);
    if let Some(cursor) = cursor {
        query.insert("cursor".into(), cursor);
    }
    crate::ReadListing::parse("P01".into(), query).unwrap()
}

fn listing_wire(snapshot: crate::ListingSnapshot) -> Value {
    json_codec::to_value(crate::backend_listing::ListingResponse::from(snapshot)).unwrap()
}

#[test]
fn detached_primary_read_finishes_with_writer_locked_and_keeps_original_generation() {
    let (_root, backend, repo, identity) = listing_fixture();
    let control = crate::tests::history_read_control();
    let expected = {
        let engine = backend.engine.lock().unwrap();
        listing_wire(
            capture(&engine, request(None), &control)
                .unwrap()
                .read(&control)
                .unwrap(),
        )
    };
    let plan = capture(&backend.engine.lock().unwrap(), request(None), &control).unwrap();
    let file = expected["entries"][0]["file_id"].as_str().unwrap();
    let mut writer = backend.engine.lock().unwrap();
    writer
        .set_file_note(
            file,
            None,
            &storage_contract::Note::parse("new note".into()).unwrap(),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    save_listing_files(&mut writer, &repo, &["new.txt"]);
    // The actual detached page reader runs while the writer guard is held.
    // A channel timeout diagnoses an accidental reacquisition, without sleeps.
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || send.send(plan.read(&control)).unwrap());
    let result = receive.recv_timeout(std::time::Duration::from_secs(30));
    drop(writer);
    worker.join().unwrap();
    let result = result.expect("detached listing waited for writer mutex");
    let snapshot = backend
        .finish_listing(&identity, result, &crate::tests::history_read_control())
        .unwrap();
    assert_eq!(listing_wire(snapshot), expected);
    let cursor = expected["next_cursor"].as_str().unwrap().to_owned();
    assert_eq!(
        capture(
            &backend.engine.lock().unwrap(),
            request(Some(cursor)),
            &crate::tests::history_read_control(),
        )
        .err()
        .unwrap()
        .code,
        storage_contract::ErrorCode::ListingChanged
    );
}

#[test]
fn listing_completion_reauthorizes_before_success_read_failure_and_expiry() {
    for managed in [false, true] {
        let (root, backend, _repo, _) = listing_fixture();
        let auth =
            crate::auth::AuthStore::initialize(root.path(), "owner", "integration-test-password")
                .unwrap();
        let mut auth = if managed {
            backend.set_management_auth(auth.reader()).unwrap();
            auth
        } else {
            backend.protect_auth(auth).unwrap()
        };
        let token = auth
            .login("owner", "integration-test-password")
            .unwrap()
            .token;
        let identity = auth.authenticate(&token).unwrap();
        let control = crate::tests::history_read_control();
        let result = capture(&backend.engine.lock().unwrap(), request(None), &control)
            .unwrap()
            .read(&control);
        let failure = || {
            ApiError::new(
                ::http::StatusCode::INTERNAL_SERVER_ERROR,
                storage_contract::ErrorCode::ObjectCorrupt,
                "selected namespace is corrupt",
            )
        };
        let error = backend
            .finish_listing(&identity, Err(failure()), &control)
            .err()
            .unwrap();
        assert_eq!(error.code, storage_contract::ErrorCode::ObjectCorrupt);
        assert_eq!(error.message, "selected namespace is corrupt");
        auth.logout(&token).unwrap();
        for result in [result, Err(failure())] {
            assert_eq!(
                backend
                    .finish_listing(&identity, result, &control)
                    .err()
                    .unwrap()
                    .code,
                storage_contract::ErrorCode::Unauthorized
            );
        }
        let expired = crate::HistoryReadControl::until(std::time::Instant::now());
        assert_eq!(
            backend
                .finish_listing(&identity, Err(failure()), &expired)
                .err()
                .unwrap()
                .code,
            storage_contract::ErrorCode::Unauthorized
        );
    }
}

#[test]
fn detached_primary_cancellation_and_deadline_precede_namespace_enumeration() {
    let (_root, backend, _repo, _) = listing_fixture();
    let control = crate::tests::history_read_control();
    let plan = capture(&backend.engine.lock().unwrap(), request(None), &control).unwrap();
    drop(control.wait_owner());
    assert_eq!(
        plan.read(&control).err().unwrap().code,
        storage_contract::ErrorCode::RequestTimeout
    );
    let control = crate::tests::history_read_control();
    let plan = capture(&backend.engine.lock().unwrap(), request(None), &control).unwrap();
    let expired = crate::HistoryReadControl::until(std::time::Instant::now());
    assert_eq!(
        plan.read(&expired).err().unwrap().code,
        storage_contract::ErrorCode::RequestTimeout
    );
}

#[test]
fn listing_encoding_uses_existing_response_cap_without_truncating_a_page() {
    let (_root, backend, _repo, _) = listing_fixture();
    let control = crate::tests::history_read_control();
    let mut snapshot = capture(&backend.engine.lock().unwrap(), request(None), &control)
        .unwrap()
        .read(&control)
        .unwrap();
    snapshot.entries[0].note =
        Some("x".repeat(storage_contract::client_protocol::MAX_RESPONSE_BYTES));
    let error = crate::backend_file_history::encode_response_bounded(
        crate::backend_listing::ListingResponse::from(snapshot),
    )
    .err()
    .unwrap();
    assert_eq!(
        error.code,
        storage_contract::ErrorCode::ResourceLimitExceeded
    );
}

fn backup_request(path: &str, query: &str, cursor: Option<String>) -> crate::ReadListing {
    let mut values = BTreeMap::from([
        ("path".into(), path.into()),
        ("limit".into(), "1".into()),
        ("query".into(), query.into()),
    ]);
    if let Some(cursor) = cursor {
        values.insert("cursor".into(), cursor);
    }
    crate::ReadListing::parse("B01".into(), values).unwrap()
}

#[test]
fn detached_backup_read_keeps_publication_while_writer_mutates() {
    let (_root, backend, repo, identity) = listing_fixture_with_copies(2);
    let control = crate::tests::history_read_control();
    let request = || backup_request("P01/Files", "", None);
    let expected = listing_wire(
        capture(&backend.engine.lock().unwrap(), request(), &control)
            .unwrap()
            .read(&control)
            .unwrap(),
    );
    let plan = capture(&backend.engine.lock().unwrap(), request(), &control).unwrap();
    let mut writer = backend.engine.lock().unwrap();
    writer
        .set_file_note(
            expected["entries"][0]["file_id"].as_str().unwrap(),
            None,
            &storage_contract::Note::parse("later note".into()).unwrap(),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    save_listing_files(&mut writer, &repo, &["new.txt"]);
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || send.send(plan.read(&control)).unwrap());
    let result = receive.recv_timeout(std::time::Duration::from_secs(30));
    drop(writer);
    worker.join().unwrap();
    let snapshot = backend
        .finish_listing(
            &identity,
            result.expect("backup projection waited for writer mutex"),
            &crate::tests::history_read_control(),
        )
        .unwrap();
    assert_eq!(listing_wire(snapshot), expected);
    assert_eq!(
        capture(
            &backend.engine.lock().unwrap(),
            backup_request(
                "P01/Files",
                "",
                Some(expected["next_cursor"].as_str().unwrap().into())
            ),
            &crate::tests::history_read_control()
        )
        .err()
        .unwrap()
        .code,
        storage_contract::ErrorCode::ListingChanged
    );
}

#[test]
fn detached_backup_validates_global_conflicts_before_path_and_query() {
    let (_root, backend, _repo, _identity) = listing_fixture_with_copies(2);
    let control = crate::tests::history_read_control();
    let mut engine = backend.engine.lock().unwrap();
    // A synthetic cross-file logical collision exercises the read boundary;
    // fixture replacement is not a supported production mutation.
    let mut state = engine.state().clone();
    let files = state
        .versions
        .values()
        .map(|version| version.file_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 2);
    for (file, path) in files
        .iter()
        .zip(["elsewhere/collision", "elsewhere/COLLISION"])
    {
        state
            .file_backup_paths
            .insert(file.clone(), BTreeMap::from([("B01".into(), path.into())]));
    }
    storage_engine::test_support::replace_state(&mut engine, state);
    for path in ["", "P01/Files", "missing"] {
        let plan = capture(
            &engine,
            backup_request(path, "no-matching-row", None),
            &control,
        )
        .unwrap();
        assert_eq!(
            plan.read(&control).err().unwrap().code,
            storage_contract::ErrorCode::BackupPathConflict
        );
    }
}

#[test]
fn detached_backup_empty_candidates_still_read_corrupt_namespace_objects() {
    let (root, backend, _repo, _identity) = listing_fixture_with_copies(2);
    let control = crate::tests::history_read_control();
    let mut engine = backend.engine.lock().unwrap();
    let empty = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Empty").unwrap(),
            "P01",
            1,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let namespace = storage_engine::test_support::plant_namespace(
        &engine,
        &[storage_contract::Entry {
            entry_id: storage_engine::new_id("entry"),
            path: name_policy::PortablePath::parse("uncached-directory").unwrap(),
            kind: storage_contract::NamespaceEntryKind::Directory,
            file_id: String::new(),
            version_id: String::new(),
            size: 0,
        }],
    )
    .unwrap();
    let reference = namespace.root.clone();
    let mut state = engine.state().clone();
    let head = state.branches[&empty.main_branch_id].head_commit_id.clone();
    state.commits.get_mut(&head).unwrap().namespace = namespace;
    // No version has B01 placement; the global namespace walk must still happen.
    for placement in state.placements.values_mut() {
        placement.route_slot_ids.retain(|slot| slot != "B01");
    }
    storage_engine::test_support::replace_state(&mut engine, state);
    let plan = capture(
        &engine,
        backup_request("missing", "no-match", None),
        &control,
    )
    .unwrap();
    // The planted namespace was never read into SegmentCaches. Corrupt every
    // actual metadata replica after capture, without touching any real pool.
    for disk in engine
        .config()
        .disks
        .iter()
        .filter(|disk| disk.metadata_member)
    {
        let object = disk
            .root
            .join("metadata/objects")
            .join(reference.kind.as_str())
            .join(&reference.hash[..2])
            .join(format!("{}.json", reference.hash));
        assert!(object.starts_with(root.path()));
        std::fs::write(object, b"{}").unwrap();
    }
    drop(engine);
    let failure = plan.read(&control).err().unwrap();
    assert_eq!(failure.code, storage_contract::ErrorCode::ObjectCorrupt);
    assert!(std::error::Error::source(&failure).is_some());
}

#[test]
fn detached_backup_cancellation_precedes_global_namespace_work() {
    let (_root, backend, _repo, _identity) = listing_fixture_with_copies(2);
    let control = crate::tests::history_read_control();
    let plan = capture(
        &backend.engine.lock().unwrap(),
        backup_request("", "", None),
        &control,
    )
    .unwrap();
    drop(control.wait_owner());
    assert_eq!(
        plan.read(&control).err().unwrap().code,
        storage_contract::ErrorCode::RequestTimeout
    );
}

#[test]
fn detached_backup_node_limit_includes_global_implicit_parents() {
    let (_root, backend, _repo, _identity) = listing_fixture_with_copies(2);
    let control = crate::tests::history_read_control();
    let engine = backend.engine.lock().unwrap();
    // Exercise the captured limit independently of the much larger fixture
    // initialization budget. P01 + Files + two files require four nodes.
    for maximum in [2, 3, 4] {
        let request = PageRequest::parse(&engine, backup_request("", "no-match", None)).unwrap();
        let plan = backup_namespace::BackupListing::capture(
            engine.published_read_view(),
            request,
            "B01".into(),
            String::new(),
            maximum,
        );
        let result = plan.read(&control);
        if maximum < 4 {
            assert_eq!(
                result.err().unwrap().code,
                storage_contract::ErrorCode::ResourceLimitExceeded
            );
        } else {
            assert!(result.unwrap().entries.is_empty());
        }
    }
}
