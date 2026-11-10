//! Pure planning fixtures, not publication admission or physical-durability tests.
//! Nested inside disk_registry: only tests can construct this private source
//! without an Engine. Production callers cannot pass a forged registration set.
use super::*;
use crate::engine::disk_lifecycle::*;
use crate::*;
use std::collections::BTreeMap;

const HISTORY: &str = "history_00000000000000000000000000000001";
fn id(prefix: &str, n: usize) -> String {
    format!("{prefix}_{n:032x}")
}
fn operation() -> OperationId {
    "op_00000000000000000000000000000001".parse().unwrap()
}

struct Fixture {
    config: ValidatedConfig,
    state: State,
}
impl Fixture {
    fn new() -> Self {
        let mut config =
            Config::development((0..4).map(|n| format!("/unopened-preview/disabled-{n}").into()))
                .unwrap();
        // Two independent primary roots, one with two backup tiers. The fourth
        // registration has no metadata vote, but still owns visible content.
        config.disks[3].level = 1;
        config.disks[3].upstream_slot_id = None;
        config.disks[3].metadata_member = false;
        let mut state = State {
            sequence: 9,
            business_generation: 7,
            topology_generation: 3,
            ..State::default()
        };
        state.control_auth =
            Some(json_codec::json!({"private_test_marker":"never project credentials"}));
        for (n, slot) in [(1, "P01"), (2, "D04")] {
            let history = id("history", n);
            let repository = Repository {
                repository_id: id("repo", n),
                name: format!("namespace-{n}"),
                history_tree_id: history.clone(),
                main_branch_id: id("br", n),
                primary_slot_id: slot.into(),
                target_copies: 3,
                name_policy: "portable".into(),
                unicode_version: "15.1".into(),
                smb_enabled: false,
                smb_binding_generation: 1,
                file_manager_root: true,
                directory_deletions: std::collections::BTreeMap::new(),
            };
            state.histories.insert(
                history.clone(),
                HistoryTree {
                    history_tree_id: history.clone(),
                    repository_id: repository.repository_id.clone(),
                    root_commit_id: id("commit", n),
                    main_branch_id: Some(repository.main_branch_id.clone()),
                    generation: 1,
                    state: HistoryState::Active,
                    provenance: None,
                },
            );
            state.branches.insert(
                repository.main_branch_id.clone(),
                Branch {
                    branch_id: repository.main_branch_id.clone(),
                    history_tree_id: history,
                    name: "main".into(),
                    head_commit_id: id("commit", n),
                    fork_commit_id: id("commit", n),
                    branch_kind: BranchKind::Full,
                    file_id: None,
                    file_node_id: None,
                    pending_file_versions: BTreeMap::new(),
                    generation: 1,
                    state: BranchState::Active,
                },
            );
            state
                .repositories
                .insert(repository.repository_id.clone(), repository);
        }
        let mut fixture = Self {
            config: config.try_into().unwrap(),
            state,
        };
        fixture.add_version(1, 1, &["P01"]); // a, target exclusive.
        fixture.add_version(2, 2, &["P01", "B01", "C01"]); // b, shared.
        fixture.add_version(3, 3, &["P01", "B01"]); // older version of c.
        fixture.add_version(4, 3, &["P01"]); // c HEAD, target exclusive.
        fixture.add_version(5, 4, &["D04"]); // unrelated, nonvoting primary.
        fixture
            .state
            .versions
            .get_mut(&id("ver", 5))
            .unwrap()
            .history_tree_id = id("history", 2);
        fixture.commit(
            1,
            1,
            vec![
                file(1, 1, "a"),
                file(2, 2, "dir/b"),
                file(3, 4, "c"),
                directory(10, "dir"),
                directory(11, "empty"),
            ],
        );
        fixture.commit(2, 2, vec![file(4, 5, "unrelated"), directory(12, "empty")]);
        fixture.commit(3, 1, vec![file(3, 3, "c")]);
        fixture.directory_branch(
            1,
            1,
            vec![
                directory(20, "shared"),
                file(2, 2, "shared/b"),
                file(3, 4, "shared/c"),
            ],
        );
        fixture.directory_branch(
            2,
            1,
            vec![
                directory(21, "alias"),
                file(2, 2, "alias/b"),
                file(3, 3, "alias/c"),
            ],
        );
        fixture.directory_branch(3, 2, vec![directory(22, "empty")]);
        fixture
            .state
            .file_notes
            .insert(id("file", 3), "retained note".into());
        fixture
            .state
            .version_notes
            .insert(id("ver", 3), "old body note".into());
        fixture
            .state
            .directory_selections
            .insert(id("dir", 1), id("dbr", 1));
        fixture
    }
    fn add_version(&mut self, n: usize, file: usize, route: &[&str]) {
        let version = FileVersion {
            history_tree_id: HISTORY.into(),
            version_id: id("ver", n),
            file_id: id("file", file),
            creation_primary_slot_id: route[0].into(),
            logical_size: n as u64,
            version_sequence: n as u64,
            content_hash: "0".repeat(64).parse().unwrap(),
            checksum_chunk_bytes: 262144,
            checksum_manifest_hash: "1".repeat(64),
            creation_name: format!("file-{file}"),
            storage_name: format!("v{n:06}_20261104T000000Z_00000000.txt"),
            created_at: "2026-11-04T00:00:00Z".into(),
            source_version_ids: Vec::new(),
            created_as_full_checkpoint: true,
        };
        self.state.placements.insert(
            version.version_id.clone(),
            Placement {
                version_id: version.version_id.clone(),
                primary_slot_id: route[0].into(),
                target_copies: route.len() as u32,
                route_slot_ids: route.iter().map(|slot| (*slot).into()).collect(),
                topology_generation: 3,
                binding_generations: route.iter().map(|slot| ((*slot).into(), 1)).collect(),
            },
        );
        self.state
            .versions
            .insert(version.version_id.clone(), version);
    }
    fn commit(&mut self, n: usize, history: usize, entries: Vec<Entry>) {
        self.state.commits.insert(
            id("commit", n),
            Commit {
                commit_id: id("commit", n),
                history_tree_id: id("history", history),
                kind: "save".into(),
                parent_commit_ids: Vec::new(),
                tree_id: id("tree", n),
                sequence: n as u64,
                operation_id: operation(),
                author: "admin".into(),
                message: "frozen namespace".into(),
                entries,
                namespace_deletions: None,
                pending_file_versions: BTreeMap::new(),
                provenance: None,
            },
        );
    }
    fn directory_branch(&mut self, n: usize, repository: usize, entries: Vec<Entry>) {
        let branch = DirectoryBranch {
            directory_branch_id: id("dbr", n),
            repository_id: id("repo", repository),
            directory_id: id("dir", n),
            path: "".into(),
            name: format!("view-{n}"),
            parent_branch_id: None,
            head_tree_id: id("dtree", n),
            commit_id: id("commit", repository),
            entries: entries.clone(),
            file_branches: BTreeMap::new(),
            file_contexts: BTreeMap::new(),
            inherited_files: BTreeMap::new(),
            directory_refs: BTreeMap::new(),
            materialized: false,
            sequence: 1,
        };
        self.state.directory_trees.insert(
            branch.head_tree_id.clone(),
            DirectoryTree {
                tree_id: branch.head_tree_id.clone(),
                directory_branch_id: branch.directory_branch_id.clone(),
                parent_tree_id: None,
                commit_id: branch.commit_id.clone(),
                path: branch.path.clone(),
                entries,
                file_branches: BTreeMap::new(),
                file_contexts: BTreeMap::new(),
                inherited_files: BTreeMap::new(),
                directory_refs: BTreeMap::new(),
                sequence: 1,
            },
        );
        self.state
            .directory_branches
            .insert(branch.directory_branch_id.clone(), branch);
    }
    fn registry(&self) -> CurrentDiskRegistry<'_> {
        CurrentDiskRegistry::from_config(&self.config)
    }
    fn preview(&self, slot: &str) -> DeregistrationPreview<'_> {
        let registry = self.registry();
        DeregistrationPreview::capture(
            &self.state,
            registry,
            None,
            &registry.slot(slot).unwrap().identity(),
        )
        .unwrap()
    }
}
fn file(file: usize, version: usize, path: &str) -> Entry {
    Entry {
        entry_id: id("entry", file),
        path: name_policy::PortablePath::parse(path).unwrap(),
        kind: NamespaceEntryKind::File,
        file_id: id("file", file),
        version_id: id("ver", version),
        size: version as u64,
    }
}
fn directory(n: usize, path: &str) -> Entry {
    Entry {
        entry_id: id("entry", n),
        path: name_policy::PortablePath::parse(path).unwrap(),
        kind: NamespaceEntryKind::Directory,
        file_id: String::new(),
        version_id: String::new(),
        size: 0,
    }
}
fn slots(impact: &VersionDeregistrationImpact<'_>) -> Vec<String> {
    impact
        .remaining_holders
        .iter()
        .map(|disk| disk.slot_id.to_owned())
        .collect()
}
fn occurrence<'a>(
    view: &'a NamespaceDeregistrationPreview<'a>,
    path: &str,
) -> NamespaceOccurrenceImpact<'a> {
    view.occurrences()
        .next_page(256.try_into().unwrap())
        .rows
        .into_iter()
        .map(|row| row.unwrap())
        .find(|row| row.path == path)
        .unwrap()
}

#[test]
fn exclusive_a_hides_shared_b_and_unrelated_disk_survive_without_policy_change() {
    let fixture = Fixture::new();
    let plan = fixture.preview("P01");
    let a = plan.version(&id("ver", 1)).unwrap();
    assert_eq!(a.effect, RegistrationEffect::TargetExclusive);
    assert_eq!(a.target_role, Some(TargetPlacementRole::Primary));
    assert!(slots(&a).is_empty());
    let b = plan.version(&id("ver", 2)).unwrap();
    assert_eq!(b.effect, RegistrationEffect::SharedWithRegistered);
    assert_eq!(slots(&b), ["B01", "C01"]);
    assert_eq!(b.original_primary_slot_id, "P01");
    assert_eq!(b.target_copies, 3);
    assert_eq!(b.remaining_holders.iter().next().unwrap().level, 2);
    let unrelated = plan.version(&id("ver", 5)).unwrap();
    assert_eq!(unrelated.effect, RegistrationEffect::Unaffected);
    assert_eq!(unrelated.target_role, None);
    assert_eq!(slots(&unrelated), ["D04"]);
    assert!(
        !unrelated
            .remaining_holders
            .iter()
            .next()
            .unwrap()
            .metadata_member
    );
    let main = plan
        .namespace(DeregistrationNamespace::BranchHead(&id("br", 1)))
        .unwrap();
    assert_eq!(
        occurrence(&main, "a").visibility_after,
        OccurrenceVisibility::Hidden
    );
    assert_eq!(
        occurrence(&main, "dir/b").visibility_after,
        OccurrenceVisibility::RetainedContentOnly
    );
    assert_eq!(
        main.root_visibility_after().unwrap(),
        OccurrenceVisibility::RetainedContentOnly
    );
}

#[test]
fn backup_only_target_preserves_primary_and_does_not_remove_its_namespace() {
    let fixture = Fixture::new();
    let plan = fixture.preview("B01");
    let b = plan.version(&id("ver", 2)).unwrap();
    assert_eq!(b.target_role, Some(TargetPlacementRole::BackupOnly));
    assert_eq!(slots(&b), ["P01", "C01"]);
    assert_eq!(b.effect, RegistrationEffect::SharedWithRegistered);
    let main = plan
        .namespace(DeregistrationNamespace::BranchHead(&id("br", 1)))
        .unwrap();
    for path in ["a", "dir/b", "c", "dir", "empty"] {
        assert_eq!(
            occurrence(&main, path).visibility_after,
            OccurrenceVisibility::RegisteredNamespace
        );
    }
    assert_eq!(fixture.state.placements[&id("ver", 2)].target_copies, 3);
}

#[test]
fn exclusive_head_never_promotes_an_older_shared_version() {
    let fixture = Fixture::new();
    let plan = fixture.preview("P01");
    let head = plan
        .namespace(DeregistrationNamespace::BranchHead(&id("br", 1)))
        .unwrap();
    let row = occurrence(&head, "c");
    assert_eq!(row.visibility_after, OccurrenceVisibility::Hidden);
    assert_eq!(row.version.unwrap().version_id, id("ver", 4));
    let historical = plan
        .namespace(DeregistrationNamespace::Commit(&id("commit", 3)))
        .unwrap();
    let old = occurrence(&historical, "c");
    assert_eq!(
        old.visibility_after,
        OccurrenceVisibility::RetainedContentOnly
    );
    assert_eq!(old.version.unwrap().version_id, id("ver", 3));
    assert_eq!(
        fixture.state.branches[&id("br", 1)].head_commit_id,
        id("commit", 1)
    );
}

#[test]
fn shared_directory_occurrences_remain_distinct_and_frozen() {
    let fixture = Fixture::new();
    let plan = fixture.preview("P01");
    let one = plan
        .namespace(DeregistrationNamespace::DirectoryHead(&id("dbr", 1)))
        .unwrap();
    let two = plan
        .namespace(DeregistrationNamespace::DirectoryHead(&id("dbr", 2)))
        .unwrap();
    let b1 = occurrence(&one, "shared/b");
    let b2 = occurrence(&two, "alias/b");
    assert_eq!(b1.entry_id, b2.entry_id);
    assert_ne!(b1.scope, b2.scope);
    assert_eq!(
        b1.visibility_after,
        OccurrenceVisibility::RetainedContentOnly
    );
    assert_eq!(
        b2.visibility_after,
        OccurrenceVisibility::RetainedContentOnly
    );
    assert_eq!(
        occurrence(&one, "shared/c").visibility_after,
        OccurrenceVisibility::Hidden
    );
    assert_eq!(
        occurrence(&two, "alias/c").visibility_after,
        OccurrenceVisibility::RetainedContentOnly
    );
    assert_eq!(
        occurrence(&one, "shared").visibility_after,
        OccurrenceVisibility::RetainedContentOnly
    );
    let tree = plan
        .namespace(DeregistrationNamespace::DirectoryTree(&id("dtree", 2)))
        .unwrap();
    assert_eq!(tree.frozen_head_id(), id("dtree", 2));
    assert_eq!(
        occurrence(&tree, "alias/c").version.unwrap().version_id,
        id("ver", 3)
    );
    assert_eq!(
        fixture.state.directory_selections[&id("dir", 1)],
        id("dbr", 1)
    );
}

#[test]
fn empty_directories_follow_their_owner_not_other_disks_missing_files() {
    let fixture = Fixture::new();
    let plan = fixture.preview("P01");
    let removed = plan
        .namespace(DeregistrationNamespace::BranchHead(&id("br", 1)))
        .unwrap();
    assert_eq!(
        occurrence(&removed, "empty").visibility_after,
        OccurrenceVisibility::Hidden
    );
    let retained = plan
        .namespace(DeregistrationNamespace::DirectoryHead(&id("dbr", 3)))
        .unwrap();
    assert_eq!(
        occurrence(&retained, "empty").visibility_after,
        OccurrenceVisibility::RegisteredNamespace
    );
    assert_eq!(
        retained.root_visibility_after().unwrap(),
        OccurrenceVisibility::RegisteredNamespace
    );
}

#[test]
fn target_pool_disk_slot_and_binding_spoofs_are_rejected() {
    let fixture = Fixture::new();
    let original = fixture.registry().slot("P01").unwrap().identity();
    for changed in [0, 1, 2, 3] {
        let mut target = original.clone();
        match changed {
            0 => target.pool_id = id("pool", 999),
            1 => target.disk_id = id("disk", 999),
            2 => target.slot_id = "B01".into(),
            3 => target.binding_generation += 1,
            _ => unreachable!(),
        }
        assert!(matches!(
            DeregistrationPreview::capture(&fixture.state, fixture.registry(), None, &target),
            Err(DeregistrationPlanError::TargetIdentityMismatch { .. })
        ));
    }
    let mut absent = original;
    absent.slot_id = "P99".into();
    assert!(matches!(
        DeregistrationPreview::capture(&fixture.state, fixture.registry(), None, &absent),
        Err(DeregistrationPlanError::TargetNotRegistered(_))
    ));
}

#[test]
fn stale_or_missing_placement_bindings_fail_instead_of_counting_a_fake_holder() {
    for corrupt in [0, 1, 2, 3] {
        let mut fixture = Fixture::new();
        let placement = fixture.state.placements.get_mut(&id("ver", 2)).unwrap();
        match corrupt {
            0 => {
                placement.binding_generations.insert("B01".into(), 2);
            }
            1 => {
                placement.binding_generations.remove("B01");
            }
            2 => {
                placement.route_slot_ids[1] = "X99".into();
                placement.binding_generations.remove("B01");
                placement.binding_generations.insert("X99".into(), 1);
            }
            3 => {
                placement.route_slot_ids[2] = "B01".into();
            }
            _ => unreachable!(),
        }
        let plan = fixture.preview("P01");
        assert!(matches!(
            plan.version(&id("ver", 2)),
            Err(DeregistrationPlanError::PlacementBindingMismatch { .. }
                | DeregistrationPlanError::PlacementInvalid { .. }
                | DeregistrationPlanError::PlacementSlotNotRegistered { .. })
        ));
    }
}

#[test]
fn unavailable_or_disabled_io_roots_do_not_remove_registrations() {
    // No roots are opened or required. There is intentionally no availability,
    // health or I/O-mode argument that could filter a registered holder. Config
    // does not yet encode durable Disabled: this is NOT a disabled-I/O test.
    let fixture = Fixture::new();
    let plan = fixture.preview("P01");
    assert_eq!(
        plan.remaining_registered_disks()
            .map(|disk| disk.slot_id)
            .collect::<Vec<_>>(),
        ["B01", "C01", "D04"]
    );
    assert_eq!(slots(&plan.version(&id("ver", 2)).unwrap()), ["B01", "C01"]);
    assert_eq!(plan.boundary().published_snapshot_hash, None);
    assert_eq!(plan.boundary().admitted_membership_generation_hash, None);
}

#[test]
fn purged_and_migrated_versions_are_not_revived_or_omitted_from_the_plan() {
    let mut fixture = Fixture::new();
    fixture
        .state
        .content_states
        .insert(id("ver", 2), ContentState::Purged);
    fixture
        .state
        .content_states
        .insert(id("ver", 3), ContentState::Migrated);
    let plan = fixture.preview("P01");
    assert_eq!(
        plan.version(&id("ver", 2)).unwrap().content_state,
        ContentState::Purged
    );
    assert_eq!(
        plan.version(&id("ver", 3)).unwrap().content_state,
        ContentState::Migrated
    );
    assert_eq!(
        plan.versions()
            .next_page(256.try_into().unwrap())
            .rows
            .len(),
        5
    );
}

#[test]
fn pages_are_bounded_complete_ordered_and_do_not_restart_the_scan() {
    let mut fixture = Fixture::new();
    for n in 6..600 {
        fixture.add_version(n, n, &["P01", "B01"]);
    }
    let plan = fixture.preview("P01");
    assert!(ImpactPageSize::try_from(0).is_err());
    assert!(ImpactPageSize::try_from(257).is_err());
    let mut scan = plan.versions();
    let mut ids = Vec::new();
    loop {
        let page = scan.next_page(17.try_into().unwrap());
        assert!(page.rows.len() <= 17);
        ids.extend(
            page.rows
                .into_iter()
                .map(|row| row.unwrap().version_id.to_owned()),
        );
        if !page.has_more {
            break;
        }
    }
    assert_eq!(
        ids,
        fixture.state.versions.keys().cloned().collect::<Vec<_>>()
    );
    assert!(scan.next_page(17.try_into().unwrap()).rows.is_empty());
    let namespace = plan
        .namespace(DeregistrationNamespace::BranchHead(&id("br", 1)))
        .unwrap();
    let mut scan = namespace.occurrences();
    let first = scan.next_page(2.try_into().unwrap());
    assert_eq!(first.rows.len(), 2);
    assert!(first.has_more);
    let second = scan.next_page(2.try_into().unwrap());
    assert_eq!(second.rows[0].as_ref().unwrap().path, "c");
    let last = scan.next_page(2.try_into().unwrap());
    assert_eq!(last.rows.len(), 1);
    assert!(!last.has_more);
}

#[test]
fn preview_preserves_complete_historical_bytes_and_config_without_cloning_them() {
    let fixture = Fixture::new();
    let before_state = canonical_json(&fixture.state).unwrap();
    let before_config = canonical_json(&fixture.config).unwrap();
    let before_placements = canonical_json(&fixture.state.placements).unwrap();
    let state_hash = object_hash("preview-test-state", &fixture.state).unwrap();
    let plan = fixture.preview("P01");
    let mut versions = plan.versions();
    loop {
        let page = versions.next_page(2.try_into().unwrap());
        for row in page.rows {
            assert!(row.is_ok());
        }
        if !page.has_more {
            break;
        }
    }
    for scope in [
        DeregistrationNamespace::BranchHead(&id("br", 1)),
        DeregistrationNamespace::DirectoryHead(&id("dbr", 2)),
    ] {
        let namespace = plan.namespace(scope).unwrap();
        assert!(namespace.root_visibility_after().is_ok());
        let mut rows = namespace.occurrences();
        loop {
            let page = rows.next_page(2.try_into().unwrap());
            for row in page.rows {
                assert!(row.is_ok());
            }
            if !page.has_more {
                break;
            }
        }
    }
    let b = plan.version(&id("ver", 2)).unwrap();
    assert!(std::ptr::eq(
        b.version_id.as_ptr(),
        fixture.state.versions[&id("ver", 2)].version_id.as_ptr()
    ));
    assert_eq!(canonical_json(&fixture.state).unwrap(), before_state);
    assert_eq!(canonical_json(&fixture.config).unwrap(), before_config);
    assert_eq!(
        canonical_json(&fixture.state.placements).unwrap(),
        before_placements
    );
    assert_eq!(
        object_hash("preview-test-state", &fixture.state).unwrap(),
        state_hash
    );
    assert_eq!(fixture.state.file_notes[&id("file", 3)], "retained note");
    assert_eq!(fixture.state.version_notes[&id("ver", 3)], "old body note");
}

#[test]
fn empty_namespace_root_follows_exact_owner_and_does_not_borrow_other_roots_content() {
    let mut fixture = Fixture::new();
    fixture
        .state
        .directory_branches
        .get_mut(&id("dbr", 1))
        .unwrap()
        .entries
        .clear();
    fixture
        .state
        .directory_branches
        .get_mut(&id("dbr", 3))
        .unwrap()
        .entries
        .clear();
    let plan = fixture.preview("P01");
    let removed = plan
        .namespace(DeregistrationNamespace::DirectoryHead(&id("dbr", 1)))
        .unwrap();
    assert_eq!(
        removed.root_visibility_after().unwrap(),
        OccurrenceVisibility::Hidden
    );
    let page = removed.occurrences().next_page(1.try_into().unwrap());
    assert!(page.rows.is_empty());
    assert!(!page.has_more);
    let kept = plan
        .namespace(DeregistrationNamespace::DirectoryHead(&id("dbr", 3)))
        .unwrap();
    assert_eq!(
        kept.root_visibility_after().unwrap(),
        OccurrenceVisibility::RegisteredNamespace
    );
}

#[test]
fn directory_prefix_requires_a_component_boundary() {
    let mut fixture = Fixture::new();
    fixture.directory_branch(
        4,
        1,
        vec![
            directory(40, "dir"),
            directory(41, "dir-other"),
            file(2, 2, "dir-other/b"),
        ],
    );
    let plan = fixture.preview("P01");
    let view = plan
        .namespace(DeregistrationNamespace::DirectoryHead(&id("dbr", 4)))
        .unwrap();
    assert_eq!(
        occurrence(&view, "dir").visibility_after,
        OccurrenceVisibility::Hidden
    );
    assert_eq!(
        occurrence(&view, "dir-other").visibility_after,
        OccurrenceVisibility::RetainedContentOnly
    );
}

#[test]
fn invalid_version_is_an_explicit_page_result_not_silently_dropped() {
    let mut fixture = Fixture::new();
    fixture.state.placements.remove(&id("ver", 2));
    let plan = fixture.preview("P01");
    let mut scan = plan.versions();
    let page = scan.next_page(2.try_into().unwrap());
    assert!(page.has_more);
    assert_eq!(page.rows[0].as_ref().unwrap().version_id, id("ver", 1));
    assert!(
        matches!(&page.rows[1], Err(DeregistrationPlanError::PlacementMissing(version)) if version == &id("ver", 2))
    );
    let rest = scan.next_page(256.try_into().unwrap());
    assert!(!rest.has_more);
    assert_eq!(rest.rows.len(), 3);
    assert_eq!(rest.rows[0].as_ref().unwrap().version_id, id("ver", 3));
}

#[test]
fn shared_directory_reference_graph_and_inherited_versions_are_not_rewritten() {
    let mut fixture = Fixture::new();
    let reference = DirectoryReference {
        directory_branch_id: id("dbr", 2),
        tree_id: id("dtree", 2),
    };
    fixture
        .state
        .directory_branches
        .get_mut(&id("dbr", 1))
        .unwrap()
        .directory_refs
        .insert(id("dir", 2), reference.clone());
    fixture
        .state
        .directory_trees
        .get_mut(&id("dtree", 1))
        .unwrap()
        .directory_refs
        .insert(id("dir", 2), reference);
    fixture
        .state
        .branches
        .get_mut(&id("br", 1))
        .unwrap()
        .pending_file_versions
        .insert(id("file", 3), id("ver", 3));
    fixture
        .state
        .directory_branches
        .get_mut(&id("dbr", 2))
        .unwrap()
        .inherited_files
        .insert(id("file", 3), id("ver", 3));
    let original = canonical_json(&fixture.state).unwrap();
    let plan = fixture.preview("P01");
    let view = plan
        .namespace(DeregistrationNamespace::DirectoryHead(&id("dbr", 2)))
        .unwrap();
    assert_eq!(
        occurrence(&view, "alias/c").version.unwrap().version_id,
        id("ver", 3)
    );
    assert_eq!(canonical_json(&fixture.state).unwrap(), original);
    assert_eq!(
        fixture.state.directory_branches[&id("dbr", 1)].directory_refs[&id("dir", 2)].tree_id,
        id("dtree", 2)
    );
}
