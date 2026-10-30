use super::*;
use crate::engine_backend::{field, wire::PendingSources};
use crate::tests::{history_read_control as tree_control, tree_value};
use crate::{
    Backend,
    auth::{AuthStore, Identity},
    engine_backend::{EngineBackend, install_test_administrator},
};
use data_serialization::{Serialize, Serializer, ser::Error};
use json_codec::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use storage_contract::Config;

#[path = "alias_order_tests.rs"]
mod alias_order_tests;
#[path = "authorization_tests.rs"]
mod authorization_tests;

fn capture_tree_fixture(
    engine: &Engine,
    request: crate::ReadTree<'_>,
) -> Result<crate::TreeSnapshot, crate::ApiError> {
    capture_tree(engine.published_read_view(), request, &tree_control())
}

fn fixture() -> (
    tempfile::TempDir,
    Engine,
    storage_engine::Repository,
    Identity,
) {
    let root = crate::tests::private_tempdir();
    let config = Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ]);
    let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Projection").unwrap(),
            "P01",
            3,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let identity = install_test_administrator(&mut engine, "owner");
    (root, engine, repo, identity)
}

#[test]
fn encoded_tree_returns_without_retaining_engine_lock() {
    let (root, engine, repo, _) = fixture();
    let (backend, _auth, _token, identity) = authenticated_backend(engine, root.path());
    let commit = backend.engine.lock().unwrap().state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let snapshot = backend
        .tree_snapshot(
            &identity,
            crate::ReadTree {
                branch: &repo.main_branch_id,
                commit: None,
            },
            &tree_control(),
        )
        .unwrap();
    assert!(std::thread::scope(|scope| {
        scope
            .spawn(|| backend.engine.try_lock().is_ok())
            .join()
            .unwrap()
    }));
    let expected = json!({"branch_id":repo.main_branch_id,"commit_id":commit,"entries":[],"pending_file_versions":{}});
    assert_eq!(tree_value(&snapshot), expected);
}

#[test]
fn tree_encoder_accepts_exact_byte_cap_and_rejects_one_byte_more() {
    use storage_contract::client_protocol::MAX_RESPONSE_BYTES;
    let mut pending = BTreeMap::from([("file".to_owned(), String::new())]);
    let shell = crate::TreeSnapshot::encode("branch", "commit", [], &pending).unwrap();
    pending
        .get_mut("file")
        .unwrap()
        .push_str(&"x".repeat(MAX_RESPONSE_BYTES - shell.body().len()));
    let exact = crate::TreeSnapshot::encode("branch", "commit", [], &pending).unwrap();
    assert_eq!(exact.body().len(), MAX_RESPONSE_BYTES);
    assert_eq!(tree_value(&exact)["pending_file_versions"], json!(pending));
    pending.get_mut("file").unwrap().push('x');
    assert_eq!(
        crate::TreeSnapshot::encode("branch", "commit", [], &pending)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::ResourceLimitExceeded,
    );
}

#[test]
fn tree_encoder_escapes_data_and_stops_before_consuming_later_entries() {
    let branch = "branch\"\\\n";
    let commit = "commit\t\u{1}é";
    let pending = BTreeMap::from([("file\"\\".to_owned(), "source\n\t".to_owned())]);
    let encoded = crate::TreeSnapshot::encode(branch, commit, [], &pending).unwrap();
    assert_eq!(
        tree_value(&encoded),
        json!({
            "branch_id": branch, "commit_id": commit, "entries": [],
            "pending_file_versions": pending,
        })
    );

    let mut oversized = directory_entry(0, "first.txt".into());
    oversized.version_id = "x".repeat(storage_contract::client_protocol::MAX_RESPONSE_BYTES);
    let visited = std::cell::Cell::new(0);
    let entries = [oversized, directory_entry(1, "later.txt".into())]
        .into_iter()
        .inspect(|_| visited.set(visited.get() + 1));
    assert_eq!(
        crate::TreeSnapshot::encode("branch", "commit", entries, &BTreeMap::new())
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::ResourceLimitExceeded,
    );
    assert_eq!(visited.get(), 1);
}

#[test]
fn detached_tree_view_keeps_publication_after_new_head_and_engine_drop() {
    let (_root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "before.txt", b"before");
    let request = crate::ReadTree {
        branch: &repo.main_branch_id,
        commit: None,
    };
    let expected = tree_value(&capture_tree_fixture(&engine, request).unwrap());
    let view = engine.published_read_view();
    save_projection_file(&mut engine, &repo.main_branch_id, "after.txt", b"after");
    drop(engine);
    let captured = capture_tree(
        view,
        crate::ReadTree {
            branch: &repo.main_branch_id,
            commit: None,
        },
        &tree_control(),
    )
    .unwrap();
    assert_eq!(tree_value(&captured), expected);
}

#[test]
fn detached_tree_read_honors_expired_control() {
    let (_root, engine, repo, _) = fixture();
    let expired = crate::HistoryReadControl::until(std::time::Instant::now());
    let error = capture_tree(
        engine.published_read_view(),
        crate::ReadTree {
            branch: &repo.main_branch_id,
            commit: None,
        },
        &expired,
    )
    .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::RequestTimeout);
    assert_eq!(error.status, ::http::StatusCode::REQUEST_TIMEOUT);
}

#[test]
fn tree_completion_reauthorizes_before_results_read_errors_and_expiry() {
    let (_root, mut engine, repo, identity) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "before.txt", b"before");
    let backend = EngineBackend::new(engine);
    let view = backend.engine.lock().unwrap().published_read_view();
    save_projection_file(
        &mut backend.engine.lock().unwrap(),
        &repo.main_branch_id,
        "after.txt",
        b"after",
    );
    let snapshot = capture_tree(
        view,
        crate::ReadTree {
            branch: &repo.main_branch_id,
            commit: None,
        },
        &tree_control(),
    )
    .unwrap();
    assert_eq!(
        tree_value(&snapshot)["entries"].as_array().unwrap().len(),
        1
    );
    let failure = || {
        crate::ApiError::new(
            ::http::StatusCode::INTERNAL_SERVER_ERROR,
            storage_contract::ErrorCode::ObjectCorrupt,
            "selected namespace is corrupt",
        )
    };
    assert_eq!(
        backend
            .finish_tree_snapshot(&identity, Err(failure()), &tree_control())
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::ObjectCorrupt,
    );
    install_test_administrator(&mut backend.engine.lock().unwrap(), "replacement");
    for result in [Ok(snapshot), Err(failure())] {
        assert_eq!(
            backend
                .finish_tree_snapshot(&identity, result, &tree_control())
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::Unauthorized,
        );
    }
    let expired = crate::HistoryReadControl::until(std::time::Instant::now());
    assert_eq!(
        backend
            .finish_tree_snapshot(&identity, Err(failure()), &expired)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized,
    );
}

#[test]
fn detached_historical_context_first_none_suppresses_later_inheritance_and_fallback() {
    let (_root, mut engine, repo, _) = fixture();
    let (commit, entries) = synthetic_commit(&mut engine, &repo, 1);
    let entry = &entries[0];
    let mut state = engine.state().clone();
    for (id, inherited) in [("a-first", None), ("z-later", Some("source".to_owned()))] {
        state.directory_trees.insert(
            id.into(),
            storage_contract::DirectoryTree {
                tree_id: id.into(),
                directory_branch_id: "directory".into(),
                parent_tree_id: None,
                commit_id: commit.commit_id.clone(),
                path: String::new(),
                entries: Vec::new(),
                file_branches: BTreeMap::new(),
                file_contexts: BTreeMap::from([(
                    entry.lookup_key().to_owned(),
                    storage_contract::FileContext {
                        node_id: None,
                        selection_generation: 0,
                        branch_id: repo.main_branch_id.clone(),
                        inherited_from: inherited,
                    },
                )]),
                inherited_files: BTreeMap::new(),
                directory_refs: BTreeMap::new(),
                sequence: state.sequence,
            },
        );
    }
    storage_engine::test_support::replace_state(&mut engine, state);
    let snapshot = capture_tree_fixture(
        &engine,
        crate::ReadTree {
            branch: &repo.main_branch_id,
            commit: Some(&commit.commit_id),
        },
    )
    .unwrap();
    assert_eq!(tree_value(&snapshot)["pending_file_versions"], json!({}));
    // Removing only the first context exposes the later inherited marker.
    let mut state = engine.state().clone();
    state
        .directory_trees
        .get_mut("a-first")
        .unwrap()
        .file_contexts
        .clear();
    storage_engine::test_support::replace_state(&mut engine, state);
    let snapshot = capture_tree_fixture(
        &engine,
        crate::ReadTree {
            branch: &repo.main_branch_id,
            commit: Some(&commit.commit_id),
        },
    )
    .unwrap();
    assert_eq!(
        tree_value(&snapshot)["pending_file_versions"],
        json!(BTreeMap::from([(
            entry.file_id.clone(),
            entry.version_id.clone()
        ),]))
    );
}

fn authenticated_backend(
    engine: Engine,
    root: &std::path::Path,
) -> (Arc<EngineBackend>, AuthStore, String, Identity) {
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = AuthStore::initialize(root, "owner", "integration-test-password").unwrap();
    let mut auth = backend.protect_auth(auth).unwrap();
    let login = auth.login("owner", "integration-test-password").unwrap();
    let identity = auth.authenticate(&login.token).unwrap();
    (backend, auth, login.token, identity)
}

#[test]
fn tree_projection_is_frozen_and_preserves_admission_and_wire_shape() {
    let (root, mut engine, repo, _) = fixture();
    let input = json!({"branch_id": repo.main_branch_id});
    let expected = json!({
        "branch_id": repo.main_branch_id,
        "commit_id": engine.state().branches[&repo.main_branch_id].head_commit_id,
        "entries": engine.list_tree(&repo.main_branch_id).unwrap(),
        "pending_file_versions": {},
    });
    let projection = capture_tree_fixture(&engine, crate::tests::tree_request(&input)).unwrap();
    let other = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Other").unwrap(),
            "P01",
            3,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let foreign = &engine.state().branches[&other.main_branch_id].head_commit_id;
    assert_eq!(
        capture_tree_fixture(
            &engine,
            crate::tests::tree_request(&json!({
                "branch_id": repo.main_branch_id, "query": {"commit_id": foreign}
            }))
        )
        .err()
        .unwrap()
        .code,
        storage_contract::ErrorCode::NotFound
    );
    assert_eq!(tree_value(&projection), expected);
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    assert_eq!(
        backend
            .tree_snapshot(
                &identity,
                crate::tests::tree_request(&input),
                &tree_control()
            )
            .map(|snapshot| tree_value(&snapshot))
            .unwrap(),
        expected
    );
    let denied = Identity {
        principal: "other".into(),
        ..identity.clone()
    };
    assert_eq!(
        backend
            .tree_snapshot(&denied, crate::tests::tree_request(&input), &tree_control())
            .map(|snapshot| tree_value(&snapshot))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    let frozen = {
        let mut engine = backend.engine.lock().unwrap();
        let frozen = capture_tree_fixture(&engine, crate::tests::tree_request(&input)).unwrap();
        save_projection_file(&mut engine, &repo.main_branch_id, "new.txt", b"new content");
        frozen
    };
    let current = backend
        .tree_snapshot(
            &identity,
            crate::tests::tree_request(&input),
            &tree_control(),
        )
        .map(|snapshot| tree_value(&snapshot))
        .unwrap();
    assert_ne!(current["commit_id"], expected["commit_id"]);
    assert_eq!(current["entries"].as_array().unwrap().len(), 1);
    auth.logout(&token).unwrap();
    // An admitted projection keeps its owned snapshot across live data changes
    // and revocation, but can never admit the session to a subsequent request.
    assert_eq!(tree_value(&frozen), expected);
    assert_eq!(
        backend
            .tree_snapshot(
                &identity,
                crate::tests::tree_request(&input),
                &tree_control()
            )
            .map(|snapshot| tree_value(&snapshot))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
}

// Exact pre-optimization behavior used to compare the request-local resolver.
// Commits no longer embed entries, so the oracle takes the commit's complete
// materialized namespace exactly as the old flat scan saw it.
fn legacy_pending_entry_source<'a>(
    engine: &'a Engine,
    branch: &str,
    commit: &'a storage_contract::Commit,
    commit_entries: &'a [storage_contract::Entry],
    entry: &storage_contract::Entry,
) -> Option<&'a str> {
    if entry.kind != storage_contract::NamespaceEntryKind::File {
        return None;
    }
    let state = engine.state();
    let visible = commit_entries.iter().find(|visible| {
        visible.lookup_key() == entry.lookup_key() && visible.file_id == entry.file_id
    })?;
    let current = state.branches.get(branch)?;
    if current.head_commit_id == commit.commit_id {
        if let Some(repo) = state
            .repositories
            .values()
            .find(|repo| repo.main_branch_id == branch)
            && let Some(context) = engine
                .file_context(&repo.repository_id, &entry.path)
                .unwrap()
        {
            return if context.inherited_from.is_some() {
                Some(visible.version_id.as_str())
            } else {
                state
                    .branches
                    .get(&context.branch_id)?
                    .pending_file_versions
                    .get(&entry.file_id)
                    .map(String::as_str)
            };
        }
    } else if let Some(context) = state
        .directory_trees
        .values()
        .filter(|tree| tree.commit_id == commit.commit_id)
        .find_map(|tree| tree.file_contexts.get(entry.lookup_key()))
    {
        return context
            .inherited_from
            .as_ref()
            .map(|_| visible.version_id.as_str());
    }
    commit
        .pending_file_versions
        .get(&entry.file_id)
        .filter(|version| **version == entry.version_id)
        .map(String::as_str)
        .or_else(|| {
            (current.head_commit_id == commit.commit_id)
                .then(|| engine.pending_file_source(branch, &entry.file_id))
                .flatten()
        })
}

/// A historical commit with `count` synthetic file entries whose namespace is
/// planted on the engine's members and installed into the engine state, so
/// resolver point reads see exactly these entries.
fn synthetic_commit(
    engine: &mut Engine,
    repo: &storage_engine::Repository,
    count: usize,
) -> (storage_contract::Commit, Vec<storage_contract::Entry>) {
    let head = &engine.state().branches[&repo.main_branch_id].head_commit_id;
    let mut commit = engine.state().commits[head].clone();
    commit.commit_id = "historical-projection-fixture".into();
    let entries: Vec<_> = (0..count)
        .map(|index| {
            let path = format!("file-{index:05}.txt");
            storage_contract::Entry {
                entry_id: format!("entry-{index}"),
                path: name_policy::PortablePath::parse(&path).unwrap(),
                kind: storage_contract::NamespaceEntryKind::File,
                file_id: format!("file-{index}"),
                version_id: format!("version-{index}"),
                size: index as u64,
            }
        })
        .collect();
    commit.namespace = storage_engine::test_support::plant_namespace(engine, &entries).unwrap();
    commit.pending_file_versions = entries
        .iter()
        .map(|entry| (entry.file_id.clone(), entry.version_id.clone()))
        .collect();
    let mut state = engine.state().clone();
    state
        .commits
        .insert(commit.commit_id.clone(), commit.clone());
    storage_engine::test_support::replace_state(engine, state);
    (commit, entries)
}

#[test]
fn canonical_tree_resolution_eliminates_repeated_entry_matching_at_scale() {
    let (_root, mut engine, repo, _) = fixture();
    for count in [100, 1_000, 10_000] {
        let (commit, entries) = synthetic_commit(&mut engine, &repo, count);
        let resolver = PendingSources::new(&engine, &repo.main_branch_id, &commit);
        let optimized = resolver.pending_versions().unwrap();
        assert_eq!(
            resolver.matching_visits(),
            0,
            "known entries need no search"
        );
        let checked: BTreeMap<_, _> = entries
            .iter()
            .filter_map(|entry| {
                resolver
                    .checked(entry)
                    .unwrap()
                    .map(|source| (entry.file_id.clone(), source))
            })
            .collect();
        // One bounded point read per checked entry, never a snapshot scan:
        // the old flat resolver visited count*(count+1)/2 entries here.
        assert_eq!(resolver.matching_visits(), count);
        assert_eq!(optimized, checked);
        assert_eq!(optimized, commit.pending_file_versions);
        // The exact original implementation remains the semantic oracle, not a
        // timing threshold. Its loop is also exercised at every requested size.
        let legacy: BTreeMap<_, _> = entries
            .iter()
            .filter_map(|entry| {
                legacy_pending_entry_source(&engine, &repo.main_branch_id, &commit, &entries, entry)
                    .map(|source| (entry.file_id.clone(), source.to_owned()))
            })
            .collect();
        assert_eq!(optimized, legacy);
    }
}

#[test]
fn checked_entries_resolve_against_published_identity_not_caller_claims() {
    let (_root, mut engine, repo, _) = fixture();
    let (commit, entries) = synthetic_commit(&mut engine, &repo, 3);
    let resolver = PendingSources::new(&engine, &repo.main_branch_id, &commit);
    let expected: BTreeMap<_, _> = entries
        .iter()
        .filter_map(|entry| {
            legacy_pending_entry_source(&engine, &repo.main_branch_id, &commit, &entries, entry)
                .map(|source| (entry.file_id.clone(), source.to_owned()))
        })
        .collect();
    assert_eq!(resolver.pending_versions().unwrap(), expected);
    // An external snapshot's entry resolves through the commit's own published
    // entry at that key: the caller's claimed version never participates.
    let mut external = entries[0].clone();
    external.version_id = "different-version".into();
    assert_eq!(
        resolver.checked(&external).unwrap(),
        legacy_pending_entry_source(&engine, &repo.main_branch_id, &commit, &entries, &external)
            .map(str::to_owned)
    );
    for entry in &entries {
        assert_eq!(
            resolver.checked(entry).unwrap(),
            legacy_pending_entry_source(&engine, &repo.main_branch_id, &commit, &entries, entry)
                .map(str::to_owned)
        );
    }
    external.file_id = "absent-file".into();
    assert_eq!(resolver.checked(&external).unwrap(), None);
    external.kind = storage_contract::NamespaceEntryKind::Directory;
    assert_eq!(resolver.checked(&external).unwrap(), None);
    assert!(
        resolver.matching_visits() > 0,
        "external input keeps checked lookup"
    );
}

fn save_projection_file(engine: &mut Engine, branch: &str, path: &str, body: &[u8]) {
    let workspace = engine.create_workspace(branch, "owner").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, body)
        .unwrap();
    engine
        .commit_workspace(storage_engine::CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap();
}

#[test]
fn real_current_and_historical_tree_sources_match_previous_wire_output() {
    let (_root, mut engine, repo, _) = fixture();
    save_projection_file(
        &mut engine,
        &repo.main_branch_id,
        "folder/data.txt",
        b"first",
    );
    let first = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    save_projection_file(
        &mut engine,
        &repo.main_branch_id,
        "folder/data.txt",
        b"second",
    );
    let current = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    for commit_id in [&first, &current] {
        let commit = &engine.state().commits[commit_id];
        let entries = engine
            .list_tree_at(&repo.main_branch_id, commit_id)
            .unwrap();
        let expected: BTreeMap<_, _> = entries
            .iter()
            .filter_map(|entry| {
                legacy_pending_entry_source(&engine, &repo.main_branch_id, commit, &entries, entry)
                    .map(|source| (entry.file_id.clone(), source.to_owned()))
            })
            .collect();
        let projection = capture_tree_fixture(
            &engine,
            crate::tests::tree_request(&json!({
                "branch_id": repo.main_branch_id, "query": {"commit_id": commit_id}
            })),
        )
        .unwrap();
        assert_eq!(
            tree_value(&projection),
            json!({
                "branch_id": repo.main_branch_id,
                "commit_id": commit_id,
                "entries": entries,
                "pending_file_versions": expected,
            })
        );
        for entry in &entries {
            assert_eq!(
                PendingSources::new(&engine, &repo.main_branch_id, commit)
                    .checked(entry)
                    .unwrap(),
                legacy_pending_entry_source(&engine, &repo.main_branch_id, commit, &entries, entry)
                    .map(str::to_owned)
            );
            if commit_id == &current {
                assert_eq!(
                    engine
                        .file_context_for_current_entry(&repo.repository_id, entry)
                        .map(|context| context.into_owned()),
                    engine
                        .file_context(&repo.repository_id, &entry.path)
                        .unwrap()
                );
            }
        }
    }
}

#[test]
fn pending_main_file_and_historical_branch_sources_match_previous_response() {
    let (_root, mut engine, repo, _) = fixture();
    save_projection_file(
        &mut engine,
        &repo.main_branch_id,
        "data.txt",
        b"parent bytes",
    );
    let original = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == storage_contract::NamespaceEntryKind::File)
        .unwrap();
    let created = engine
        .create_file_branch(
            &original.file_id,
            &original.version_id,
            &storage_contract::BranchName::parse("pending-view").unwrap(),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let pending = created.result["branch_id"].as_str().unwrap().to_owned();
    engine
        .select_file_branch(
            &original.file_id,
            &pending,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let old_pending = engine.state().branches[&pending].head_commit_id.clone();
    let assert_sources = |engine: &Engine, branch: &str, commit_id: &str| {
        let commit = &engine.state().commits[commit_id];
        let entries = engine.list_tree_at(branch, commit_id).unwrap();
        let legacy: BTreeMap<_, _> = entries
            .iter()
            .filter_map(|entry| {
                legacy_pending_entry_source(engine, branch, commit, &entries, entry)
                    .map(|source| (entry.file_id.clone(), source.to_owned()))
            })
            .collect();
        let tree = capture_tree_fixture(
            engine,
            crate::tests::tree_request(&json!({
                "branch_id": branch, "query": {"commit_id": commit_id}
            })),
        )
        .unwrap();
        assert_eq!(
            tree_value(&tree),
            json!({
                "branch_id": branch,
                "commit_id": commit_id,
                "entries": entries,
                "pending_file_versions": legacy,
            })
        );
        legacy
    };
    for branch in [&repo.main_branch_id, &pending] {
        let commit_id = &engine.state().branches[branch].head_commit_id;
        let sources = assert_sources(&engine, branch, commit_id);
        assert_eq!(sources.get(&original.file_id), Some(&original.version_id));
    }
    save_projection_file(
        &mut engine,
        &repo.main_branch_id,
        "data.txt",
        b"written branch bytes",
    );
    let sources = assert_sources(&engine, &pending, &old_pending);
    assert_eq!(sources.get(&original.file_id), Some(&original.version_id));
    let current = &engine.state().branches[&repo.main_branch_id].head_commit_id;
    assert!(assert_sources(&engine, &repo.main_branch_id, current).is_empty());
}

#[test]
fn inherited_directory_context_keeps_first_match_and_historical_sources() {
    let (_root, mut engine, repo, _) = fixture();
    save_projection_file(
        &mut engine,
        &repo.main_branch_id,
        "folder/data.txt",
        b"inherited bytes",
    );
    engine
        .create_directory_branches(
            &repo.repository_id,
            Some(&name_policy::LookupPath::parse("folder").unwrap()),
            &storage_contract::BranchName::parse("inherited-view").unwrap(),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    engine
        .select_directory_branches(
            &repo.repository_id,
            Some(&name_policy::LookupPath::parse("folder").unwrap()),
            &storage_contract::BranchName::parse("inherited-view").unwrap(),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let head = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let commit = &engine.state().commits[&head];
    let head_entries = engine.list_tree_at(&repo.main_branch_id, &head).unwrap();
    let original = head_entries
        .iter()
        .find(|entry| entry.kind == storage_contract::NamespaceEntryKind::File)
        .unwrap();
    assert!(
        engine
            .file_context(&repo.repository_id, &original.path)
            .unwrap()
            .unwrap()
            .inherited_from
            .is_some()
    );
    assert!(matches!(
        engine.file_context_for_current_entry(&repo.repository_id, original),
        Some(std::borrow::Cow::Borrowed(_))
    ));
    let resolver = PendingSources::new(&engine, &repo.main_branch_id, commit);
    assert_eq!(
        resolver.checked(original).unwrap().as_deref(),
        Some(original.version_id.as_str())
    );
    let mut external = original.clone();
    external.version_id = "not-the-first-visible-version".into();
    assert_eq!(
        resolver.checked(&external).unwrap().as_deref(),
        Some(original.version_id.as_str())
    );
    assert_eq!(
        resolver.checked(&external).unwrap(),
        legacy_pending_entry_source(
            &engine,
            &repo.main_branch_id,
            commit,
            &head_entries,
            &external
        )
        .map(str::to_owned)
    );
    let expected: BTreeMap<_, _> = head_entries
        .iter()
        .filter_map(|entry| {
            legacy_pending_entry_source(&engine, &repo.main_branch_id, commit, &head_entries, entry)
                .map(|source| (entry.file_id.clone(), source.to_owned()))
        })
        .collect();
    assert_eq!(resolver.pending_versions().unwrap(), expected);
    save_projection_file(
        &mut engine,
        &repo.main_branch_id,
        "folder/data.txt",
        b"materialized",
    );
    let historical = &engine.state().commits[&head];
    let expected: BTreeMap<_, _> = head_entries
        .iter()
        .filter_map(|entry| {
            legacy_pending_entry_source(
                &engine,
                &repo.main_branch_id,
                historical,
                &head_entries,
                entry,
            )
            .map(|source| (entry.file_id.clone(), source.to_owned()))
        })
        .collect();
    let projection = capture_tree_fixture(
        &engine,
        crate::tests::tree_request(&json!({
            "branch_id": repo.main_branch_id, "query": {"commit_id": head}
        })),
    )
    .unwrap();
    assert_eq!(
        tree_value(&projection)["pending_file_versions"],
        json!(expected)
    );
}

fn directory_entry(index: usize, path: String) -> storage_contract::Entry {
    storage_contract::Entry {
        entry_id: format!("entry_{index:032x}"),
        path: name_policy::PortablePath::parse(&path).unwrap(),
        kind: storage_contract::NamespaceEntryKind::File,
        file_id: format!("file_{index:032x}"),
        version_id: format!("version_{index:032x}"),
        size: 0,
    }
}
/// Point the branch head at a planted namespace of exactly these entries.
fn plant_head_namespace(
    engine: &mut Engine,
    repo: &storage_engine::Repository,
    entries: &[storage_contract::Entry],
) -> String {
    let namespace = storage_engine::test_support::plant_namespace(engine, entries).unwrap();
    let mut state = engine.state().clone();
    let head = state.branches[&repo.main_branch_id].head_commit_id.clone();
    state.commits.get_mut(&head).unwrap().namespace = namespace;
    storage_engine::test_support::replace_state(engine, state);
    head
}
#[test]
fn large_directory_is_paginated_by_actual_bytes_and_count_without_full_tree_copies() {
    use smb_protocol::protocol::{
        MAX_DIRECTORY_PAGE_BYTES, MAX_DIRECTORY_PAGE_ENTRIES, MAX_FRAME_BYTES, Response,
    };
    let (_root, mut engine, repo, _) = fixture();
    let mut entries: Vec<_> = (0..12_000)
        .map(|index| directory_entry(index, format!("f{index:05}-{}", "x".repeat(180))))
        .collect();
    for entry in &entries {
        name_policy::validate_path(&entry.path).unwrap();
    }
    entries.sort_by(|left, right| left.lookup_key().cmp(right.lookup_key()));
    assert!(
        json_codec::to_vec(&json!({"entries":entries}))
            .unwrap()
            .len()
            > MAX_FRAME_BYTES
    );
    let head = plant_head_namespace(&mut engine, &repo, &entries);
    let tree = engine.namespace_tree(&repo.main_branch_id).unwrap();
    let mut after = None;
    let mut seen = Vec::new();
    loop {
        let page = directory_page(&head, &tree, "", after.as_deref()).unwrap();
        assert!(page.entries.len() <= MAX_DIRECTORY_PAGE_ENTRIES);
        assert!(
            json_codec::to_vec(&Response::Ok {
                result: json_codec::to_value(&page).unwrap()
            })
            .unwrap()
            .len()
                <= MAX_DIRECTORY_PAGE_BYTES
        );
        assert!(!page.more || !page.entries.is_empty());
        after = page
            .entries
            .last()
            .map(|entry| entry.lookup_key().to_owned());
        seen.extend(page.entries.iter().map(|entry| entry.entry_id.clone()));
        if !page.more {
            break;
        }
    }
    assert_eq!(
        seen,
        entries
            .iter()
            .map(|entry| entry.entry_id.clone())
            .collect::<Vec<_>>()
    );
}
#[test]
fn directory_byte_bound_applies_to_utf8_and_long_paths_before_the_count_limit() {
    use smb_protocol::protocol::{MAX_DIRECTORY_PAGE_BYTES, MAX_DIRECTORY_PAGE_ENTRIES, Response};
    // Valid paths have up to 200 UTF-16 units, so non-ASCII names can consume
    // considerably more bytes than the entry-count budget alone predicts.
    let (_root, mut engine, repo, _) = fixture();
    let parent = vec!["界".repeat(50); 3].join("/");
    let entries: Vec<_> = (0..300)
        .map(|index| directory_entry(index, format!("{parent}/f{index:05}")))
        .collect();
    for entry in &entries {
        name_policy::validate_path(&entry.path).unwrap();
    }
    let head = plant_head_namespace(&mut engine, &repo, &entries);
    let tree = engine.namespace_tree(&repo.main_branch_id).unwrap();
    let page = directory_page(&head, &tree, &name_policy::path_key(&parent), None).unwrap();
    assert!(page.more);
    assert!(page.entries.len() < MAX_DIRECTORY_PAGE_ENTRIES);
    assert!(
        json_codec::to_vec(&Response::Ok {
            result: json_codec::to_value(page).unwrap()
        })
        .unwrap()
        .len()
            <= MAX_DIRECTORY_PAGE_BYTES
    );
}

#[test]
fn smb_lookup_matches_legacy_on_published_and_reopened_unicode_trees() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(
        &mut engine,
        &repo.main_branch_id,
        "Straße/Café.txt",
        b"content",
    );
    save_projection_file(&mut engine, &repo.main_branch_id, "Zed.txt", b"last");
    let expected = engine
        .namespace_tree(&repo.main_branch_id)
        .unwrap()
        .get("strasse/CAFÉ.TXT")
        .unwrap()
        .unwrap();
    for reopened in [false, true] {
        if reopened {
            drop(engine);
            engine = Engine::open(&root.path().join("system")).unwrap();
        }
        for path in [
            "STRASSE/CAFE\u{301}.TXT",
            "straße/Café.txt",
            "zed.TXT",
            "absent",
            "",
        ] {
            let key = name_policy::path_key(path);
            // Exact former lookup; only canonical published entries enter the
            // new view. Engine boundary tests reject bad keys/order/collisions.
            let legacy = engine
                .list_tree(&repo.main_branch_id)
                .unwrap()
                .into_iter()
                .find(|entry| name_policy::path_key(&entry.path) == key);
            if path.is_empty() {
                assert!(name_policy::LookupPath::parse(path).is_err());
                continue;
            }
            let projected = lookup_current(
                &engine,
                &repo.main_branch_id,
                &name_policy::LookupPath::parse(path).unwrap(),
            )
            .unwrap();
            assert_eq!(projected, legacy);
        }
        let projected = json_codec::to_value(
            lookup_current(
                &engine,
                &repo.main_branch_id,
                &name_policy::LookupPath::parse("strasse/CAFÉ.TXT").unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(projected["version_id"], expected.version_id);
        assert!(projected.get("entries").is_none());
        assert!(projected.get("pending_file_versions").is_none());
        assert!(json_codec::to_vec(&projected).unwrap().len() < 1024);
    }
}

#[test]
fn smb_lookup_rechecks_admission_and_returns_only_current_entry() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "Straße.txt", b"content");
    let expected = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| name_policy::path_key(&entry.path) == "strasse.txt");
    let path = name_policy::LookupPath::parse("STRASSE.TXT").unwrap();
    let frozen = lookup_current(&engine, &repo.main_branch_id, &path).unwrap();
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    assert_eq!(
        backend
            .lookup_current(&identity, &repo.main_branch_id, &path)
            .unwrap(),
        expected
    );
    assert_eq!(
        backend
            .lookup_current(
                &identity,
                &repo.main_branch_id,
                &name_policy::LookupPath::parse("missing").unwrap()
            )
            .unwrap(),
        None
    );
    let denied = Identity {
        principal: "other".into(),
        ..identity.clone()
    };
    assert_eq!(
        backend
            .lookup_current(&denied, &repo.main_branch_id, &path)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    {
        let mut engine = backend.engine.lock().unwrap();
        save_projection_file(
            &mut engine,
            &repo.main_branch_id,
            "Straße.txt",
            b"updated content",
        );
    }
    let current = backend
        .lookup_current(&identity, &repo.main_branch_id, &path)
        .unwrap();
    assert_ne!(
        current.unwrap().version_id,
        expected.as_ref().unwrap().version_id
    );
    auth.logout(&token).unwrap();
    assert_eq!(
        json_codec::to_value(&frozen).unwrap(),
        json_codec::to_value(expected).unwrap()
    );
    assert_eq!(
        backend
            .lookup_current(&identity, &repo.main_branch_id, &path)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
}

#[test]
fn namespace_reads_preserve_migrated_branch_error() {
    let (_root, mut engine, repo, identity) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"first");
    let cut = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"second");
    let plan = engine
        .create_split_plan(&repo.history_tree_id, &cut, "owner")
        .unwrap();
    engine
        .commit_plan(
            &plan.plan_id,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let backend = EngineBackend::new(engine);
    assert_eq!(
        backend
            .lookup_at(
                &identity,
                &repo.main_branch_id,
                &cut,
                &name_policy::LookupPath::parse("file.txt").unwrap()
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::TreeMoved
    );
    assert_eq!(
        backend
            .lookup_current(
                &identity,
                &repo.main_branch_id,
                &name_policy::LookupPath::parse("file.txt").unwrap()
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::TreeMoved
    );
    assert_eq!(
        backend
            .tree_snapshot(
                &identity,
                crate::ReadTree {
                    branch: &repo.main_branch_id,
                    commit: None
                },
                &tree_control(),
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::TreeMoved
    );
    assert_eq!(
        backend
            .resolve_entry(
                &identity,
                crate::ResolveEntry {
                    branch: &repo.main_branch_id,
                    path: &name_policy::LookupPath::parse("file.txt").unwrap(),
                    commit: None
                }
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::TreeMoved
    );
}

#[path = "lookup_at_tests.rs"]
mod lookup_at_tests;

#[path = "snapshot_at_tests.rs"]
mod snapshot_at_tests;

#[test]
fn typed_directory_reads_pin_snapshots_and_refresh_on_restart() {
    let (_root, mut engine, repo, identity) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "before.txt", b"first");
    let backend = EngineBackend::new(engine);
    let read = |selection| {
        backend.read_directory(
            &identity,
            crate::ReadDirectory {
                branch: &repo.main_branch_id,
                entry_id: &repo.main_branch_id,
                path: None,
                selection,
            },
        )
    };
    let captured = read(crate::DirectorySelection::Capture).unwrap();
    assert!(captured.entries.is_empty());
    assert!(!captured.more);
    {
        let mut engine = backend.engine.lock().unwrap();
        save_projection_file(&mut engine, &repo.main_branch_id, "after.txt", b"second");
    }
    let pinned = read(crate::DirectorySelection::Page {
        commit: Some(&captured.commit_id),
        after: None,
    })
    .unwrap();
    assert_eq!(pinned.commit_id, captured.commit_id);
    assert_eq!(
        pinned
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        vec!["before.txt"]
    );
    let restarted = read(crate::DirectorySelection::Page {
        commit: None,
        after: None,
    })
    .unwrap();
    assert_ne!(restarted.commit_id, captured.commit_id);
    assert_eq!(
        restarted
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        vec!["after.txt", "before.txt"]
    );
    let exhausted = read(crate::DirectorySelection::Page {
        commit: Some(&restarted.commit_id),
        after: Some("before.txt"),
    })
    .unwrap();
    assert!(exhausted.entries.is_empty());
    assert!(!exhausted.more);
    assert_eq!(
        backend
            .read_directory(
                &identity,
                crate::ReadDirectory {
                    branch: &repo.main_branch_id,
                    entry_id: "not-the-root",
                    path: None,
                    selection: crate::DirectorySelection::Capture,
                }
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::InvalidRequest
    );
    let path = name_policy::PortablePath::parse("before.txt").unwrap();
    assert_eq!(
        backend
            .read_directory(
                &identity,
                crate::ReadDirectory {
                    branch: &repo.main_branch_id,
                    entry_id: &pinned.entries[0].entry_id,
                    path: Some(&path),
                    selection: crate::DirectorySelection::Capture,
                }
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::SmbRebindRequired
    );
}

#[test]
fn typed_resolve_preserves_directory_absence_and_missing_entry_errors() {
    let (root, mut engine, repo, _) = fixture();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    engine
        .namespace_operation(
            &workspace.workspace_id,
            storage_contract::NamespaceOperation::Mkdir {
                path: name_policy::PortablePath::parse("Directory").unwrap(),
            },
        )
        .unwrap();
    engine
        .commit_workspace(storage_engine::CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap();
    let (backend, _auth, _token, identity) = authenticated_backend(engine, root.path());
    let resolved = backend
        .resolve_entry(
            &identity,
            crate::ResolveEntry {
                branch: &repo.main_branch_id,
                path: &name_policy::LookupPath::parse("DIRECTORY").unwrap(),
                commit: None,
            },
        )
        .unwrap();
    assert_eq!(
        resolved.entry.kind,
        storage_contract::NamespaceEntryKind::Directory
    );
    assert!(resolved.version.is_none());
    assert!(resolved.content.is_none());
    let wire = json_codec::to_value(crate::backend_read::ResolveResponse::from(resolved)).unwrap();
    assert_eq!(wire.as_object().unwrap().len(), 4);
    assert!(wire["version"].is_null());
    assert_eq!(wire["entry"]["path"], "Directory");
    let missing = backend
        .resolve_entry(
            &identity,
            crate::ResolveEntry {
                branch: &repo.main_branch_id,
                path: &name_policy::LookupPath::parse("missing").unwrap(),
                commit: None,
            },
        )
        .unwrap_err();
    assert_eq!(missing.code, storage_contract::ErrorCode::NotFound);
    assert_eq!(missing.message, "entry not found");
}

#[path = "graph_tests.rs"]
mod graph_tests;

#[path = "smb_binding_tests.rs"]
mod smb_binding_tests;

#[path = "storage_metadata_tests.rs"]
mod storage_metadata_tests;

#[path = "comparison_tests.rs"]
mod comparison_tests;

#[path = "history_tests.rs"]
mod history_tests;

#[path = "diagnostics_tests.rs"]
mod diagnostics_tests;

#[path = "default_directory_tests.rs"]
mod default_directory_tests;

#[path = "directory_scopes_tests.rs"]
mod directory_scopes_tests;

#[path = "directory_selection_tests.rs"]
mod directory_selection_tests;

#[path = "archive_tests.rs"]
mod archive_tests;

#[path = "create_branch_tests.rs"]
mod create_branch_tests;

#[path = "create_repository_tests.rs"]
mod create_repository_tests;

#[path = "merge_link_tests.rs"]
mod merge_link_tests;

#[test]
fn directory_scope_order_agrees_across_full_summary_listing_and_selection() {
    use crate::{DirectoryBranchFilter, DirectoryBranchProjection, DirectoryScopeRows};
    let (root, mut engine, repo, _) = fixture();
    for path in ["folder/a.txt", "folder/b.txt"] {
        save_projection_file(&mut engine, &repo.main_branch_id, path, b"content");
    }
    let path = name_policy::LookupPath::parse("folder").unwrap();
    for name in ["zeta", "alpha"] {
        engine
            .create_directory_branches(
                &repo.repository_id,
                Some(&path),
                &storage_contract::BranchName::parse(name).unwrap(),
                "owner",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
    }
    let mut expected = engine
        .state()
        .directory_branches
        .values()
        .filter(|branch| branch.repository_id == repo.repository_id && branch.path == "folder")
        .map(|branch| {
            (
                std::cmp::Reverse(branch.sequence),
                branch.directory_branch_id.clone(),
            )
        })
        .collect::<Vec<_>>();
    expected.sort();
    let expected = expected.into_iter().map(|(_, id)| id).collect::<Vec<_>>();
    assert!(expected.len() >= 3);
    // Deliberately request the opposite of display order. Selection must keep it.
    let sources = expected
        .iter()
        .rev()
        .map(|id| crate::DirectorySelectionSourceInput {
            directory_branch_id: id.clone(),
            expected_tree_id: engine.state().directory_branches[id].head_tree_id.clone(),
        })
        .collect();
    let selection = crate::ReadDirectorySelection::parse(
        repo.repository_id.clone(),
        "folder",
        sources,
        Vec::new(),
    )
    .unwrap();
    let (backend, _auth, _token, identity) = authenticated_backend(engine, root.path());
    let filter = DirectoryBranchFilter::Directory(path);
    for projection in [
        DirectoryBranchProjection::Full,
        DirectoryBranchProjection::Summary,
    ] {
        let snapshot = backend
            .directory_scopes(
                &identity,
                crate::ReadDirectoryScopes {
                    repository_id: &repo.repository_id,
                    filter: &filter,
                    projection,
                },
            )
            .unwrap();
        let ids = match snapshot.branches {
            DirectoryScopeRows::Full(branches) => branches
                .into_iter()
                .map(|row| row.branch.directory_branch_id)
                .collect::<Vec<_>>(),
            DirectoryScopeRows::Summary(branches) => branches
                .into_iter()
                .map(|row| row.directory_branch_id)
                .collect::<Vec<_>>(),
        };
        assert_eq!(ids, expected);
    }
    let listing = backend
        .listing(
            &identity,
            crate::ReadListing::parse(
                "P01".into(),
                BTreeMap::from([
                    ("path".into(), "Projection/folder".into()),
                    ("limit".into(), "1".into()),
                ]),
            )
            .unwrap(),
            &crate::tests::history_read_control(),
        )
        .unwrap();
    assert_eq!(
        listing
            .directory_branches
            .iter()
            .map(|row| &row.directory_branch_id)
            .collect::<Vec<_>>(),
        expected.iter().collect::<Vec<_>>()
    );
    assert_eq!(listing.entries.len(), 1);
    assert!(listing.next_cursor.is_some());
    let selected = backend.directory_selection(&identity, selection).unwrap();
    assert_eq!(
        selected
            .branches
            .iter()
            .map(|row| &row.summary.directory_branch_id)
            .collect::<Vec<_>>(),
        expected.iter().rev().collect::<Vec<_>>()
    );
    let missing =
        DirectoryBranchFilter::Directory(name_policy::LookupPath::parse("missing").unwrap());
    for projection in [
        DirectoryBranchProjection::Full,
        DirectoryBranchProjection::Summary,
    ] {
        let snapshot = backend
            .directory_scopes(
                &identity,
                crate::ReadDirectoryScopes {
                    repository_id: &repo.repository_id,
                    filter: &missing,
                    projection,
                },
            )
            .unwrap();
        match snapshot.branches {
            DirectoryScopeRows::Full(rows) => assert!(rows.is_empty()),
            DirectoryScopeRows::Summary(rows) => assert!(rows.is_empty()),
        }
        let Err(error) = backend.directory_scopes(
            &identity,
            crate::ReadDirectoryScopes {
                repository_id: "missing",
                filter: &filter,
                projection,
            },
        ) else {
            panic!("missing repository admitted");
        };
        assert_eq!(error.code, storage_contract::ErrorCode::NotFound);
    }
}

#[test]
fn owned_directory_scope_sort_preserves_tie_breaks_and_current_flags() {
    use crate::{
        DirectoryBranchChoice, DirectoryScopeBranch, DirectoryScopeRows, DirectoryScopesSnapshot,
    };
    let (_root, mut engine, repo, _) = fixture();
    save_projection_file(
        &mut engine,
        &repo.main_branch_id,
        "folder/file.txt",
        b"content",
    );
    engine
        .create_directory_branches(
            &repo.repository_id,
            Some(&name_policy::LookupPath::parse("folder").unwrap()),
            &storage_contract::BranchName::parse("template").unwrap(),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let template = engine.state().directory_branches.values().next().unwrap();
    // Synthetic observations exercise equal sequences independently of publish IDs.
    let branches = [(7, "z"), (6, "older"), (7, "a")]
        .into_iter()
        .map(|(sequence, id)| {
            let mut branch = template.clone();
            branch.sequence = sequence;
            branch.directory_branch_id = id.into();
            DirectoryScopeBranch {
                branch,
                current: id == "z",
            }
        })
        .collect::<Vec<_>>();
    let mut choices = branches
        .iter()
        .map(|row| DirectoryBranchChoice::capture(&row.branch, row.current))
        .collect::<Vec<_>>();
    let mut full = DirectoryScopesSnapshot {
        repository_id: repo.repository_id,
        branches: DirectoryScopeRows::Full(branches),
    };
    drop(engine);
    crate::engine_backend::directory_scopes::sort(&mut full);
    crate::engine_backend::directory_scopes::sort_choices(&mut choices);
    let DirectoryScopeRows::Full(rows) = full.branches else {
        panic!("full projection changed kind");
    };
    let ids = rows
        .iter()
        .map(|row| row.branch.directory_branch_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["a", "z", "older"]);
    assert_eq!(
        choices
            .iter()
            .map(|row| row.directory_branch_id.as_str())
            .collect::<Vec<_>>(),
        ids
    );
    assert_eq!(
        rows.iter().map(|row| row.current).collect::<Vec<_>>(),
        [false, true, false]
    );
    assert_eq!(
        choices.iter().map(|row| row.current).collect::<Vec<_>>(),
        [false, true, false]
    );
    let mut empty = DirectoryScopesSnapshot {
        repository_id: "empty".into(),
        branches: DirectoryScopeRows::Full(Vec::new()),
    };
    crate::engine_backend::directory_scopes::sort(&mut empty);
    crate::engine_backend::directory_scopes::sort_choices(&mut []);
}

fn directory_page(
    commit: &str,
    tree: &storage_engine::NamespaceTree<'_>,
    key: &str,
    after: Option<&str>,
) -> Result<DirectorySnapshot, ApiError> {
    DirectoryPagePlan::capture(commit, tree, key, after)?.finish()
}

fn capture_directory_page(engine: &Engine, branch: &str) -> DirectoryPagePlan {
    match DirectoryRead::capture(
        engine,
        crate::ReadDirectory {
            branch,
            entry_id: branch,
            path: None,
            selection: crate::DirectorySelection::Page {
                commit: None,
                after: None,
            },
        },
    )
    .unwrap()
    {
        DirectoryRead::Page(plan) => plan,
        DirectoryRead::Captured(_) => panic!("page request returned capture-only result"),
    }
}

#[test]
fn detached_directory_page_owns_only_capped_entries_and_preserves_encoded_bytes() {
    use smb_protocol::protocol::{MAX_DIRECTORY_PAGE_BYTES, MAX_DIRECTORY_PAGE_ENTRIES, Response};
    let (_root, mut engine, repo, _) = fixture();
    let entries: Vec<_> = (0..300)
        .map(|index| directory_entry(index, format!("f{index:05}-{}", "界".repeat(60))))
        .collect();
    let head = plant_head_namespace(&mut engine, &repo, &entries);
    // Independent whole-envelope oracle: trim a count-bounded prefix until
    // the original false-valued response envelope fits, then mark continuation.
    let mut expected = DirectorySnapshot {
        commit_id: head,
        entries: entries[..MAX_DIRECTORY_PAGE_ENTRIES].to_vec(),
        more: false,
    };
    while json_codec::to_vec(&Response::Ok {
        result: json_codec::to_value(&expected).unwrap(),
    })
    .unwrap()
    .len()
        > MAX_DIRECTORY_PAGE_BYTES
    {
        expected.entries.pop().unwrap();
    }
    expected.more = expected.entries.len() < entries.len();
    let plan = capture_directory_page(&engine, &repo.main_branch_id);
    assert_eq!(plan.children.len(), MAX_DIRECTORY_PAGE_ENTRIES + 1);
    drop(engine);
    let page = plan.finish().unwrap();
    assert_eq!(
        json_codec::to_vec(&page).unwrap(),
        json_codec::to_vec(&expected).unwrap()
    );
    assert!(page.more);
    assert!(page.entries.len() <= MAX_DIRECTORY_PAGE_ENTRIES);
}

#[test]
fn detached_directory_page_keeps_snapshot_and_checks_current_authority_before_result() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "before.txt", b"first");
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let plan = capture_directory_page(&backend.engine.lock().unwrap(), &repo.main_branch_id);
    let old_commit = plan.commit_id.clone();
    save_projection_file(
        &mut backend.engine.lock().unwrap(),
        &repo.main_branch_id,
        "after.txt",
        b"second",
    );
    let page = backend
        .finish_directory_page(&identity, &repo.main_branch_id, plan.finish())
        .unwrap();
    assert_eq!(page.commit_id, old_commit);
    assert_eq!(
        page.entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        vec!["before.txt"]
    );
    let pending = capture_directory_page(&backend.engine.lock().unwrap(), &repo.main_branch_id);
    auth.logout(&token).unwrap();
    assert_eq!(
        backend
            .finish_directory_page(&identity, &repo.main_branch_id, pending.finish())
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    assert_eq!(
        backend
            .finish_directory_page(
                &identity,
                &repo.main_branch_id,
                Err(ApiError::limit("budget failure"))
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
}

#[test]
fn detached_directory_completion_rechecks_share_generation_and_branch_migration() {
    let (_root, engine, repo, identity) = fixture();
    let backend = EngineBackend::new(engine);
    let mut bound = identity.clone();
    bound.smb_binding = Some(crate::auth::SmbBinding {
        repository_id: repo.repository_id.clone(),
        branch_id: repo.main_branch_id.clone(),
        generation: repo.smb_binding_generation,
    });
    backend
        .authorize_current(&bound, &repo.main_branch_id)
        .unwrap();
    let plan = capture_directory_page(&backend.engine.lock().unwrap(), &repo.main_branch_id);
    {
        let mut engine = backend.engine.lock().unwrap();
        let mut state = engine.state().clone();
        state
            .repositories
            .get_mut(&repo.repository_id)
            .unwrap()
            .smb_binding_generation += 1;
        storage_engine::test_support::replace_state(&mut engine, state);
    }
    assert_eq!(
        backend
            .finish_directory_page(&bound, &repo.main_branch_id, plan.finish())
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::SmbRebindRequired
    );
    let plan = capture_directory_page(&backend.engine.lock().unwrap(), &repo.main_branch_id);
    {
        let mut engine = backend.engine.lock().unwrap();
        let mut state = engine.state().clone();
        state.branches.get_mut(&repo.main_branch_id).unwrap().state =
            storage_contract::BranchState::Migrated;
        storage_engine::test_support::replace_state(&mut engine, state);
    }
    assert_eq!(
        backend
            .finish_directory_page(&identity, &repo.main_branch_id, plan.finish())
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::TreeMoved
    );
    assert_eq!(
        backend
            .finish_directory_page(
                &identity,
                &repo.main_branch_id,
                Err(ApiError::limit("budget failure"))
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::TreeMoved
    );
}

#[test]
fn detached_directory_budget_failure_is_preserved_for_current_authority() {
    let (_root, engine, repo, identity) = fixture();
    let backend = EngineBackend::new(engine);
    // Exercise the byte-budget error directly without admitting an oversized
    // stored record or changing namespace validation for this test.
    let mut oversized = directory_entry(0, "file.txt".into());
    oversized.entry_id = "x".repeat(smb_protocol::protocol::MAX_DIRECTORY_PAGE_BYTES);
    let plan = DirectoryPagePlan {
        commit_id: "captured".into(),
        children: vec![oversized],
    };
    let error = backend
        .finish_directory_page(&identity, &repo.main_branch_id, plan.finish())
        .unwrap_err();
    assert_eq!(error.code, ApiError::limit("unused").code);
    assert_eq!(error.message, "directory entry exceeds page byte budget");
}
