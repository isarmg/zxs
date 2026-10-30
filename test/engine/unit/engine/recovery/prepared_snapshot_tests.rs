use super::*;

fn publish(engine: &mut Engine, name: &str) -> OperationId {
    let operation = OperationId::generate();
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse(name).unwrap(),
            "P01",
            1,
            "admin",
            operation,
        )
        .unwrap();
    operation
}

#[test]
fn prepared_latest_root_preserves_full_checkpoint_and_suffix_replay() {
    for checkpoint in ["absent", "boundary", "suffix"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("system");
        let mut engine =
            Engine::initialize(&root, Config::development([temp.path().join("primary")])).unwrap();
        let first = publish(&mut engine, "first");
        if checkpoint == "suffix" {
            drop(engine);
            engine = Engine::open(&root).unwrap();
        }
        let latest = publish(&mut engine, "latest");
        let config = engine.config.clone();
        let expected = canonical_json(engine.state()).unwrap();
        let sequence = engine.state.sequence;
        let first_result = engine.operation(first).unwrap();
        let latest_result = engine.operation(latest).unwrap();
        drop(engine);
        if checkpoint == "boundary" {
            drop(Engine::open(&root).unwrap());
        }
        let journal = log_path(&config.disks[0]);
        let before = std::fs::read(&journal).unwrap();
        let mut engine = Engine::prepare_workspace_recovery(root, config).unwrap();
        let prepared = engine.prepare_recovery().unwrap();
        let (prepared_sequence, paired) = prepared.candidates.as_ref().unwrap();
        assert_eq!(*prepared_sequence, sequence);
        assert_eq!(paired.evidence.candidate.operation_id, latest);
        match checkpoint {
            "absent" => assert!(prepared.checkpoint.is_none()),
            "boundary" => assert_eq!(prepared.checkpoint.as_ref().unwrap().sequence, sequence),
            "suffix" => assert!(prepared.checkpoint.as_ref().unwrap().sequence < sequence),
            _ => unreachable!(),
        }
        assert_eq!(
            canonical_json(
                &prepared
                    .workspace_receipt(&engine, latest)
                    .unwrap()
                    .unwrap()
                    .result
            )
            .unwrap(),
            canonical_json(&latest_result).unwrap(),
        );
        engine.seed_workspace_recovery(&prepared).unwrap();
        SNAPSHOT_LOAD_COUNT.with(|count| count.set(0));
        engine
            .apply_recovery(RecoveryMode::CompletePending, prepared)
            .unwrap();
        assert_eq!(canonical_json(engine.state()).unwrap(), expected);
        assert_eq!(
            canonical_json(&engine.operation(first).unwrap()).unwrap(),
            canonical_json(&first_result).unwrap()
        );
        assert_eq!(
            canonical_json(&engine.operation(latest).unwrap()).unwrap(),
            canonical_json(&latest_result).unwrap()
        );
        assert_eq!(std::fs::read(journal).unwrap(), before);
        SNAPSHOT_LOAD_COUNT
            .with(|count| assert_eq!(count.get(), if checkpoint == "boundary" { 1 } else { 2 }));
    }
}

#[test]
fn prepared_root_still_admits_every_referenced_dependency() {
    for (dependency, missing) in [
        ("state-page", false),
        ("state-page", true),
        ("state-control-auth", false),
        ("state-control-auth", true),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("system");
        let mut engine =
            Engine::initialize(&root, Config::development([temp.path().join("primary")])).unwrap();
        publish(&mut engine, "published");
        let config = engine.config.clone();
        let snapshot: Snapshot = load_object(
            &config.disks[0],
            "snapshot",
            engine.snapshot.as_ref().unwrap(),
        )
        .unwrap();
        let hash = if dependency == "state-page" {
            crate::journal::map_pages(&config.disks[0], &snapshot.maps["repositories"])[0].clone()
        } else {
            snapshot.control_auth.hash().to_owned()
        };
        drop(engine);
        let damaged = object_path(&config.disks[0], dependency, &hash);
        if missing {
            std::fs::remove_file(&damaged).unwrap();
        } else {
            std::fs::write(&damaged, b"corrupt dependency").unwrap();
        }
        let journal = log_path(&config.disks[0]);
        let before = std::fs::read(&journal).unwrap();
        let mut engine = Engine::prepare_workspace_recovery(root, config).unwrap();
        let prepared = engine.prepare_recovery().unwrap();
        assert!(prepared.candidates.is_some());
        engine.seed_workspace_recovery(&prepared).unwrap();
        assert!(
            engine
                .apply_recovery(RecoveryMode::CompletePending, prepared)
                .is_err()
        );
        assert_eq!(engine.state.sequence, 0);
        assert_eq!(std::fs::read(journal).unwrap(), before);
        if missing {
            assert!(!damaged.exists());
        } else {
            assert_eq!(std::fs::read(damaged).unwrap(), b"corrupt dependency");
        }
    }
}

#[test]
fn corrupt_latest_root_is_rejected_during_read_only_preparation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let mut engine =
        Engine::initialize(&root, Config::development([temp.path().join("primary")])).unwrap();
    publish(&mut engine, "published");
    let config = engine.config.clone();
    let snapshot = engine.snapshot.clone().unwrap();
    drop(engine);
    let damaged = object_path(&config.disks[0], "snapshot", &snapshot);
    std::fs::write(&damaged, b"corrupt root").unwrap();
    let journal = log_path(&config.disks[0]);
    let before = std::fs::read(&journal).unwrap();
    let mut engine = Engine::prepare_workspace_recovery(root, config).unwrap();
    assert!(engine.prepare_recovery().is_err());
    assert!(engine.journals.is_empty());
    assert_eq!(engine.state.sequence, 0);
    assert_eq!(std::fs::read(journal).unwrap(), before);
    assert_eq!(std::fs::read(damaged).unwrap(), b"corrupt root");
}

#[test]
fn pending_latest_decision_does_not_take_the_last_sealed_root() {
    for mode in [RecoveryMode::PreservePending, RecoveryMode::CompletePending] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("system");
        let mut engine =
            Engine::initialize(&root, Config::development([temp.path().join("primary")])).unwrap();
        let published = publish(&mut engine, "published");
        let published_state = canonical_json(engine.state()).unwrap();
        let published_sequence = engine.state.sequence;
        let pending = OperationId::generate();
        engine.inject_fault_once("after_accept_1");
        assert!(
            engine
                .create_repository(
                    &storage_contract::RepositoryName::parse("pending").unwrap(),
                    "P01",
                    1,
                    "admin",
                    pending,
                )
                .is_err()
        );
        let config = engine.config.clone();
        drop(engine);
        let journal = log_path(&config.disks[0]);
        let before = std::fs::read(&journal).unwrap();
        let mut engine = Engine::prepare_workspace_recovery(root, config).unwrap();
        let prepared = engine.prepare_recovery().unwrap();
        let (sequence, paired) = prepared.candidates.as_ref().unwrap();
        assert_eq!(*sequence, published_sequence);
        assert_eq!(paired.evidence.candidate.operation_id, published);
        assert!(
            prepared
                .workspace_receipt(&engine, pending)
                .unwrap()
                .is_none()
        );
        engine.seed_workspace_recovery(&prepared).unwrap();
        engine.apply_recovery(mode, prepared).unwrap();
        match mode {
            RecoveryMode::PreservePending => {
                assert!(engine.blocked);
                assert_eq!(canonical_json(engine.state()).unwrap(), published_state);
                assert_eq!(std::fs::read(journal).unwrap(), before);
            }
            RecoveryMode::CompletePending => {
                assert!(!engine.blocked);
                assert_eq!(engine.state.repositories.len(), 2);
                assert_eq!(
                    engine.operation(pending).unwrap().commit_state,
                    storage_contract::CommitState::Published
                );
                assert!(std::fs::read(journal).unwrap().starts_with(&before));
            }
        }
    }
}

#[test]
fn schema_admitted_root_and_raw_entry_share_full_state_admission() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let mut engine =
        Engine::initialize(&root, Config::development([temp.path().join("primary")])).unwrap();
    publish(&mut engine, "published");
    let disk = &engine.config.disks[0];
    let snapshot: Snapshot =
        load_object(disk, "snapshot", engine.snapshot.as_ref().unwrap()).unwrap();
    let expected = canonical_json(engine.state()).unwrap();
    let admitted = SchemaValidatedSnapshot::new(snapshot.clone()).unwrap();
    assert_eq!(admitted.snapshot().sequence, snapshot.sequence);
    SNAPSHOT_LOAD_COUNT.with(|count| count.set(0));
    let prepared = admitted
        .resolve(|kind, hash| load_authenticated_object(disk, kind, hash))
        .unwrap();
    let raw = resolve_admitted_snapshot(snapshot.clone(), |kind, hash| {
        load_authenticated_object(disk, kind, hash)
    })
    .unwrap();
    assert_eq!(canonical_json(prepared.state()).unwrap(), expected);
    assert_eq!(canonical_json(raw.state()).unwrap(), expected);
    SNAPSHOT_LOAD_COUNT.with(|count| assert_eq!(count.get(), 2));

    for extra in [false, true] {
        let mut malformed = snapshot.clone();
        if extra {
            malformed
                .maps
                .insert("unknown-map".into(), snapshot.maps["plans"].clone());
        } else {
            malformed.maps.remove("plans");
        }
        let error = SchemaValidatedSnapshot::new(malformed.clone())
            .err()
            .unwrap();
        assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
        let error = resolve_admitted_snapshot(malformed, |_, _| {
            panic!("invalid root schema must reject before reading dependencies")
        })
        .err()
        .unwrap();
        assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
    }
}

fn namespace_handoff_fixture(
    engine: &mut Engine,
) -> (String, storage_contract::NamespaceReference) {
    publish(engine, "files");
    let branch = engine.state.branches.keys().next().unwrap().clone();
    let workspace = engine.create_workspace(&branch, "admin").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "proof.txt", b"retained proof")
        .unwrap();
    engine
        .commit_workspace(storage_contract::CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "proof".into(),
            request_digest: None,
        })
        .unwrap();
    let head = &engine.state.branches[&branch].head_commit_id;
    (branch, engine.state.commits[head].namespace.root.clone())
}

#[test]
fn fresh_normal_open_retains_admitted_namespace_but_offline_and_in_place_do_not() {
    for mode in ["normal", "offline", "in-place"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("system");
        let mut engine =
            Engine::initialize(&root, Config::development([temp.path().join("primary")])).unwrap();
        let (branch, reference) = namespace_handoff_fixture(&mut engine);
        let config = engine.config.clone();
        drop(engine);
        let engine = match mode {
            "normal" => Engine::open(&root).unwrap(),
            "offline" => Engine::open_for_recovery(&root).unwrap(),
            "in-place" => {
                let mut engine =
                    Engine::prepare_workspace_recovery(root.clone(), config.clone()).unwrap();
                engine.recover_mode(RecoveryMode::CompletePending).unwrap();
                engine
            }
            _ => unreachable!(),
        };
        let path = object_path(&config.disks[0], reference.kind.as_str(), &reference.hash);
        std::fs::write(&path, b"changed after successful admission").unwrap();
        let result = engine.namespace_tree(&branch).unwrap().get("proof.txt");
        if mode == "normal" {
            assert_eq!(result.unwrap().unwrap().path(), "proof.txt");
        } else {
            assert!(
                result.is_err(),
                "non-normal opening must not receive admission cache"
            );
        }
        // No ordinary cached proof exempts a new independent admission.
        drop(engine);
        assert!(Engine::open(&root).is_err());
    }
}

#[test]
fn namespace_seed_rejects_changed_state_resources_cache_and_snapshot_owners() {
    use crate::namespace::NamespaceNode;
    use crate::segment_store::{MAX_CACHED_NAMESPACE_BYTES, NamespaceCache, NamespaceCacheKey};
    use std::sync::Arc;
    use storage_contract::{NamespacePage, NamespaceRefKind, NamespaceReference};
    for changed in [
        "state",
        "resources",
        "cache",
        "snapshot",
        "blocked",
        "unchanged",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("system");
        let mut engine =
            Engine::initialize(&root, Config::development([temp.path().join("primary")])).unwrap();
        publish(&mut engine, "proof");
        let reference = NamespaceReference {
            kind: NamespaceRefKind::Page,
            hash: "a".repeat(64),
        };
        let node = Arc::new(NamespacePage {
            format: Default::default(),
            entries: Vec::new(),
        });
        let weak = Arc::downgrade(&node);
        let mut cache = NamespaceCache::new(usize::MAX, MAX_CACHED_NAMESPACE_BYTES);
        cache.insert(
            NamespaceCacheKey::parse(&reference).unwrap(),
            NamespaceNode::Page(node),
            1,
        );
        let seed = RecoveryNamespaceSeed::new(&engine, engine.snapshot.clone().unwrap(), cache);
        match changed {
            "state" => engine.state = Arc::new((*engine.state).clone()),
            "resources" => {
                engine.disk_resources =
                    Arc::new(DiskResources::open(&engine.config, false).unwrap())
            }
            "cache" => engine.segment_caches = Arc::new(crate::segment_store::SegmentCaches::new()),
            "snapshot" => engine.snapshot = Some("b".repeat(64)),
            "blocked" => engine.blocked = true,
            "unchanged" => {}
            _ => unreachable!(),
        }
        seed.install(&engine);
        assert_eq!(weak.upgrade().is_some(), changed == "unchanged");
    }
}

#[test]
fn completing_a_pending_candidate_does_not_retain_the_preceding_admission_cache() {
    use crate::namespace::NamespaceNode;
    use crate::segment_store::{NamespaceCache, NamespaceCacheKey, MAX_CACHED_NAMESPACE_BYTES};
    use std::sync::Arc;
    use storage_contract::{NamespacePage, NamespaceRefKind, NamespaceReference};

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let mut engine =
        Engine::initialize(&root, Config::development([temp.path().join("primary")])).unwrap();
    namespace_handoff_fixture(&mut engine);
    let config = engine.config.clone();
    engine.inject_fault_once("after_accept_1");
    assert!(
        engine
            .create_repository(
                &storage_contract::RepositoryName::parse("pending").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate(),
            )
            .is_err()
    );
    drop(engine);
    for pending in [true, false] {
        let mut engine = Engine::prepare_workspace_recovery(root.clone(), config.clone()).unwrap();
        let marker = Arc::new(NamespacePage {
            format: Default::default(),
            entries: Vec::new(),
        });
        let weak = Arc::downgrade(&marker);
        let reference = NamespaceReference {
            kind: NamespaceRefKind::Page,
            hash: "a".repeat(64),
        };
        let mut cache = NamespaceCache::new(usize::MAX, MAX_CACHED_NAMESPACE_BYTES);
        cache.insert(
            NamespaceCacheKey::parse(&reference).unwrap(),
            NamespaceNode::Page(marker),
            1,
        );
        engine.segment_caches.replace_namespace(cache);
        engine.recover_for_open(RecoveryMode::CompletePending).unwrap();
        assert!(!engine.blocked);
        assert_eq!(engine.state.repositories.len(), 2);
        // Pending completion legitimately fills the ordinary cache while
        // projecting container names. Only an admission-cache replacement
        // removes this test-owned marker; ordinary inserts stay far below 8 MiB.
        assert_eq!(weak.upgrade().is_some(), pending);
        // The second fresh replay is now fully published and must replace it.
    }
}
