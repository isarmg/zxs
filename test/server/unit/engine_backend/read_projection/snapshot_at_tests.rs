use super::*;
use storage_contract::{Entry, NamespaceEntryKind, NamespaceOperation, NamespaceOperationKind};

#[test]
fn shared_snapshot_keeps_exact_commit_after_new_publication_and_engine_drop() {
    let (_root, mut engine, repo, identity) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "Café.txt", b"first");
    let commit = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let original = engine.list_tree(&repo.main_branch_id).unwrap()[0].clone();
    let backend = EngineBackend::new(engine);
    let frozen = backend
        .snapshot_at(&identity, &repo.main_branch_id, &commit)
        .unwrap();
    // The frozen commit's bounded point probe resolves the exact entry.
    assert_eq!(
        backend
            .snapshot_entry(
                &identity,
                &repo.main_branch_id,
                &commit,
                original.lookup_key()
            )
            .unwrap(),
        Some(original.clone())
    );
    {
        let mut engine = backend.engine.lock().unwrap();
        assert!(std::ptr::eq(
            frozen.as_ref(),
            &engine.state().commits[&commit]
        ));
        assert_eq!(frozen.namespace, engine.state().commits[&commit].namespace);
        save_projection_file(&mut engine, &repo.main_branch_id, "Café.txt", b"second");
        assert_ne!(
            engine.list_tree(&repo.main_branch_id).unwrap()[0].version_id,
            original.version_id
        );
    }
    // The frozen commit still names its immutable namespace after publication.
    assert_eq!(
        backend
            .snapshot_entry(
                &identity,
                &repo.main_branch_id,
                &commit,
                original.lookup_key()
            )
            .unwrap(),
        Some(original.clone())
    );
    let same = backend
        .snapshot_at(&identity, &repo.main_branch_id, &commit)
        .unwrap();
    assert!(Arc::ptr_eq(&same, &frozen));
    drop(same);
    drop(backend);
    assert_eq!(frozen.commit_id, commit);
    assert_eq!(frozen.namespace.entries, 1);
}

#[test]
fn shared_snapshot_preserves_tree_errors_and_current_identity_admission() {
    let (root, mut engine, repo, _) = fixture();
    let commit = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let other = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Other").unwrap(),
            "P01",
            3,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let foreign = engine.state().branches[&other.main_branch_id]
        .head_commit_id
        .clone();
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    for (branch, selected) in [
        (repo.main_branch_id.as_str(), foreign.as_str()),
        (repo.main_branch_id.as_str(), "missing-commit"),
        ("missing-branch", commit.as_str()),
    ] {
        let old = backend
            .tree_snapshot(
                &identity,
                crate::tests::tree_request(
                    &(json!({"branch_id":branch,"query":{"commit_id":selected}})),
                ),
            )
            .map(|snapshot| json_codec::to_value(snapshot).unwrap())
            .unwrap_err();
        let new = backend
            .snapshot_at(&identity, branch, selected)
            .unwrap_err();
        assert_eq!(
            (new.status, new.code, new.message, new.retryable),
            (old.status, old.code, old.message, old.retryable)
        );
    }
    let frozen = backend
        .snapshot_at(&identity, &repo.main_branch_id, &commit)
        .unwrap();
    auth.logout(&token).unwrap();
    for (branch, selected) in [
        (repo.main_branch_id.as_str(), commit.as_str()),
        ("missing-branch", "missing-commit"),
    ] {
        assert_eq!(
            backend
                .snapshot_at(&identity, branch, selected)
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::Unauthorized
        );
    }
    assert_eq!(frozen.commit_id, commit);
}

#[test]
fn shared_snapshot_rechecks_smb_binding_before_selecting_commit() {
    let (_root, engine, repo, mut identity) = fixture();
    let commit = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    identity.smb_binding = Some(crate::auth::SmbBinding {
        repository_id: repo.repository_id.clone(),
        branch_id: repo.main_branch_id.clone(),
        generation: repo.smb_binding_generation + 1,
    });
    let backend = EngineBackend::new(engine);
    for (branch, selected) in [
        (repo.main_branch_id.as_str(), commit.as_str()),
        ("missing-branch", "missing-commit"),
    ] {
        assert_eq!(
            backend
                .snapshot_at(&identity, branch, selected)
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::SmbRebindRequired
        );
    }
}

#[test]
fn shared_snapshot_preserves_archived_reads_and_migrated_branch_rejection() {
    let (_root, mut engine, repo, identity) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"first");
    let cut = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let archived = engine
        .create_branch(
            &cut,
            &storage_contract::BranchName::parse("Archive").unwrap(),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    engine
        .archive_branch(
            &archived.branch_id,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let backend = EngineBackend::new(engine);
    let frozen = backend
        .snapshot_at(&identity, &archived.branch_id, &archived.head_commit_id)
        .unwrap();
    let tree = backend
        .tree_snapshot(&identity, crate::tests::tree_request(&(json!({"branch_id":archived.branch_id,"query":{"commit_id":archived.head_commit_id}})))).map(|snapshot| json_codec::to_value(snapshot).unwrap())
        .unwrap();
    assert_eq!(tree["commit_id"].as_str().unwrap(), frozen.commit_id);
    assert_eq!(
        tree["entries"].as_array().unwrap().len() as u64,
        frozen.namespace.entries
    );
    {
        let mut engine = backend.engine.lock().unwrap();
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
    }
    let old = backend
        .tree_snapshot(
            &identity,
            crate::tests::tree_request(
                &(json!({"branch_id":repo.main_branch_id,"query":{"commit_id":cut}})),
            ),
        )
        .map(|snapshot| json_codec::to_value(snapshot).unwrap())
        .unwrap_err();
    let new = backend
        .snapshot_at(&identity, &repo.main_branch_id, &cut)
        .unwrap_err();
    assert_eq!(new.code, storage_contract::ErrorCode::TreeMoved);
    assert_eq!(
        (new.status, new.code, new.message),
        (old.status, old.code, old.message)
    );
    assert_eq!(frozen.commit_id, archived.head_commit_id);
}

fn publication_fact(entries: &[Entry], item: &str) -> (u64, usize) {
    let entry = entries.iter().find(|entry| entry.entry_id == item).unwrap();
    (entry.size, std::hint::black_box(&entry.version_id).len())
}

fn namespace_facts(entries: &[Entry], source: &str, target: &str) -> (bool, bool, bool) {
    let find = |key: &str| {
        entries
            .iter()
            .find(|entry| entry.lookup_key() == key)
            .unwrap()
    };
    let source = find(source);
    let target = find(target);
    let prefix = format!("{}/", target.lookup_key());
    (
        source.kind == target.kind,
        source.entry_id != target.entry_id,
        target.kind == NamespaceEntryKind::Directory
            && entries
                .iter()
                .any(|entry| entry.lookup_key().starts_with(&prefix)),
    )
}

// This isolates snapshot acquisition and the two existing read-side predicates.
// Fixture publication and all content/upload work remain outside the timing.
#[test]
#[ignore = "comparative immutable snapshot latency measurement"]
fn measure_shared_snapshot_against_publication_and_namespace_tree_round_trips() {
    use std::{hint::black_box, time::Instant};
    #[derive(Debug)]
    enum Consumer {
        Publication,
        Namespace,
    }
    for count in [10, 100, 1_000] {
        let (_root, mut engine, repo, identity) = fixture();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "owner")
            .unwrap();
        for path in ["source", "target", "target/child"]
            .into_iter()
            .map(str::to_owned)
            .chain((0..count - 4).map(|index| format!("directory-{index:05}")))
        {
            engine
                .namespace_operation(
                    &workspace.workspace_id,
                    NamespaceOperation::parse(NamespaceOperationKind::Mkdir, &path, None).unwrap(),
                )
                .unwrap();
        }
        engine
            .stage_bytes(&workspace.workspace_id, "target.txt", b"baseline")
            .unwrap();
        engine
            .commit_workspace(storage_engine::CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: storage_contract::OperationId::generate(),
                principal: "owner".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "snapshot fixture".into(),
                request_digest: None,
            })
            .unwrap();
        let commit = engine.state().branches[&repo.main_branch_id]
            .head_commit_id
            .clone();
        let entries = engine.list_tree(&repo.main_branch_id).unwrap();
        assert_eq!(entries.len(), count);
        let item = entries
            .iter()
            .find(|entry| entry.path() == "target.txt")
            .unwrap()
            .entry_id
            .clone();
        let expected_publication = publication_fact(&entries, &item);
        let expected_namespace = namespace_facts(&entries, "source", "target");
        let backend = EngineBackend::new(engine);
        let old = || {
            let tree = backend
                .tree_snapshot(
                    &identity,
                    crate::tests::tree_request(
                        &(json!({"branch_id":repo.main_branch_id,"query":{"commit_id":commit}})),
                    ),
                )
                .map(|snapshot| json_codec::to_value(snapshot).unwrap())
                .unwrap();
            json_codec::from_value::<Vec<Entry>>(tree["entries"].clone()).unwrap()
        };
        // The typed consumers are the SMB read paths: bounded point probes of
        // the frozen commit namespace, never a materialized snapshot copy.
        let probe = |key: &str| {
            backend
                .snapshot_entry(&identity, &repo.main_branch_id, &commit, key)
                .unwrap()
        };
        let typed_publication = || {
            let entry = probe("target.txt").unwrap();
            (entry.size, std::hint::black_box(&entry.version_id).len())
        };
        let typed_namespace = || {
            let source = probe("source").unwrap();
            let target = probe("target").unwrap();
            (
                source.kind == target.kind,
                source.entry_id != target.entry_id,
                target.kind == NamespaceEntryKind::Directory
                    && backend
                        .snapshot_directory_has_children(
                            &identity,
                            &repo.main_branch_id,
                            &commit,
                            "target",
                        )
                        .unwrap(),
            )
        };
        assert_eq!(publication_fact(&old(), &item), expected_publication);
        assert_eq!(typed_publication(), expected_publication);
        assert_eq!(
            namespace_facts(&old(), "source", "target"),
            expected_namespace
        );
        assert_eq!(typed_namespace(), expected_namespace);
        for consumer in [Consumer::Publication, Consumer::Namespace] {
            let legacy_inspect = |entries: &[Entry]| match consumer {
                Consumer::Publication => {
                    black_box(publication_fact(entries, &item));
                }
                Consumer::Namespace => {
                    black_box(namespace_facts(entries, "source", "target"));
                }
            };
            let typed_inspect = || match consumer {
                Consumer::Publication => {
                    black_box(typed_publication());
                }
                Consumer::Namespace => {
                    black_box(typed_namespace());
                }
            };
            let mut legacy = Vec::with_capacity(101);
            let mut typed = Vec::with_capacity(101);
            for sample in 0..101 {
                // Alternate ordering to avoid giving one path every warm-cache run.
                if sample % 2 == 0 {
                    let start = Instant::now();
                    legacy_inspect(&old());
                    legacy.push(start.elapsed().as_nanos());
                    let start = Instant::now();
                    typed_inspect();
                    typed.push(start.elapsed().as_nanos());
                } else {
                    let start = Instant::now();
                    typed_inspect();
                    typed.push(start.elapsed().as_nanos());
                    let start = Instant::now();
                    legacy_inspect(&old());
                    legacy.push(start.elapsed().as_nanos());
                }
            }
            legacy.sort_unstable();
            typed.sort_unstable();
            println!(
                "consumer={consumer:?} entries={count} samples=101 old_p50_ns={} old_p95_ns={} typed_p50_ns={} typed_p95_ns={}",
                legacy[50], legacy[95], typed[50], typed[95]
            );
        }
    }
}

#[test]
fn public_commit_snapshot_reuses_state_arc_and_serializes_after_unlock_and_revocation() {
    let (root, mut engine, repo, _) = fixture();
    save_projection_file(&mut engine, &repo.main_branch_id, "file.txt", b"first");
    let commit = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let frozen = backend.commit_snapshot(&identity, &commit).unwrap();
    {
        let engine = backend.engine.lock().unwrap();
        assert!(std::ptr::eq(
            frozen.as_ref(),
            &engine.state().commits[&commit]
        ));
        assert_eq!(frozen.namespace, engine.state().commits[&commit].namespace);
    }
    struct Probe<'a>(&'a EngineBackend, crate::commit_wire::CommitResponse);
    impl Serialize for Probe<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let available = std::thread::scope(|scope| {
                scope
                    .spawn(|| self.0.engine.try_lock().is_ok())
                    .join()
                    .unwrap()
            });
            if !available {
                return Err(S::Error::custom(
                    "commit serialization retained admission mutex",
                ));
            }
            self.1.serialize(serializer)
        }
    }
    let expected = json_codec::to_value(frozen.as_ref()).unwrap();
    auth.logout(&token).unwrap();
    assert_eq!(
        backend
            .commit_snapshot(&identity, &commit)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    // An admitted immutable response may finish; it grants no later request admission.
    let encoded = json_codec::to_value(Probe(
        &backend,
        crate::commit_wire::CommitResponse::from(frozen),
    ))
    .unwrap();
    assert_eq!(encoded, expected);
}
