//! Independent membership and full-encoder oracles for transaction write sets.
use super::changes::HistoryChanges;
use super::*;
use json_codec::json;

fn record<T: data_serialization::de::DeserializeOwned>(value: json_codec::Value) -> T {
    json_codec::from_value(value).unwrap()
}

// Deliberately a dependency-only fixture: two histories share a repository,
// file and directory. Such repository sharing is not publishable under today's
// manifest validator, but invalidation must not assume that invariant to omit
// an owner. Actual valid publication graphs are tested separately below.
pub(super) fn dependency_fixture() -> State {
    let mut state = State::default();
    for (history, repository, branch, commit, version, file, directory) in [
        (
            "h1",
            "r1",
            "b1",
            "c1",
            "v1",
            "shared-file",
            "shared-directory",
        ),
        (
            "h2",
            "r1",
            "b2",
            "c2",
            "v2",
            "shared-file",
            "shared-directory",
        ),
        (
            "h3",
            "r2",
            "b3",
            "c3",
            "v3",
            "other-file",
            "other-directory",
        ),
    ] {
        state.histories.insert(
            history.into(),
            record(json!({
                "history_tree_id":history,"repository_id":repository,"root_commit_id":commit,
                "main_branch_id":branch,"generation":"1","state":"active","provenance":null
            })),
        );
        state.repositories.entry(repository.into()).or_insert_with(|| record(json!({
            "repository_id":repository,"name":repository,"history_tree_id":history,
            "main_branch_id":branch,"primary_slot_id":"P01","target_copies":1,
            "name_policy":"portable","unicode_version":"15.1","smb_enabled":false,"smb_binding_generation":"1"
        })));
        state.branches.insert(branch.into(), record(json!({
            "branch_id":branch,"history_tree_id":history,"name":branch,"head_commit_id":commit,
            "fork_commit_id":commit,"branch_kind":"file","file_id":file,"generation":"1","state":"active"
        })));
        let namespace = crate::namespace::fixture_store::namespace(&[Entry {
            entry_id: directory.into(),
            path: name_policy::PortablePath::parse("directory").unwrap(),
            kind: NamespaceEntryKind::Directory,
            file_id: String::new(),
            version_id: String::new(),
            size: 0,
        }]);
        state.commits.insert(commit.into(), record(json!({
            "commit_id":commit,"history_tree_id":history,"kind":"save","parent_commit_ids":[],
            "tree_id":"tree","sequence":"1","operation_id":"op_00000000000000000000000000000000","author":"author","message":"message",
            "namespace":json_codec::to_value(&namespace).unwrap(),"provenance":null
        })));
        state.versions.insert(version.into(), record(json!({
            "history_tree_id":history,"version_id":version,"file_id":file,"creation_primary_slot_id":"P01",
            "logical_size":"0","version_sequence":"1","content_hash":"a".repeat(64),"checksum_chunk_bytes":65536,
            "checksum_manifest_hash":"hash","creation_name":"file","storage_name":"file","created_at":"now",
            "source_version_ids":[],"created_as_full_checkpoint":true
        })));
    }
    for (branch, repository) in [("d1", "r1"), ("d3", "r2")] {
        state.directory_branches.insert(branch.into(), record(json!({
            "directory_branch_id":branch,"repository_id":repository,"directory_id":"shared-directory",
            "path":"directory","name":branch,"parent_branch_id":null,"head_tree_id":"t1","commit_id":"c1",
            "entries":[],"file_branches":{},"materialized":true,"sequence":"1"
        })));
    }
    state.directory_trees.insert(
        "t1".into(),
        record(json!({
            "tree_id":"t1","directory_branch_id":"d1","parent_tree_id":null,"commit_id":"c1",
            "path":"directory","entries":[],"file_branches":{},"sequence":"1"
        })),
    );
    state.file_contexts.insert(
        "r1/path".into(),
        record(json!({"branch_id":"b3","inherited_from":null})),
    );
    state.file_path_nodes.insert("node".into(), record(json!({
        "node_id":"node","history_tree_id":"h1","file_id":"shared-file","version_id":"v1",
        "parent_node_ids":[],"branch_id":"b1","path":"new","previous_path":"old","operation_id":"op_00000000000000000000000000000000",
        "author":"author","sequence":"1","created_at":"1"
    })));
    state.file_metadata_nodes.insert(
        "node".into(),
        record(json!({
            "node_id":"node","history_tree_id":"h2","file_id":"shared-file","version_id":"v2",
            "parent_node_ids":[],"branch_id":"b2","kind":"tags","tags":[],"operation_id":"op_00000000000000000000000000000000",
            "author":"author","sequence":"1","created_at":"1"
        })),
    );
    state.file_branch_merges.insert("merge".into(), record(json!({
        "merge_id":"merge","history_tree_id":"h1","file_id":"shared-file","input_branch_ids":[],
        "input_node_ids":[],"chosen_node_id":"node","result_node_id":"node","surviving_branch_id":"b1",
        "reused_node":true,"parent_node_ids":[],"operation_id":"op_00000000000000000000000000000000","sequence":"1"
    })));
    state.placements.insert(
        "v1".into(),
        record(json!({
            "version_id":"v1","primary_slot_id":"P01","target_copies":1,"route_slot_ids":["P01"],
            "topology_generation":"1","binding_generations":{"P01":"1"}
        })),
    );
    state.merge_links.insert("link".into(), record(json!({
        "merge_link_id":"link","history_tree_id":"h1","source_branch_id":"b1","source_commit_id":"c1",
        "target_branch_id":"b1","target_commit_id":"c1","author":"author","operation_id":"op_00000000000000000000000000000000"
    })));
    state.history_boundaries.insert("boundary".into(), record(json!({
        "boundary_id":"boundary","history_tree_id":"h1","local_commit_id":"c1","local_branch_id":null,
        "relation":"split","external_history_tree_id":"h3","external_commit_id":"c3","source_merge_link_id":null
    })));
    state.placement_rules.insert("rule".into(), record(json!({
        "rule_id":"rule","repository_id":"r1","path_prefix":"","primary_slot_id":"P01","generation":"1"
    })));
    state.protection_policies.insert("policy".into(), record(json!({
        "policy_id":"policy","repository_id":"r1","path_prefix":"","target_copies":1,"inheritable":true,
        "generation":"1"
    })));
    state
        .file_selections
        .insert("shared-file".into(), "b1".into());
    state.file_branch_aliases.insert("b1".into(), "b3".into());
    state.file_node_parents.insert("v1".into(), vec![]);
    state.file_path_notes.insert("node".into(), "note".into());
    state
        .directory_tree_selections
        .insert("shared-directory".into(), "t1".into());
    state
        .directory_selections
        .insert("shared-directory".into(), "d1".into());
    state
        .directory_selection_generations
        .insert("shared-directory".into(), 1);
    state.directory_selection_generations.insert("r1".into(), 1);
    state
        .file_selection_generations
        .insert("shared-file".into(), 1);
    state
        .file_write_origins
        .insert("shared-file".into(), "d1".into());
    state
        .file_locations
        .insert("shared-file".into(), "path".into());
    state
        .file_backup_paths
        .insert("shared-file".into(), BTreeMap::new());
    state
        .file_backup_policies
        .insert("shared-file".into(), "policy".into());
    state.file_notes.insert("shared-file".into(), "note".into());
    state.version_notes.insert("v1".into(), "note".into());
    state.file_names.insert("shared-file".into(), "file".into());
    state
        .content_states
        .insert("v1".into(), storage_contract::ContentState::Available);
    state
}

fn assert_changes(before: &State, expected: &[&str], change: impl FnOnce(&mut State)) {
    let mut after = before.clone();
    after.begin_tracking();
    change(&mut after);
    assert_eq!(
        HistoryChanges::between(
            before,
            &after,
            &crate::namespace::fixture_store::FixtureSource
        )
        .histories,
        expected
            .iter()
            .map(|id| (*id).to_owned())
            .collect::<BTreeSet<_>>()
    );
}

#[test]
fn every_manifest_map_deletion_uses_its_independently_specified_membership() {
    let before = dependency_fixture();
    macro_rules! removed {
        ($field:ident, $key:literal, $expected:expr) => {
            assert_changes(&before, $expected, |after| {
                assert!(after.$field.remove($key).is_some(), stringify!($field));
            });
        };
    }
    removed!(histories, "h1", &["h1"]);
    removed!(repositories, "r1", &["h1", "h2"]);
    removed!(branches, "b1", &["h1"]);
    removed!(commits, "c1", &["h1"]);
    removed!(versions, "v1", &["h1"]);
    removed!(file_path_nodes, "node", &["h1"]);
    removed!(file_metadata_nodes, "node", &["h2"]);
    removed!(file_branch_merges, "merge", &["h1"]);
    removed!(merge_links, "link", &["h1"]);
    removed!(history_boundaries, "boundary", &["h1"]);
    removed!(directory_branches, "d1", &["h1", "h2"]);
    removed!(placement_rules, "rule", &["h1", "h2"]);
    removed!(protection_policies, "policy", &["h1", "h2"]);
    removed!(placements, "v1", &["h1"]);
    removed!(content_states, "v1", &["h1"]);
    removed!(file_node_parents, "v1", &["h1"]);
    removed!(version_notes, "v1", &["h1"]);
    removed!(file_write_origins, "shared-file", &["h1", "h2"]);
    removed!(file_locations, "shared-file", &["h1", "h2"]);
    removed!(file_backup_paths, "shared-file", &["h1", "h2"]);
    removed!(file_backup_policies, "shared-file", &["h1", "h2"]);
    removed!(file_selection_generations, "shared-file", &["h1", "h2"]);
    removed!(file_notes, "shared-file", &["h1", "h2"]);
    removed!(file_names, "shared-file", &["h1", "h2"]);
    removed!(directory_tree_selections, "shared-directory", &["h1", "h2"]);
    removed!(
        directory_selection_generations,
        "shared-directory",
        &["h1", "h2"]
    );
    removed!(directory_selection_generations, "r1", &["h1", "h2"]);
    // Context ownership is the key prefix, not its b3 value. Alias ownership
    // is its b1 source, not its b3 target. Path notes take the union of nodes.
    removed!(file_contexts, "r1/path", &["h1", "h2"]);
    removed!(file_branch_aliases, "b1", &["h1"]);
    removed!(file_path_notes, "node", &["h1", "h2"]);
    removed!(file_selections, "shared-file", &["h1"]);
    removed!(directory_trees, "t1", &["h1", "h2"]);
    removed!(directory_selections, "shared-directory", &["h1", "h2"]);
}

#[test]
fn migration_retargeting_and_simultaneous_deletion_keep_old_and_new_owners() {
    let before = dependency_fixture();
    macro_rules! history_move {
        ($field:ident, $key:literal, $old:literal) => {
            assert_changes(&before, &[$old, "h3"], |after| {
                after.$field.get_mut($key).unwrap().history_tree_id = "h3".into()
            });
        };
    }
    history_move!(branches, "b1", "h1");
    history_move!(commits, "c1", "h1");
    history_move!(versions, "v1", "h1");
    history_move!(file_path_nodes, "node", "h1");
    history_move!(file_metadata_nodes, "node", "h2");
    history_move!(file_branch_merges, "merge", "h1");
    history_move!(merge_links, "link", "h1");
    history_move!(history_boundaries, "boundary", "h1");
    assert_changes(&before, &["h1"], |after| {
        after.histories.get_mut("h1").unwrap().repository_id = "r2".into()
    });
    assert_changes(&before, &["h1", "h2", "h3"], |after| {
        after
            .directory_branches
            .get_mut("d1")
            .unwrap()
            .repository_id = "r2".into()
    });
    assert_changes(&before, &["h1", "h2", "h3"], |after| {
        after.placement_rules.get_mut("rule").unwrap().repository_id = "r2".into()
    });
    assert_changes(&before, &["h1", "h2", "h3"], |after| {
        after
            .protection_policies
            .get_mut("policy")
            .unwrap()
            .repository_id = "r2".into()
    });
    assert_changes(&before, &["h1", "h3"], |after| {
        after
            .file_selections
            .insert("shared-file".into(), "b3".into());
    });
    assert_changes(&before, &["h1"], |after| {
        after.file_branch_aliases.insert("b1".into(), "b2".into());
    });
    assert_changes(&before, &["h1", "h2", "h3"], |after| {
        after
            .directory_trees
            .get_mut("t1")
            .unwrap()
            .directory_branch_id = "d3".into()
    });
    assert_changes(&before, &["h1", "h2", "h3"], |after| {
        after
            .directory_selections
            .insert("shared-directory".into(), "d3".into());
    });
    assert_changes(&before, &["h1", "h2", "h3"], |after| {
        let context = after.file_contexts.remove("r1/path").unwrap();
        after.file_contexts.insert("r2/path".into(), context);
    });
    assert_changes(&before, &["h1", "h2"], |after| {
        after.versions.remove_discard("v1");
        after.file_notes.remove("shared-file");
    });
    assert_changes(&before, &["h1", "h2"], |after| {
        after.commits.remove_discard("c1");
        after
            .directory_selection_generations
            .remove("shared-directory");
    });
}

#[test]
fn replacement_all_and_lost_lineage_are_conservative_but_control_state_is_independent() {
    let before = dependency_fixture();
    assert_changes(&before, &["h1", "h2", "h3"], |after| {
        after.file_notes = Default::default()
    });
    assert_changes(&before, &["h1", "h2", "h3"], |after| {
        after.versions = Default::default()
    });
    assert_changes(&before, &["h1", "h2", "h3"], |after| {
        after
            .file_names
            .values_mut()
            .for_each(|name| name.push('!'));
    });
    assert_changes(&before, &["h1", "h2", "h3"], |after| {
        after.protection_policies.retain(|_, _| true)
    });
    assert_changes(&before, &["h1", "h2", "h3"], |after| {
        after
            .file_notes
            .insert("shared-file".into(), "changed".into());
        after.begin_tracking();
    });
    let decoded: State = json_codec::from_slice(&canonical_json(&before).unwrap()).unwrap();
    assert_eq!(
        HistoryChanges::between(
            &before,
            &decoded,
            &crate::namespace::fixture_store::FixtureSource
        )
        .histories,
        before.histories.keys().cloned().collect()
    );
    assert_changes(&before, &[], |after| {
        after.sequence += 1;
        after.business_generation += 1;
        after.topology_generation += 1;
        after.control_auth = Some(json!({"revision": "new"}));
        after
            .events
            .push(Event {
                sequence: 9,
                operation_id: "op_00000000000000000000000000000000".parse().unwrap(),
                kind: "file.save".into(),
                object_id: "h1".into(),
            })
            .unwrap();
        after.operations = Default::default();
        after.plans = Default::default();
        after
            .operations
            .push(storage_contract::OperationRecordEntry {
                principal: "author".into(),
                result: OperationResult {
                    operation_id: OperationId::generate(),
                    kind: "file.save".into(),
                    commit_id: None,
                    history_tree_id: None,
                    branch_id: None,
                    commit_state: CommitState::Published,
                    protection_at_publish: ProtectionAtPublish::NotApplicable,
                    current_health: CurrentHealth::Healthy,
                    request_digest: "digest".into(),
                    sequence: 9,
                    result: json!({}),
                },
            })
            .unwrap();
    });
}

fn save_oracle_file(engine: &mut Engine, branch: &str) -> String {
    let workspace = engine.create_workspace(branch, "oracle").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "dir/file.txt", b"oracle body")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "oracle".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "oracle fixture".into(),
            request_digest: None,
        })
        .unwrap();
    engine
        .list_tree(branch)
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == NamespaceEntryKind::File)
        .unwrap()
        .file_id
}

fn assert_full_encoder_oracle(
    before: &State,
    after: &State,
    roots: &crate::metadata_map::MapRoot,
    members: &[DiskConfig],
) -> crate::metadata_map::MapRoot {
    let source = crate::history_package::fixture_member_source(members);
    let (actual, prepared) =
        prepare_history_manifests_incremental(before, after, roots, members, MAX_METADATA, &source)
            .unwrap();
    let (expected, full) = prepare_history_manifests(after, MAX_METADATA, &source).unwrap();
    assert_eq!(
        canonical_json(actual.reference()).unwrap(),
        canonical_json(expected.reference()).unwrap()
    );
    assert_eq!(
        prepared
            .iter()
            .map(|object| object.hash.clone())
            .collect::<BTreeSet<_>>(),
        full.iter()
            .map(|object| object.hash.clone())
            .collect::<BTreeSet<_>>()
    );
    for object in prepared {
        object.store(members).unwrap();
    }
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("p"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    let oracle = &config.disks[..1];
    for object in full {
        object.store(oracle).unwrap();
    }
    // Graph admission walks each commit's namespace closure. Those objects are
    // published by publication, never by the manifest encoder under test, so
    // the oracle disk replays a member's exact published namespace bytes.
    for kind in [
        storage_contract::NAMESPACE_PAGE_KIND,
        storage_contract::NAMESPACE_INDEX_KIND,
    ] {
        for hash in crate::journal::list_object_hashes(&members[0], kind).unwrap() {
            let target = object_path(&oracle[0], kind, &hash);
            let mut directories = std::fs::DirBuilder::new();
            directories.recursive(true);
            std::os::unix::fs::DirBuilderExt::mode(&mut directories, 0o700);
            directories.create(target.parent().unwrap()).unwrap();
            fs::copy(object_path(&members[0], kind, &hash), target).unwrap();
        }
    }
    for id in after.histories.keys() {
        let hash = &expected.catalog_binding(id).unwrap().hash;
        let (_, graph) = bound_history_graph_observed(oracle, hash, &mut |_| {}).unwrap();
        for object in graph {
            let (kind, digest) = (object.kind(), object.hash());
            assert_eq!(
                fs::read(object_path(&members[0], kind, digest)).unwrap(),
                fs::read(object_path(&oracle[0], kind, digest)).unwrap()
            );
        }
    }
    actual
}

#[test]
fn incremental_roots_and_every_graph_object_match_forced_full_encoder_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("p"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    let mut engine = Engine::initialize(&temp.path().join("state"), config).unwrap();
    let first = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("first").unwrap(),
            "P01",
            1,
            "oracle",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let file = save_oracle_file(&mut engine, &first.main_branch_id);
    let second = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("second").unwrap(),
            "P01",
            1,
            "oracle",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    save_oracle_file(&mut engine, &second.main_branch_id);
    let members = engine.members();
    let mut before = engine.state().clone();
    let mut roots = store_history_manifests_bounded(&members, &before, MAX_METADATA).unwrap();
    let untouched = roots
        .catalog_binding(&second.history_tree_id)
        .unwrap()
        .hash
        .clone();
    for note in [Some("added"), Some("updated"), None] {
        let mut after = before.clone();
        after.begin_tracking();
        match note {
            Some(note) => {
                after.file_notes.insert(file.clone(), note.into());
            }
            None => {
                after.file_notes.remove(&file);
            }
        }
        roots = assert_full_encoder_oracle(&before, &after, &roots, &members);
        assert_eq!(
            roots.catalog_binding(&second.history_tree_id).unwrap().hash,
            untouched
        );
        before = after;
    }
    let mut after = before.clone();
    after.begin_tracking();
    after.sequence += 1;
    after.control_auth = Some(json!({"generation": "private"}));
    assert!(
        HistoryChanges::between(
            &before,
            &after,
            &crate::history_package::fixture_member_source(&members)
        )
        .histories
        .is_empty()
    );
    let unchanged = assert_full_encoder_oracle(&before, &after, &roots, &members);
    assert_eq!(unchanged, roots);
    // Fresh containers and deserialization lose provenance, but cannot change
    // bytes or suppress the existing caller-specific admission limit.
    after.file_notes = Default::default();
    assert_eq!(
        assert_full_encoder_oracle(&before, &after, &roots, &members),
        roots
    );
    let decoded: State = json_codec::from_slice(&canonical_json(&after).unwrap()).unwrap();
    assert_eq!(
        assert_full_encoder_oracle(&before, &decoded, &roots, &members),
        roots
    );
    let source = crate::history_package::fixture_member_source(&members);
    assert!(
        prepare_history_manifests_incremental(&before, &before, &roots, &members, 1, &source)
            .is_err()
    );
    assert!(prepare_history_manifests(&before, 1, &source).is_err());

    // Exercise a genuine newly published history from the same logical base:
    // every old root remains byte-identical, and deletion removes only its root.
    let base = engine.state().clone();
    let original_roots = store_history_manifests_bounded(&members, &base, MAX_METADATA).unwrap();
    let third = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("third").unwrap(),
            "P01",
            1,
            "oracle",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let new_state = engine.state().clone();
    assert_eq!(
        HistoryChanges::between(
            &base,
            &new_state,
            &crate::history_package::fixture_member_source(&members)
        )
        .histories,
        BTreeSet::from([third.history_tree_id.clone()])
    );
    let expanded = assert_full_encoder_oracle(&base, &new_state, &original_roots, &members);
    for history in base.histories.keys() {
        assert_eq!(
            expanded.catalog_binding(history).unwrap().hash,
            original_roots.catalog_binding(history).unwrap().hash
        );
    }
    let mut removed = new_state.clone();
    removed.begin_tracking();
    removed.histories.remove(&third.history_tree_id);
    assert_eq!(
        assert_full_encoder_oracle(&new_state, &removed, &expanded, &members),
        original_roots
    );
}

#[test]
fn fallback_keeps_exact_root_and_never_overwrites_corrupt_or_unsafe_dependencies() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("p"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    let before = crate::canonical_publication_fixture::CanonicalPublicationFixture::read()
        .restore(&config.disks);
    let mut after = before.clone();
    after.begin_tracking();
    let roots = store_history_manifests_bounded(&config.disks, &before, MAX_METADATA).unwrap();
    let history = before.histories.keys().next().unwrap();
    let hash = &roots.catalog_binding(history).unwrap().hash;

    // A missing leaf (with an intact root on every replica) exercises failure
    // after the loader has already authenticated the published root.
    let (_, graph) = bound_history_graph_observed(&config.disks, hash, &mut |_| {}).unwrap();
    let object = graph
        .iter()
        .find(|object| object.kind() == "state-page")
        .unwrap();
    let (kind, leaf) = (object.kind(), object.hash());
    for disk in &config.disks {
        fs::remove_file(object_path(disk, kind, leaf)).unwrap();
    }
    let (actual, objects) = prepare_history_manifests_incremental(
        &before,
        &after,
        &roots,
        &config.disks,
        MAX_METADATA,
        &crate::history_package::fixture_member_source(&config.disks),
    )
    .unwrap();
    assert_eq!(actual, roots);
    for object in objects {
        object.store(&config.disks).unwrap();
    }
    for disk in &config.disks {
        bound_history_graph_observed(std::slice::from_ref(disk), hash, &mut |_| {}).unwrap();
    }

    // Every root is corrupt, so a healthy replica cannot mask the fallback.
    for disk in &config.disks {
        fs::write(object_path(disk, "history-manifest", hash), b"corrupt root").unwrap();
    }
    let (actual, objects) = prepare_history_manifests_incremental(
        &before,
        &after,
        &roots,
        &config.disks,
        MAX_METADATA,
        &crate::history_package::fixture_member_source(&config.disks),
    )
    .unwrap();
    assert_eq!(actual, roots);
    let error = objects
        .iter()
        .try_for_each(|object| object.store(&config.disks).map(|_| ()))
        .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::ObjectCollision);
    for disk in &config.disks {
        assert_eq!(
            fs::read(object_path(disk, "history-manifest", hash)).unwrap(),
            b"corrupt root"
        );
    }

    // A syntactically valid but wrong prior root cannot be silently rebound to
    // the reconstructed State, even when the claimed object is wholly absent.
    let wrong_roots = crate::metadata_map::Encoder::new(MAX_METADATA)
        .update_catalog(&roots, BTreeSet::from([history.clone()]), |id| {
            let mut binding = roots.catalog_binding(id).cloned();
            if id == history {
                binding.as_mut().unwrap().hash = "0".repeat(64);
            }
            Ok(binding)
        })
        .unwrap();
    let Err(error) = prepare_history_manifests_incremental(
        &before,
        &after,
        &wrong_roots,
        &config.disks,
        MAX_METADATA,
        &crate::history_package::fixture_member_source(&config.disks),
    ) else {
        panic!("fallback must retain the exact published root");
    };
    assert_eq!(error.code(), storage_contract::ErrorCode::ObjectCorrupt);
    assert!(error.message().contains("published baseline"));

    // Falling back after an unsafe read is not authority to follow or replace
    // a symlink. The ordinary immutable writer must preserve both link/target.
    let target = temp.path().join("must-not-change");
    fs::write(&target, b"sentinel").unwrap();
    for disk in &config.disks {
        let path = object_path(disk, "history-manifest", hash);
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
    }
    let (_, objects) = prepare_history_manifests_incremental(
        &before,
        &after,
        &roots,
        &config.disks,
        MAX_METADATA,
        &crate::history_package::fixture_member_source(&config.disks),
    )
    .unwrap();
    let error = objects
        .iter()
        .try_for_each(|object| object.store(&config.disks).map(|_| ()))
        .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::UnsafePath);
    assert_eq!(fs::read(&target).unwrap(), b"sentinel");
    for disk in &config.disks {
        assert!(
            fs::symlink_metadata(object_path(disk, "history-manifest", hash))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}

#[test]
fn receipt_log_appends_are_history_independent_with_exact_segment_tracking() {
    let before = dependency_fixture();
    let id = OperationId::generate();
    assert_changes(&before, &[], |after| {
        after
            .operations
            .push(storage_contract::OperationRecordEntry {
                principal: "author".into(),
                result: OperationResult {
                    operation_id: id,
                    kind: "file.save".into(),
                    commit_id: Some("c1".into()),
                    history_tree_id: Some("h1".into()),
                    branch_id: Some("b1".into()),
                    commit_state: CommitState::Published,
                    protection_at_publish: ProtectionAtPublish::NotApplicable,
                    current_health: CurrentHealth::Healthy,
                    request_digest: "digest".into(),
                    sequence: u64::MAX,
                    result: json!({}),
                },
            })
            .unwrap();
        let storage_contract::SegmentChanges::Segments(dirty) =
            after.operations.changes_since(&before.operations)
        else {
            panic!("normal receipt append must retain its exact segment write set");
        };
        assert_eq!(
            dirty.iter().copied().collect::<Vec<_>>(),
            vec![after.operations.tail_ordinal()]
        );
    });
}
