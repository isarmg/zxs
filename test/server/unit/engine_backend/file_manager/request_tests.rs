//! Malformed requests must be rejected before lazy disk-root publication.
use super::*;
use crate::BackendAction;
use std::path::{Path, PathBuf};

fn disk_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                visit(&entry.path(), files);
            } else {
                files.insert(entry.path(), std::fs::read(entry.path()).unwrap());
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, &mut files);
    files
}

#[test]
fn explicit_non_string_paths_never_mean_the_root() {
    for value in [Value::Null, json!(123), json!([]), json!({}), json!(false)] {
        assert!(path(&json!({"path":value})).is_err(), "{value}");
        assert!(path(&json!({"query":{"path":value}})).is_err(), "{value}");
        assert!(path(&json!({"path":value,"query":{"path":"valid"}})).is_err());
    }
    assert_eq!(path(&json!({})).unwrap(), "");
    assert_eq!(path(&json!({"path":""})).unwrap(), "");
    assert_eq!(path(&json!({"query":{"path":"folder"}})).unwrap(), "folder");
}

#[test]
fn malformed_lazy_root_requests_do_not_publish_any_state() {
    use BackendAction::*;
    let op = storage_engine::new_id("op");
    let cases = [
        (
            FileManagerDirectoryBranches,
            json!({"slot_id":"P01","path":"","operation_id":op}),
        ),
        (
            FileManagerDirectoryBranches,
            json!({"slot_id":"P01","path":"","name":"branch","operation_id":"bad"}),
        ),
        (
            FileManagerDirectoryBranches,
            json!({"slot_id":"P01","path":"","name":"x".repeat(129),"operation_id":op}),
        ),
        (
            FileManagerDirectorySelect,
            json!({"slot_id":"P01","path":"","name":null,"operation_id":op}),
        ),
        (
            FileManagerWriteSession,
            json!({"slot_id":"P01","path":"file.txt","branch_id":123}),
        ),
        (
            FileManagerNamespace,
            json!({"slot_id":"P01","path":"folder","kind":"unknown","operation_id":op}),
        ),
        (
            FileManagerNamespace,
            json!({"slot_id":"P01","path":"folder","kind":"mkdir"}),
        ),
        (
            FileManagerNamespace,
            json!({"slot_id":"P01","path":"folder","kind":"mkdir","operation_id":"bad"}),
        ),
        (
            FileManagerNamespace,
            json!({"slot_id":"P01","path":"folder","kind":"rename","destination":123,"operation_id":op}),
        ),
        (
            FileManagerNamespace,
            json!({"slot_id":"P01","path":"folder","kind":"rename","operation_id":op}),
        ),
        (
            FileManagerNamespace,
            json!({"slot_id":"P01","path":"folder","kind":"rename","destination":"../escape","operation_id":op}),
        ),
    ];
    for (action, input) in cases {
        let root = tempfile::tempdir().unwrap();
        let disks = root.path().join("disks");
        let config = storage_engine::Config::development([
            disks.join("one"),
            disks.join("two"),
            disks.join("three"),
        ]);
        let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
        let identity = Identity {
            admission_session: None,
            smb_binding: None,
            principal: "owner".into(),
        };
        let before = json_codec::to_value(engine.state()).unwrap();
        let bytes = disk_files(&disks);
        let failure = call(&mut engine, &identity, action, input.clone()).unwrap_err();
        assert_eq!(failure.status, 400, "{input}: {failure:?}");
        assert_eq!(
            json_codec::to_value(engine.state()).unwrap(),
            before,
            "{input}"
        );
        assert_eq!(disk_files(&disks), bytes, "{input}");
    }
}

#[test]
fn destructive_path_only_requests_never_mutate_or_create_a_disk_root() {
    let root = tempfile::tempdir().unwrap();
    let disks = root.path().join("disks");
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        storage_engine::Config::development([
            disks.join("one"),
            disks.join("two"),
            disks.join("three"),
        ]),
    )
    .unwrap();
    let identity = Identity {
        admission_session: None,
        smb_binding: None,
        principal: "owner".into(),
    };
    for kind in ["delete", "delete_tree", "rename"] {
        let mut input = json!({"slot_id":"P01","kind":kind,"path":"missing","operation_id":storage_engine::new_id("op")});
        if kind == "rename" {
            input["destination"] = json!("destination");
        }
        assert_eq!(
            call(
                &mut engine,
                &identity,
                BackendAction::FileManagerNamespace,
                input
            )
            .unwrap_err()
            .status,
            400
        );
        assert!(engine.state().repositories.is_empty());
        assert_eq!(engine.state().sequence, 0);
    }
    let repo = engine
        .create_repository("Existing", "P01", 3, "owner", &storage_engine::new_id("op"))
        .unwrap();
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation {
                kind: "mkdir".into(),
                path: "folder".into(),
                destination: None,
            },
            "owner",
            &storage_engine::new_id("op"),
        )
        .unwrap();
    let before = json_codec::to_value(engine.state()).unwrap();
    let bytes = disk_files(&disks);
    for kind in ["rename", "delete", "delete_tree"] {
        let mut request = json!({"slot_id":"P01","kind":kind,"path":"Existing/folder","operation_id":storage_engine::new_id("op")});
        if kind == "rename" {
            request["destination"] = json!("new-root-folder");
        }
        assert_eq!(
            call(
                &mut engine,
                &identity,
                BackendAction::FileManagerNamespace,
                request
            )
            .unwrap_err()
            .status,
            400
        );
        assert_eq!(json_codec::to_value(engine.state()).unwrap(), before);
        assert_eq!(disk_files(&disks), bytes);
    }
}

#[test]
fn valid_creation_and_upload_still_create_the_lazy_root() {
    for action in [
        BackendAction::FileManagerNamespace,
        BackendAction::FileManagerDirectoryBranches,
        BackendAction::FileManagerWriteSession,
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut engine = Engine::initialize(
            &root.path().join("system"),
            storage_engine::Config::development([
                root.path().join("one"),
                root.path().join("two"),
                root.path().join("three"),
            ]),
        )
        .unwrap();
        let identity = Identity {
            admission_session: None,
            smb_binding: None,
            principal: "owner".into(),
        };
        let input = match action {
            BackendAction::FileManagerNamespace => {
                json!({"slot_id":"P01","kind":"mkdir","path":"folder","operation_id":storage_engine::new_id("op")})
            }
            BackendAction::FileManagerDirectoryBranches => {
                json!({"slot_id":"P01","path":"","name":"branch","operation_id":storage_engine::new_id("op")})
            }
            _ => json!({"slot_id":"P01","path":"file.txt"}),
        };
        call(&mut engine, &identity, action, input).unwrap();
        assert_eq!(engine.state().repositories.len(), 1);
        assert!(
            engine
                .state()
                .repositories
                .values()
                .next()
                .unwrap()
                .file_manager_root
        );
    }
}

#[test]
fn baseline_name_and_used_operation_are_rejected_before_root_initialization() {
    let root = tempfile::tempdir().unwrap();
    let disks = root.path().join("disks");
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        storage_engine::Config::development([
            disks.join("one"),
            disks.join("two"),
            disks.join("three"),
        ]),
    )
    .unwrap();
    let identity = Identity {
        admission_session: None,
        smb_binding: None,
        principal: "owner".into(),
    };
    for name in ["main", "MAIN"] {
        let before = disk_files(&disks);
        let error = call(&mut engine, &identity, BackendAction::FileManagerDirectoryBranches, json!({"slot_id":"P01","path":"","name":name,"operation_id":storage_engine::new_id("op")})).unwrap_err();
        assert_eq!(error.code, "NAME_COLLISION");
        assert_eq!(engine.state().sequence, 0);
        assert_eq!(disk_files(&disks), before);
    }
    let used = storage_engine::new_id("op");
    engine
        .create_repository("Existing", "P01", 3, "owner", &used)
        .unwrap();
    let before = json_codec::to_value(engine.state()).unwrap();
    let bytes = disk_files(&disks);
    let error = call(
        &mut engine,
        &identity,
        BackendAction::FileManagerNamespace,
        json!({"slot_id":"P01","path":"fresh","kind":"mkdir","operation_id":used}),
    )
    .unwrap_err();
    assert_eq!(error.code, "IDEMPOTENCY_KEY_REUSED");
    assert_eq!(json_codec::to_value(engine.state()).unwrap(), before);
    assert_eq!(disk_files(&disks), bytes);
}

#[test]
fn destination_scope_reads_disambiguate_reused_directory_paths_by_identity() {
    let root = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        storage_engine::Config::development([
            root.path().join("one"),
            root.path().join("two"),
            root.path().join("three"),
        ]),
    )
    .unwrap();
    let identity = Identity {
        admission_session: None,
        smb_binding: None,
        principal: "owner".into(),
    };
    let mut mutate = |action, mut input: Value| {
        input["slot_id"] = "P01".into();
        input["operation_id"] = storage_engine::new_id("op").into();
        call(&mut engine, &identity, action, input).unwrap()
    };
    mutate(
        BackendAction::FileManagerNamespace,
        json!({"kind":"mkdir","path":"target"}),
    );
    let frozen = mutate(
        BackendAction::FileManagerDirectoryBranches,
        json!({"path":"target","name":"frozen"}),
    );
    let old_branch = frozen["result"]["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let repo = engine
        .state()
        .repositories
        .values()
        .next()
        .unwrap()
        .repository_id
        .clone();
    let root_scopes = directory_branch_scopes(&engine, &repo, Some("")).unwrap();
    let root_scope = root_scopes["directory_branches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|branch| branch["current"] == true)
        .unwrap();
    let target = root_scope["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path"] == "target")
        .unwrap();
    let old_directory = target["entry_id"].clone();
    let rename = json!({"slot_id":"P01","operation_id":storage_engine::new_id("op"),"kind":"rename","items":[{
        "source":{"directory_branch_id":root_scope["directory_branch_id"],"expected_tree_id":root_scope["head_tree_id"],"path":"target","entry_id":target["entry_id"]},
        "target":{"directory_branch_id":root_scope["directory_branch_id"],"expected_tree_id":root_scope["head_tree_id"],"path":"archive"}
    }]});
    call(
        &mut engine,
        &identity,
        BackendAction::FileManagerNamespaceBatch,
        rename,
    )
    .unwrap();
    call(&mut engine, &identity, BackendAction::FileManagerNamespace, json!({"slot_id":"P01","operation_id":storage_engine::new_id("op"),"kind":"mkdir","path":"target"})).unwrap();
    let fresh = call(&mut engine, &identity, BackendAction::FileManagerDirectoryBranches, json!({"slot_id":"P01","operation_id":storage_engine::new_id("op"),"path":"target","name":"fresh"})).unwrap();
    let fresh_branch = fresh["result"]["directory_branch_id"].as_str().unwrap();
    let all = directory_branch_scopes(&engine, &repo, None).unwrap();
    let cached_matches = all["directory_branches"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|branch| branch["path"] == "target")
        .collect::<Vec<_>>();
    assert!(
        cached_matches
            .iter()
            .any(|branch| branch["directory_branch_id"] == old_branch)
    );
    assert!(
        cached_matches
            .iter()
            .any(|branch| branch["directory_branch_id"] == fresh_branch)
    );
    let before = engine.state().sequence;
    for path in ["target", "TARGET"] {
        let scoped = directory_branch_scopes(&engine, &repo, Some(path)).unwrap();
        let branches = scoped["directory_branches"].as_array().unwrap();
        assert!(
            branches
                .iter()
                .any(|branch| branch["directory_branch_id"] == fresh_branch)
        );
        assert!(
            branches
                .iter()
                .all(|branch| branch["directory_id"] != old_directory)
        );
        assert!(
            branches
                .iter()
                .all(|branch| branch["directory_branch_id"] != old_branch)
        );
    }
    assert_eq!(
        engine.state().sequence,
        before,
        "scope lookup must not mutate the SMB selection"
    );
}

#[test]
fn ambiguous_primary_disk_paths_never_choose_a_repository_or_create_work() {
    for named_first in [false, true] {
        for (named_name, root_name, root_is_file) in [
            ("Docs", "Docs", false),
            ("STRASSE", "Straße", false),
            ("Docs", "Docs", true),
            ("STRASSE", "Straße", true),
        ] {
            let temporary = tempfile::tempdir().unwrap();
            let disks = temporary.path().join("disks");
            let mut engine = Engine::initialize(
                &temporary.path().join("system"),
                storage_engine::Config::development([
                    disks.join("one"),
                    disks.join("two"),
                    disks.join("three"),
                ]),
            )
            .unwrap();
            let create_named = |engine: &mut Engine| {
                engine
                    .create_repository(named_name, "P01", 1, "owner", &storage_engine::new_id("op"))
                    .unwrap()
            };
            let named = named_first.then(|| create_named(&mut engine));
            let root = engine
                .create_disk_root("P01", "owner", &storage_engine::new_id("op"))
                .unwrap();
            let mut paths = vec![format!("{root_name}-other")];
            if root_is_file {
                let workspace = engine
                    .create_workspace(&root.main_branch_id, "owner")
                    .unwrap();
                engine
                    .stage_bytes(&workspace.workspace_id, root_name, b"root file")
                    .unwrap();
                engine
                    .commit_workspace(storage_engine::CommitRequest {
                        workspace_id: workspace.workspace_id,
                        operation_id: storage_engine::new_id("op"),
                        principal: "owner".into(),
                        expected_head: workspace.base_head,
                        fencing_token: workspace.fencing_token,
                        message: "Save root file".into(),
                        request_digest: None,
                    })
                    .unwrap();
            } else {
                paths.push(format!("{root_name}/root-only"));
            }
            for path in paths {
                engine
                    .file_namespace(
                        &root.repository_id,
                        NamespaceOperation {
                            kind: "mkdir".into(),
                            path,
                            destination: None,
                        },
                        "owner",
                        &storage_engine::new_id("op"),
                    )
                    .unwrap();
            }
            let named = named.unwrap_or_else(|| create_named(&mut engine));
            engine
                .file_namespace(
                    &named.repository_id,
                    NamespaceOperation {
                        kind: "mkdir".into(),
                        path: "named-only".into(),
                        destination: None,
                    },
                    "owner",
                    &storage_engine::new_id("op"),
                )
                .unwrap();
            let identity = Identity {
                admission_session: None,
                smb_binding: None,
                principal: "owner".into(),
            };
            let before = engine.state().clone();
            let capacity = engine.workspace_capacity("owner");
            let bytes = disk_files(&disks);
            for path in [
                String::new(),
                root_name.into(),
                named_name.into(),
                format!("{root_name}/root-only"),
            ] {
                let failure =
                    listing::entries(&engine, &json!({"slot_id":"P01","path":path})).unwrap_err();
                assert_eq!(
                    (failure.status, failure.code.as_str()),
                    (409, "NAME_COLLISION")
                );
            }
            for (action, input) in [
                (
                    BackendAction::FileManagerNamespace,
                    json!({"slot_id":"P01","kind":"mkdir","path":format!("{root_name}/new-directory"),"operation_id":storage_engine::new_id("op")}),
                ),
                (
                    BackendAction::FileManagerWriteSession,
                    json!({"slot_id":"P01","path":format!("{root_name}/new-file.txt")}),
                ),
                (
                    BackendAction::FileManagerDirectoryBranches,
                    json!({"slot_id":"P01","path":root_name,"name":"new-branch","operation_id":storage_engine::new_id("op")}),
                ),
                (
                    BackendAction::FileManagerDirectorySelect,
                    json!({"slot_id":"P01","path":root_name,"name":"main","operation_id":storage_engine::new_id("op")}),
                ),
            ] {
                let failure = call(&mut engine, &identity, action, input).unwrap_err();
                assert_eq!(
                    (failure.status, failure.code.as_str()),
                    (409, "NAME_COLLISION")
                );
                assert_eq!(engine.state(), &before);
                assert_eq!(engine.workspace_capacity("owner"), capacity);
                assert_eq!(disk_files(&disks), bytes);
            }
            // Ambiguity belongs to the merged disk path, not repository state.
            // Explicit identities and unrelated component-boundary paths work.
            assert!(
                !engine
                    .list_tree_ref(&root.main_branch_id)
                    .unwrap()
                    .is_empty()
            );
            assert!(
                !engine
                    .list_tree_ref(&named.main_branch_id)
                    .unwrap()
                    .is_empty()
            );
            let other = listing::entries(
                &engine,
                &json!({"slot_id":"P01","path":format!("{root_name}-other")}),
            )
            .unwrap();
            assert_eq!(other["context"]["repository_id"], root.repository_id);
            let repaired = format!("{root_name}-repaired");
            engine
                .file_namespace(
                    &root.repository_id,
                    NamespaceOperation {
                        kind: "rename".into(),
                        path: root_name.into(),
                        destination: Some(repaired.clone()),
                    },
                    "owner",
                    &storage_engine::new_id("op"),
                )
                .unwrap();
            let listing = listing::entries(&engine, &json!({"slot_id":"P01","path":""})).unwrap();
            let listed = listing["entries"].as_array().unwrap();
            let keys = listed
                .iter()
                .map(|entry| name_policy::path_key(entry["path"].as_str().unwrap()))
                .collect::<BTreeSet<_>>();
            assert_eq!(keys.len(), listed.len());
            for (path, expected) in [
                (named_name, &named.repository_id),
                (repaired.as_str(), &root.repository_id),
            ] {
                assert_eq!(
                    resolve_read(&engine, "P01", path).unwrap().0.repository_id,
                    *expected
                );
            }
        }
    }
}
