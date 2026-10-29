use super::*;
use crate::engine_backend::field;
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

std::thread_local! {
    static TREE_PROJECTION_WORK: std::cell::Cell<(usize, usize)> = const {
        std::cell::Cell::new((0, 0))
    };
}

pub(super) fn record_tree_projection(entries: usize) {
    TREE_PROJECTION_WORK.with(|work| {
        let (projections, copied_entries) = work.get();
        work.set((projections + 1, copied_entries + entries));
    });
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
fn rendering_releases_engine_before_serializer_runs() {
    let (root, engine, repo, _) = fixture();
    let (backend, _auth, _token, identity) = authenticated_backend(engine, root.path());
    struct Probe<'a>(&'a EngineBackend, crate::TreeSnapshot);
    impl Serialize for Probe<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            // The real typed backend has returned before HTTP serializes; another
            // thread must be able to acquire its actual admission lock.
            let available = std::thread::scope(|scope| {
                scope
                    .spawn(|| self.0.engine.try_lock().is_ok())
                    .join()
                    .unwrap()
            });
            if !available {
                return Err(S::Error::custom("serialization retained Engine mutex"));
            }
            self.1.serialize(serializer)
        }
    }
    let snapshot = backend
        .tree_snapshot(
            &identity,
            crate::ReadTree {
                branch: &repo.main_branch_id,
                commit: None,
            },
        )
        .unwrap();
    let expected = json!({"branch_id":repo.main_branch_id,"commit_id":snapshot.commit_id,"entries":[],"pending_file_versions":{}});
    assert_eq!(
        json_codec::to_value(Probe(&backend, snapshot)).unwrap(),
        expected
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
    let projection =
        crate::TreeSnapshot::capture(&engine, crate::tests::tree_request(&input)).unwrap();
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
        crate::TreeSnapshot::capture(
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
    assert_eq!(json_codec::to_value(&projection).unwrap(), expected);
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    assert_eq!(
        backend
            .tree_snapshot(&identity, crate::tests::tree_request(&input))
            .map(|snapshot| json_codec::to_value(snapshot).unwrap())
            .unwrap(),
        expected
    );
    let denied = Identity {
        principal: "other".into(),
        ..identity.clone()
    };
    assert_eq!(
        backend
            .tree_snapshot(&denied, crate::tests::tree_request(&input))
            .map(|snapshot| json_codec::to_value(snapshot).unwrap())
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    let frozen = {
        let mut engine = backend.engine.lock().unwrap();
        let frozen =
            crate::TreeSnapshot::capture(&engine, crate::tests::tree_request(&input)).unwrap();
        save_projection_file(&mut engine, &repo.main_branch_id, "new.txt", b"new content");
        frozen
    };
    let current = backend
        .tree_snapshot(&identity, crate::tests::tree_request(&input))
        .map(|snapshot| json_codec::to_value(snapshot).unwrap())
        .unwrap();
    assert_ne!(current["commit_id"], expected["commit_id"]);
    assert_eq!(current["entries"].as_array().unwrap().len(), 1);
    auth.logout(&token).unwrap();
    // An admitted projection keeps its owned snapshot across live data changes
    // and revocation, but can never admit the session to a subsequent request.
    assert_eq!(json_codec::to_value(&frozen).unwrap(), expected);
    assert_eq!(
        backend
            .tree_snapshot(&identity, crate::tests::tree_request(&input))
            .map(|snapshot| json_codec::to_value(snapshot).unwrap())
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
        let projection = crate::TreeSnapshot::capture(
            &engine,
            crate::tests::tree_request(&json!({
                "branch_id": repo.main_branch_id, "query": {"commit_id": commit_id}
            })),
        )
        .unwrap();
        assert_eq!(projection.pending_file_versions, expected);
        assert_eq!(projection.entries, entries);
        assert_eq!(
            json_codec::to_value(&projection).unwrap(),
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
        let tree = crate::TreeSnapshot::capture(
            engine,
            crate::tests::tree_request(&json!({
                "branch_id": branch, "query": {"commit_id": commit_id}
            })),
        )
        .unwrap();
        assert_eq!(tree.pending_file_versions, legacy);
        assert_eq!(tree.entries, entries);
        assert_eq!(
            json_codec::to_value(&tree).unwrap(),
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
    let projection = crate::TreeSnapshot::capture(
        &engine,
        crate::tests::tree_request(&json!({
            "branch_id": repo.main_branch_id, "query": {"commit_id": head}
        })),
    )
    .unwrap();
    assert_eq!(projection.pending_file_versions, expected);
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
        let page = DirectorySnapshot::page(&head, &tree, "", after.as_deref()).unwrap();
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
    let page =
        DirectorySnapshot::page(&head, &tree, &name_policy::path_key(&parent), None).unwrap();
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
                }
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
