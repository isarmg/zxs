//! Public listener/Client tests for directory-repository rename publication.
use super::*;
use storage_contract::{
    directory_git::{DirectoryGitInitializeBody, DirectoryGitStageOwnedBody},
    file_relocation::{OBJECT_MOVE_KIND, ObjectMoveFact},
    history_window::HistoryWindowCancellation,
};

fn initialize(fixture: &Fixture, repo: &Repository, committed: bool) {
    let identity = fixture.identity();
    let snapshot = fixture
        .backend
        .tree_snapshot(
            &identity,
            ReadTree {
                branch: &repo.main_branch_id,
                commit: None,
            },
        )
        .unwrap();
    fixture
        .backend
        .initialize_directory_git(
            &identity,
            &repo.repository_id,
            DirectoryGitInitializeBody {
                path: "box".into(),
                expected_namespace_head: snapshot.commit_id,
                operation_id: OperationId::generate().unwrap(),
            },
        )
        .unwrap();
    let status = || {
        fixture
            .backend
            .directory_git_status(
                &identity,
                &repo.repository_id,
                "box",
                HistoryWindowCancellation::new(),
            )
            .unwrap()
    };
    let review = status();
    fixture
        .backend
        .stage_directory_git_owned(
            &identity,
            &repo.repository_id,
            DirectoryGitStageOwnedBody {
                directory_id: review.directory_id,
                directory_branch_id: review.directory_branch_id.unwrap(),
                expected_tree_id: review.working_tree_id.unwrap(),
                expected_index_generation: review.index_generation,
                operation_id: OperationId::generate().unwrap(),
            },
        )
        .unwrap();
    if committed {
        let review = status();
        fixture
            .backend
            .commit_directory_changes(
                &identity,
                &repo.repository_id,
                review.directory_branch_id.as_deref().unwrap(),
                storage_contract::directory_staging::DirectoryCommitBody {
                    expected_tree_id: review.working_tree_id.unwrap(),
                    expected_index_generation: review.index_generation,
                    message: "explicit directory history".into(),
                    operation_id: OperationId::generate().unwrap(),
                },
            )
            .unwrap();
    }
}

fn entries(fixture: &Fixture, repo: &Repository) -> Vec<Entry> {
    fixture
        .backend
        .tree_snapshot(
            &fixture.identity(),
            ReadTree {
                branch: &repo.main_branch_id,
                commit: None,
            },
        )
        .unwrap()
        .entries
}

#[test]
fn public_smb_directory_git_rename_staged_file_moves_directly_and_survives_restart() {
    for named in [false, true] {
        let mut fixture = Fixture::new();
        let client = fixture.connect();
        save(&client, "p/box/file", b"source", true);
        let repo = fixture.repository();
        initialize(&fixture, &repo, false);
        let client = if named {
            drop(client);
            Client::connect(
                &fixture.root.path().join("run/bridge.sock"),
                "alice",
                &repo.repository_id,
            )
            .unwrap()
        } else {
            client
        };
        let path = |relative: &str| {
            if named {
                relative.to_owned()
            } else {
                format!("p/{relative}")
            }
        };
        let source = fixture.entry(&repo.main_branch_id, "box/file");
        let (reader, _fd) = open(&client, &path("box/file"), false, false, false);
        for (source, target) in [("box/file", "outside"), ("outside", "box/returned")] {
            client
                .call(Request::Namespace {
                    kind: NamespaceOperation::Rename,
                    path: path(source),
                    destination: Some(path(target)),
                    replace: false,
                    operation_id: OperationId::generate().unwrap(),
                })
                .unwrap();
        }
        let moved = fixture.entry(&repo.main_branch_id, "box/returned");
        assert_eq!(moved.file_id, source.file_id);
        assert_eq!(moved.entry_id, source.entry_id);
        assert_eq!(moved.version_id, source.version_id);
        let mut old_bytes = [0; 6];
        assert_eq!(client.read(reader.handle, 0, &mut old_bytes).unwrap(), 6);
        assert_eq!(&old_bytes, b"source");
        close(&client, reader.handle);
        assert_eq!(read(&client, &path("box/returned")).1, b"source");
        drop(client);
        let (root, engine) = fixture.cold_open();
        assert_eq!(
            engine
                .namespace_tree(&repo.main_branch_id)
                .unwrap()
                .get("box/returned"),
            Some(&moved)
        );
        assert!(
            engine
                .namespace_tree(&repo.main_branch_id)
                .unwrap()
                .get("box/file")
                .is_none()
        );
        assert_eq!(
            exported(&engine, root.path(), &source.version_id),
            b"source"
        );
        assert_eq!(engine.file_history(&source.file_id).unwrap().len(), 1);
    }
}

#[test]
fn public_smb_directory_git_rename_committed_export_preserves_source_target_and_receipt_identity() {
    for (named, target_path, replace) in [
        (false, "missing", false),
        (false, "target", true),
        (true, "missing", false),
        (true, "target", true),
    ] {
        let mut fixture = Fixture::new();
        let client = fixture.connect();
        save(&client, "p/box/file", b"first source", true);
        let repo = fixture.repository();
        let first = fixture.entry(&repo.main_branch_id, "box/file");
        save(&client, "p/box/file", b"source", false);
        save(&client, "p/target", b"target", true);
        initialize(&fixture, &repo, true);
        let client = if named {
            drop(client);
            Client::connect(
                &fixture.root.path().join("run/bridge.sock"),
                "alice",
                &repo.repository_id,
            )
            .unwrap()
        } else {
            client
        };
        let path = |relative: &str| {
            if named {
                relative.to_owned()
            } else {
                format!("p/{relative}")
            }
        };
        let source = fixture.entry(&repo.main_branch_id, "box/file");
        let target = fixture.entry(&repo.main_branch_id, "target");
        let before = entries(&fixture, &repo);
        assert_eq!(source.file_id, first.file_id);
        let status = || {
            fixture
                .backend
                .directory_git_status(
                    &fixture.identity(),
                    &repo.repository_id,
                    "box",
                    HistoryWindowCancellation::new(),
                )
                .unwrap()
        };
        let committed = status();
        let head = committed.head_tree_id.clone().unwrap();
        let (reader, _fd) = open(&client, &path("box/file"), false, false, false);
        let operation_id = OperationId::generate().unwrap();
        let request = || Request::Namespace {
            kind: NamespaceOperation::Rename,
            path: path("box/file"),
            destination: Some(path(target_path)),
            replace,
            operation_id,
        };
        let reply = client.call(request()).unwrap();
        assert_eq!(reply["operation_id"], operation_id.to_string());
        assert_eq!(reply["operation_kind"], OBJECT_MOVE_KIND);
        assert_eq!(reply["commit_state"], "PUBLISHED");
        assert_eq!(reply["protection_at_publish"], "TARGET_MET");
        let fact: ObjectMoveFact =
            json_codec::from_value(reply["result"]["movement"].clone()).unwrap();
        assert_eq!(fact.principal, "alice");
        assert_eq!(
            fact.request.source_repository_id.as_str(),
            repo.repository_id
        );
        assert_eq!(fact.request.source_entry_id.as_str(), source.entry_id);
        assert_eq!(fact.request.source_path.as_str(), "box/file");
        assert_eq!(
            fact.source_file_id.as_ref().unwrap().as_str(),
            source.file_id
        );
        assert_eq!(fact.request.target_path.as_str(), target_path);
        assert_eq!(fact.request.target_slot_id, repo.primary_slot_id);
        assert_eq!(
            fact.request.target_repository_id.as_ref().unwrap().as_str(),
            repo.repository_id
        );
        assert_eq!(fact.request.replace, replace);
        assert_eq!(
            fact.request.target_entry_id.as_ref().map(|id| id.as_str()),
            replace.then_some(target.entry_id.as_str())
        );
        assert_eq!(
            fact.displaced_target_file_id.as_ref().map(|id| id.as_str()),
            replace.then_some(target.file_id.as_str())
        );
        assert_eq!(fact.target_repository_id.as_str(), repo.repository_id);
        assert_eq!(
            reply["request_digest"],
            fact.request.digest("alice").unwrap()
        );
        let moved = fixture.entry(&repo.main_branch_id, target_path);
        assert_eq!(moved.entry_id, fact.target_entry_id.as_str());
        assert_ne!(moved.entry_id, source.entry_id);
        assert_ne!(moved.file_id, source.file_id);
        assert_ne!(moved.version_id, source.version_id);
        let mapping = reply["result"]["version_mapping"].as_object().unwrap();
        assert_eq!(mapping.len(), 2);
        assert_eq!(mapping[&source.version_id], moved.version_id);
        assert!(
            reply["result"]["source_deletions"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(
            !entries(&fixture, &repo)
                .iter()
                .any(|entry| entry.path() == "box/file")
        );
        let working = status();
        assert_eq!(working.head_tree_id, committed.head_tree_id);
        assert_eq!(working.index_generation, committed.index_generation);
        assert!(working.unstaged);
        assert_eq!(read(&client, &path(target_path)).1, b"source");
        if !replace {
            assert_eq!(fixture.entry(&repo.main_branch_id, "target"), target);
            assert_eq!(read(&client, &path("target")).1, b"target");
        }
        let mut pinned = [0; 6];
        assert_eq!(
            client.read(reader.handle, 0, &mut pinned).unwrap(),
            pinned.len()
        );
        assert_eq!(&pinned, b"source");
        close(&client, reader.handle);
        let receipt = fixture
            .backend
            .operation_outcome(&fixture.identity(), operation_id)
            .unwrap();
        assert_eq!(receipt.result, reply["result"]);
        assert_eq!(
            receipt.request_digest,
            fact.request.digest("alice").unwrap()
        );
        let spool = fixture
            .root
            .path()
            .join("management/smb-spool")
            .join(format!("{operation_id}.namespace.json"));
        assert!(!spool.exists());
        let after = entries(&fixture, &repo);
        let mut expected = before
            .into_iter()
            .filter(|entry| entry.path() != "box/file" && (!replace || entry.path() != "target"))
            .collect::<Vec<_>>();
        expected.push(moved.clone());
        expected.sort_by(|left, right| left.lookup_key().cmp(right.lookup_key()));
        assert_eq!(after, expected);
        drop(client);
        let client = if named {
            Client::connect(
                &fixture.root.path().join("run/bridge.sock"),
                "alice",
                &repo.repository_id,
            )
            .unwrap()
        } else {
            fixture.connect()
        };
        let error = client.call(request()).unwrap_err();
        let remote = error
            .get_ref()
            .unwrap()
            .downcast_ref::<RemoteError>()
            .unwrap();
        assert_eq!(
            remote.code,
            RemoteErrorCode::Server(ErrorCode::OperationIdConflict)
        );
        assert_eq!(remote.operation_id, Some(operation_id.to_string()));
        assert!(!remote.retryable);
        assert_eq!(entries(&fixture, &repo), after);
        assert_eq!(
            fixture
                .backend
                .operation_outcome(&fixture.identity(), operation_id)
                .unwrap(),
            receipt
        );
        assert!(!spool.exists());
        drop(client);
        let (root, engine) = fixture.cold_open();
        assert_eq!(
            engine
                .namespace_tree(&repo.main_branch_id)
                .unwrap()
                .get(target_path),
            Some(&moved)
        );
        assert!(
            engine
                .namespace_tree(&repo.main_branch_id)
                .unwrap()
                .get("box/file")
                .is_none()
        );
        let current = engine.namespace_tree(&repo.main_branch_id).unwrap();
        assert_eq!(current.visible_entries().count(), expected.len());
        for entry in &expected {
            assert_eq!(current.get(entry.path()), Some(entry));
        }
        let frozen = &engine.state().directory_trees[&head];
        assert_eq!(
            frozen
                .entries
                .iter()
                .find(|entry| entry.path() == "box/file"),
            Some(&source)
        );
        assert!(!engine.state().file_deletions.contains_key(&source.file_id));
        let history = engine.file_history(&moved.file_id).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(engine.file_history(&source.file_id).unwrap().len(), 2);
        for (original, body) in [
            (&first, b"first source".as_slice()),
            (&source, b"source".as_slice()),
        ] {
            let copied = mapping[&original.version_id].as_str().unwrap();
            assert!(history.iter().any(|version| version.version_id == copied));
            assert_eq!(exported(&engine, root.path(), &original.version_id), body);
            assert_eq!(exported(&engine, root.path(), copied), body);
        }
        assert_eq!(
            exported(&engine, root.path(), &target.version_id),
            b"target"
        );
        assert_eq!(engine.file_history(&target.file_id).unwrap().len(), 1);
        if replace {
            let deletion = &engine.state().file_deletions[&target.file_id];
            assert_eq!(deletion.operation_id, operation_id);
            assert_eq!(deletion.path.as_str(), "target");
            assert_eq!(
                reply["result"]["displaced_deletions"],
                json_codec::json!([deletion])
            );
        } else {
            assert_eq!(
                engine
                    .namespace_tree(&repo.main_branch_id)
                    .unwrap()
                    .get("target"),
                Some(&target)
            );
            assert!(!engine.state().file_deletions.contains_key(&target.file_id));
            assert!(
                reply["result"]["displaced_deletions"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }
        assert_eq!(
            storage_contract::OperationOutcome::from(engine.operation(operation_id).unwrap()),
            receipt
        );
        assert_eq!(
            engine
                .state()
                .events
                .iter()
                .filter(|event| event.operation_id == operation_id)
                .count(),
            1
        );
    }
}
