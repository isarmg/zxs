//! Frozen metadata vectors only. They do not activate a registry or simulate
//! publication: the isolated Engine receives a fixture State and its exact
//! prepared snapshot hash before registry capture. Original files stay intact.
use super::super::{PreparedDiskRegistry, RegistryExitRequest, encode_registry};
use super::*;
use crate::*;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

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
                file(3, 3, "shared/c-retained"),
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
    fn commit(&mut self, n: usize, history: usize, mut entries: Vec<Entry>) {
        entries.sort_by(|left, right| left.lookup_key().cmp(right.lookup_key()));
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
    fn directory_branch(&mut self, n: usize, repository: usize, mut entries: Vec<Entry>) {
        entries.sort_by(|left, right| left.lookup_key().cmp(right.lookup_key()));
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

type Objects = BTreeMap<(String, String), Vec<u8>>;
fn read(objects: &Objects, kind: &str, hash: &str) -> Result<Vec<u8>> {
    objects
        .get(&(kind.into(), hash.into()))
        .cloned()
        .ok_or_else(|| Error::new(ErrorCode::ObjectCorrupt, "fixture object missing"))
}
fn registry(prepared: &PreparedDiskRegistry) -> VerifiedDiskRegistry {
    let objects: Objects = prepared
        .objects()
        .map(|(kind, hash, bytes)| ((kind.into(), hash.into()), bytes.to_vec()))
        .collect();
    prepared
        .verify_with(|kind, hash| read(&objects, kind, hash))
        .unwrap()
}
fn objects(prepared: &PreparedOccurrenceMask) -> Objects {
    prepared
        .objects()
        .map(|(kind, hash, bytes)| ((kind.into(), hash.into()), bytes.to_vec()))
        .collect()
}
fn fixture(owner_retained: bool) -> (tempfile::TempDir, Engine, VerifiedDiskRegistry) {
    let mut fixture = Fixture::new();
    if owner_retained {
        fixture
            .state
            .repositories
            .get_mut(&id("repo", 1))
            .unwrap()
            .primary_slot_id = "D04".into();
    }
    let temp = tempfile::tempdir().unwrap();
    let mut config = (*fixture.config).clone();
    for (n, disk) in config.disks.iter_mut().enumerate() {
        disk.root = temp.path().join(format!("data-{n}"));
    }
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    engine.state = Arc::new(fixture.state);
    engine.snapshot = Some(
        crate::journal::prepare_snapshot(&engine.state, MAX_METADATA)
            .unwrap()
            .hash,
    );
    let base = registry(&engine.prepare_disk_registry_base().unwrap());
    (temp, engine, base)
}
fn target(base: &VerifiedDiskRegistry, slot: &str) -> DiskRegistrationIdentity {
    base.historical_topology()
        .find(|disk| disk.identity.slot_id == slot)
        .unwrap()
        .identity
        .clone()
}
fn successor(base: &VerifiedDiskRegistry, hash: &str, operation: usize) -> VerifiedDiskRegistry {
    let request = RegistryExitRequest {
        target: target(base, "P01"),
        operation_id: id("op", operation).parse().unwrap(),
        principal: "admin".into(),
        request_digest: "1".repeat(64),
        intent_hash: "2".repeat(64),
        effect: RegistryExitEffect::Detach {
            occurrence_mask_hash: hash.into(),
        },
    };
    let prepared = base.prepare_exit(request).unwrap();
    let objects: Objects = prepared
        .objects()
        .map(|(kind, hash, bytes)| ((kind.into(), hash.into()), bytes.to_vec()))
        .collect();
    base.verify_successor(prepared.commitment(), |kind, hash| {
        read(&objects, kind, hash)
    })
    .unwrap()
}
fn path(value: &str) -> name_policy::PortablePath {
    name_policy::PortablePath::parse(value).unwrap()
}
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    for item in fs::read_dir(root).unwrap() {
        let path = item.unwrap().path();
        if path.is_dir() {
            out.extend(files(&path));
        } else {
            out.insert(path.clone(), fs::read(path).unwrap());
        }
    }
    out
}

fn terminal_fixture() -> (tempfile::TempDir, Engine, VerifiedDiskRegistry) {
    let mut frozen = Fixture::new();
    frozen
        .state
        .repositories
        .get_mut(&id("repo", 1))
        .unwrap()
        .primary_slot_id = "D04".into();
    let temp = tempfile::tempdir().unwrap();
    let mut config = (*frozen.config).clone();
    for (n, disk) in config.disks.iter_mut().enumerate() {
        disk.root = temp.path().join(format!("retirement-{n}"));
        disk.metadata_member = n == 0;
    }
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    engine.state = Arc::new(frozen.state);
    engine.snapshot = Some(
        crate::journal::prepare_snapshot(&engine.state, MAX_METADATA)
            .unwrap()
            .hash,
    );
    let base = registry(&engine.prepare_disk_registry_base().unwrap());
    (temp, engine, base)
}

fn terminal_request(
    base: &VerifiedDiskRegistry,
    slot: &str,
    mask: &str,
    operation: usize,
) -> RegistryExitRequest {
    RegistryExitRequest {
        target: target(base, slot),
        operation_id: id("op", operation).parse().unwrap(),
        principal: "admin".into(),
        request_digest: "1".repeat(64),
        intent_hash: "2".repeat(64),
        effect: RegistryExitEffect::Detach {
            occurrence_mask_hash: mask.into(),
        },
    }
}

#[test]
fn terminal_retirement_masks_preserve_earlier_removals_until_last_registered_holder_leaves() {
    let (_temp, engine, base) = terminal_fixture();
    let frozen = FrozenDetachSource::capture(&engine, &base, &target(&base, "P01")).unwrap();
    let mask = frozen.prepare_mask().unwrap();
    let mask_objects = objects(&mask);
    let prepared = base
        .prepare_terminal_exit(terminal_request(&base, "P01", mask.hash(), 30))
        .unwrap();
    let registry_objects: Objects = prepared
        .objects()
        .map(|(kind, hash, bytes)| ((kind.into(), hash.into()), bytes.to_vec()))
        .collect();
    let mut terminal = base
        .verify_terminal_successor(prepared.commitment(), |kind, hash| {
            read(&registry_objects, kind, hash)
        })
        .unwrap();
    let mut namespaces = frozen
        .verify_mask(mask.hash(), |kind, hash| read(&mask_objects, kind, hash))
        .unwrap()
        .prepare_current_namespaces(terminal.verified_registry())
        .unwrap();
    let original_entries = engine.state.commits[&id("commit", 1)].entries.clone();

    for (number, slot) in [(31, "B01"), (32, "C01")] {
        let source = FrozenDetachSource::capture_terminal(
            &engine.state,
            &engine.config,
            &terminal,
            &namespaces,
            &target(&base, slot),
        )
        .unwrap();
        let mask = source.prepare_mask().unwrap();
        let mask_objects = objects(&mask);
        let request = terminal_request(&base, slot, mask.hash(), number);
        let prepared = terminal
            .prepare_management_retirement(request.clone())
            .unwrap();
        let registry_objects: Objects = prepared
            .objects()
            .map(|(kind, hash, bytes)| ((kind.into(), hash.into()), bytes.to_vec()))
            .collect();
        let next = terminal
            .verify_management_retirement(request, prepared.commitment(), |kind, hash| {
                read(&registry_objects, kind, hash)
            })
            .unwrap();
        let next_namespaces = source
            .verify_mask(mask.hash(), |kind, hash| read(&mask_objects, kind, hash))
            .unwrap()
            .prepare_terminal_retirement_namespaces(&next)
            .unwrap();
        let entries = next_namespaces
            .view
            .branch_entries(&id("br", 1), &id("commit", 1))
            .unwrap();
        assert!(
            entries
                .iter()
                .all(|entry| !matches!(entry.path(), "a" | "c"))
        );
        assert_eq!(
            entries.iter().any(|entry| entry.path() == "dir/b"),
            slot == "B01"
        );
        assert!(next.require_resumable().is_err());
        assert_eq!(
            next.verified_registry().state.detached.len(),
            if slot == "B01" { 2 } else { 3 }
        );
        terminal = next;
        namespaces = next_namespaces;
    }
    assert_eq!(
        engine.state.commits[&id("commit", 1)].entries,
        original_entries
    );
    assert_eq!(namespaces.view.branches[&id("br", 1)].hidden_paths.len(), 3);
    assert!(namespaces.view.known_files.contains(&id("file", 1)));
    assert!(namespaces.view.unavailable_versions.contains(&id("ver", 2)));
    assert_eq!(terminal.registered_disks().len(), 1);
}

#[test]
fn terminal_retirement_mask_cannot_use_a_predecessor_or_rehashed_omission_as_current_namespace() {
    let (_temp, engine, base) = terminal_fixture();
    let frozen = FrozenDetachSource::capture(&engine, &base, &target(&base, "P01")).unwrap();
    let mask = frozen.prepare_mask().unwrap();
    let mask_objects = objects(&mask);
    let prepared = base
        .prepare_terminal_exit(terminal_request(&base, "P01", mask.hash(), 40))
        .unwrap();
    let registry_objects: Objects = prepared
        .objects()
        .map(|(kind, hash, bytes)| ((kind.into(), hash.into()), bytes.to_vec()))
        .collect();
    let terminal = base
        .verify_terminal_successor(prepared.commitment(), |kind, hash| {
            read(&registry_objects, kind, hash)
        })
        .unwrap();
    let namespaces = frozen
        .verify_mask(mask.hash(), |kind, hash| read(&mask_objects, kind, hash))
        .unwrap()
        .prepare_current_namespaces(terminal.verified_registry())
        .unwrap();
    let source = FrozenDetachSource::capture_terminal(
        &engine.state,
        &engine.config,
        &terminal,
        &namespaces,
        &target(&base, "B01"),
    )
    .unwrap();
    let expected = source.prepare_mask().unwrap();
    let request = terminal_request(&base, "B01", expected.hash(), 41);
    let prepared = terminal
        .prepare_management_retirement(request.clone())
        .unwrap();
    let registry_objects: Objects = prepared
        .objects()
        .map(|(kind, hash, bytes)| ((kind.into(), hash.into()), bytes.to_vec()))
        .collect();
    let next = terminal
        .verify_management_retirement(request, prepared.commitment(), |kind, hash| {
            read(&registry_objects, kind, hash)
        })
        .unwrap();
    assert!(
        FrozenDetachSource::capture_terminal(
            &engine.state,
            &engine.config,
            &next,
            &namespaces,
            &target(&base, "C01")
        )
        .is_err()
    );
    let mut omitted = source.expected().unwrap();
    omitted
        .scopes
        .remove(&NamespaceScope::Commit(id("commit", 1)).key().unwrap());
    let forged = encode_mask(&omitted).unwrap();
    let forged_objects = objects(&forged);
    assert!(
        source
            .verify_mask(forged.hash(), |kind, hash| read(
                &forged_objects,
                kind,
                hash
            ))
            .is_err()
    );
}

#[test]
fn membership_registry_occurrence_mixed_namespace_is_one_write_baseline() {
    let (_temp, engine, base) = fixture(true);
    let source = FrozenDetachSource::capture(&engine, &base, &target(&base, "P01")).unwrap();
    let prepared = source.prepare_mask().unwrap();
    let objects = objects(&prepared);
    let verified = source
        .verify_mask(prepared.hash(), |kind, hash| read(&objects, kind, hash))
        .unwrap();
    let next = successor(&base, prepared.hash(), 1);
    let view = verified.prepare_current_namespaces(&next).unwrap().view;
    let branch = id("br", 1);
    let entries = view.branch_entries(&branch, &id("commit", 1)).unwrap();
    assert!(
        entries
            .iter()
            .all(|entry| !matches!(entry.path(), "a" | "c"))
    );
    for retained in ["dir/b", "empty", "dir"] {
        assert!(entries.iter().any(|entry| entry.path() == retained));
    }
    let mut draft = entries.to_vec();
    draft
        .iter_mut()
        .find(|entry| entry.path() == "dir/b")
        .unwrap()
        .version_id = id("ver", 99);
    view.validate_draft(&branch, &draft).unwrap();
    // Production upload allocation is exercised by current_namespace_tests.
    // Here the actual current consumer must admit a fresh pair and reject old IDs.
    let fresh = Entry {
        entry_id: new_id("entry").unwrap(),
        path: path("a"),
        kind: NamespaceEntryKind::File,
        file_id: new_id("file").unwrap(),
        version_id: id("ver", 100),
        size: 1,
    };
    assert_ne!(fresh.entry_id, id("entry", 1));
    assert_ne!(fresh.file_id, id("file", 1));
    view.validate_upload_identity(entries, "a", &fresh.entry_id)
        .unwrap();
    assert!(
        view.validate_upload_identity(entries, "a", &id("entry", 1))
            .is_err()
    );
    draft.push(fresh);
    view.validate_draft(&branch, &draft).unwrap();
    draft.push(file(1, 1, "renamed-old-a"));
    assert!(view.validate_draft(&branch, &draft).is_err());
}

#[test]
fn membership_registry_occurrence_hidden_pending_identity_cannot_reenter_as_fresh_file() {
    let (_temp, mut engine, _) = fixture(true);
    // Deliberately malformed metadata vector: normal snapshot admission requires
    // pending versions to exist. This also fences a pending-only identity if a
    // future producer reaches this boundary with an unmaterialized reference.
    Arc::make_mut(&mut engine.state)
        .branches
        .get_mut(&id("br", 1))
        .unwrap()
        .pending_file_versions
        .insert(id("file", 77), id("ver", 77));
    engine.snapshot = Some(
        crate::journal::prepare_snapshot(&engine.state, MAX_METADATA)
            .unwrap()
            .hash,
    );
    let base = registry(&engine.prepare_disk_registry_base().unwrap());
    let source = FrozenDetachSource::capture(&engine, &base, &target(&base, "P01")).unwrap();
    let prepared = source.prepare_mask().unwrap();
    let objects = objects(&prepared);
    let verified = source
        .verify_mask(prepared.hash(), |kind, hash| read(&objects, kind, hash))
        .unwrap();
    let next = successor(&base, prepared.hash(), 1);
    let view = verified.prepare_current_namespaces(&next).unwrap().view;
    let branch = id("br", 1);
    let mut resurrected = file(77, 77, "a");
    resurrected.entry_id = id("entry", 999);
    assert!(view.validate_draft(&branch, &[resurrected]).is_err());
    let mut pending_body = file(77, 77, "a");
    pending_body.entry_id = new_id("entry").unwrap();
    pending_body.file_id = new_id("file").unwrap();
    assert!(view.validate_draft(&branch, &[pending_body]).is_err());
    let mut revived_version = view
        .branch_entries(&branch, &id("commit", 1))
        .unwrap()
        .iter()
        .find(|entry| entry.path() == "dir/b")
        .unwrap()
        .clone();
    revived_version.version_id = id("ver", 1);
    assert!(view.validate_draft(&branch, &[revived_version]).is_err());
}

#[test]
fn membership_registry_occurrence_versions_and_scopes_do_not_collapse_to_file_id() {
    let (_temp, engine, base) = fixture(true);
    let source = FrozenDetachSource::capture(&engine, &base, &target(&base, "P01")).unwrap();
    let prepared = source.prepare_mask().unwrap();
    let objects = objects(&prepared);
    let verified = source
        .verify_mask(prepared.hash(), |kind, hash| read(&objects, kind, hash))
        .unwrap();
    let next = successor(&base, prepared.hash(), 1);
    let view = verified.prepare_current_namespaces(&next).unwrap().view;
    let branch = id("br", 1);
    let entries = view.branch_entries(&branch, &id("commit", 1)).unwrap();
    assert!(entries.iter().all(|entry| entry.path() != "c"));
    assert!(
        view.validate_draft(&branch, &[directory(22, "foreign-empty")])
            .is_err()
    );
    view.validate_draft(&branch, &[directory(999, "new-empty")])
        .unwrap();
    // Immutable history remains history, never a current writable scope.
    assert_eq!(
        engine.state.commits[&id("commit", 3)]
            .entries
            .iter()
            .find(|entry| entry.path() == "c")
            .unwrap()
            .version_id,
        id("ver", 3)
    );
    assert!(
        view.branch_entries(&id("commit", 3), &id("commit", 3))
            .is_err()
    );
    let one = view
        .directory_entries(&id("dbr", 1), &id("dtree", 1))
        .unwrap();
    let two = view
        .directory_entries(&id("dbr", 2), &id("dtree", 2))
        .unwrap();
    assert!(one.iter().all(|entry| entry.path() != "shared/c"));
    let retained = one
        .iter()
        .find(|entry| entry.path() == "shared/c-retained")
        .unwrap();
    assert_eq!(retained.entry_id, id("entry", 3));
    assert_eq!(retained.version_id, id("ver", 3));
    assert!(
        view.validate_upload_identity(one, "shared/c", &retained.entry_id)
            .is_err()
    );
    view.validate_upload_identity(one, "shared/c", &new_id("entry").unwrap())
        .unwrap();
    assert_eq!(
        two.iter()
            .find(|entry| entry.path() == "alias/c")
            .unwrap()
            .version_id,
        id("ver", 3)
    );
}

#[test]
fn membership_registry_occurrence_removed_owner_keeps_shared_content_without_write_route() {
    let (_temp, engine, base) = fixture(false);
    let source = FrozenDetachSource::capture(&engine, &base, &target(&base, "P01")).unwrap();
    let prepared = source.prepare_mask().unwrap();
    let objects = objects(&prepared);
    let verified = source
        .verify_mask(prepared.hash(), |kind, hash| read(&objects, kind, hash))
        .unwrap();
    let next = successor(&base, prepared.hash(), 1);
    let view = verified.prepare_current_namespaces(&next).unwrap().view;
    let branch = id("br", 1);
    let entries = view.branch_entries(&branch, &id("commit", 1)).unwrap();
    assert!(entries.iter().any(|entry| entry.path() == "dir/b"));
    assert!(entries.iter().any(|entry| entry.path() == "dir"));
    assert!(entries.iter().all(|entry| entry.path() != "empty"));
    assert!(!view.branch_writable(&engine.state, &branch).unwrap());
    assert!(view.require_branch_write(&engine.state, &branch).is_err());
    assert!(
        view.workspace_baseline(&engine.state, &workspace())
            .is_err()
    );
}

#[test]
fn membership_registry_occurrence_rehashed_omissions_and_extra_deletions_are_rejected() {
    let (_temp, engine, base) = fixture(true);
    let source = FrozenDetachSource::capture(&engine, &base, &target(&base, "P01")).unwrap();
    let expected = source.expected().unwrap();
    let scope_key = NamespaceScope::BranchHead(id("br", 1)).key().unwrap();
    let mut candidates = Vec::new();
    let mut missing = expected.clone();
    missing
        .scopes
        .get_mut(&scope_key)
        .unwrap()
        .hidden
        .remove("a");
    candidates.push(missing);
    let mut shared = expected.clone();
    shared.scopes.get_mut(&scope_key).unwrap().hidden.insert(
        "dir/b".into(),
        object_hash("disk-occurrence-entry", &file(2, 2, "dir/b")).unwrap(),
    );
    candidates.push(shared);
    let mut scope = expected.clone();
    scope.scopes.remove(&scope_key);
    candidates.push(scope);
    let mut head = expected.clone();
    head.scopes
        .get_mut(&scope_key)
        .unwrap()
        .binding
        .frozen_head_id = id("commit", 3);
    candidates.push(head);
    let mut hash = expected.clone();
    *hash
        .scopes
        .get_mut(&scope_key)
        .unwrap()
        .hidden
        .get_mut("a")
        .unwrap() = "0".repeat(64);
    candidates.push(hash);
    let mut snapshot = expected.clone();
    snapshot.binding.source.snapshot_hash = Some("0".repeat(64));
    candidates.push(snapshot);
    let mut target = expected.clone();
    target.binding.target.binding_generation += 1;
    candidates.push(target);
    let mut prior = expected.clone();
    prior.binding.predecessor.view_generation += 1;
    candidates.push(prior);
    for candidate in candidates {
        let prepared = encode_mask(&candidate).unwrap();
        let objects = objects(&prepared);
        assert!(matches!(
            source.verify_mask(prepared.hash(), |kind, hash| read(&objects, kind, hash)),
            Err(ProjectionError::MaskMismatch)
        ));
    }
}

#[test]
fn membership_registry_occurrence_every_required_page_is_authenticated_without_fallback() {
    let (_temp, mut engine, _) = fixture(true);
    let state = Arc::make_mut(&mut engine.state);
    let mut entries = Vec::new();
    for n in 100..430 {
        let mut version = state.versions[&id("ver", 1)].clone();
        version.version_id = id("ver", n);
        version.file_id = id("file", n);
        state.versions.insert(version.version_id.clone(), version);
        let mut placement = state.placements[&id("ver", 1)].clone();
        placement.version_id = id("ver", n);
        state
            .placements
            .insert(placement.version_id.clone(), placement);
        entries.push(file(n, n, &format!("item-{n}")));
    }
    state.commits.get_mut(&id("commit", 1)).unwrap().entries = entries;
    engine.snapshot = Some(
        crate::journal::prepare_snapshot(&engine.state, MAX_METADATA)
            .unwrap()
            .hash,
    );
    let base = registry(&engine.prepare_disk_registry_base().unwrap());
    let source = FrozenDetachSource::capture(&engine, &base, &target(&base, "P01")).unwrap();
    let prepared = source.prepare_mask().unwrap();
    let all = objects(&prepared);
    assert!(all.keys().any(|(kind, _)| kind == "metadata-index"));
    assert!(all.keys().filter(|(kind, _)| kind == "state-page").count() > 3);
    source
        .verify_mask(prepared.hash(), |kind, hash| read(&all, kind, hash))
        .unwrap();
    for key in all.keys() {
        let mut missing = all.clone();
        missing.remove(key);
        assert!(
            source
                .verify_mask(prepared.hash(), |kind, hash| read(&missing, kind, hash))
                .is_err()
        );
        let mut corrupt = all.clone();
        corrupt.get_mut(key).unwrap().push(b'x');
        assert!(
            source
                .verify_mask(prepared.hash(), |kind, hash| read(&corrupt, kind, hash))
                .is_err()
        );
    }
    assert!(
        source
            .verify_mask(prepared.hash(), |_, _| Ok(vec![b' '; MAX_METADATA + 1]))
            .is_err()
    );
}

fn workspace() -> Workspace {
    Workspace {
        workspace_id: id("ws", 99),
        purpose: WorkspacePurpose::Normal,
        branch_id: id("br", 1),
        principal: "admin".into(),
        base_head: id("commit", 1),
        fencing_token: 9,
        files: BTreeMap::new(),
        selected_file_branches: BTreeMap::new(),
        selected_file_contexts: BTreeMap::new(),
        selected_directory_branches: BTreeMap::new(),
        directory_selection_generations: BTreeMap::new(),
        file_selection_generations: BTreeMap::new(),
        namespace_operations: Vec::new(),
        created_at: 0,
        state: WorkspaceState::Open,
    }
}
#[test]
fn membership_registry_occurrence_workspace_proof_rejects_old_identity_scope_head_and_view() {
    let (_temp, engine, base) = fixture(true);
    let source = FrozenDetachSource::capture(&engine, &base, &target(&base, "P01")).unwrap();
    let prepared = source.prepare_mask().unwrap();
    let objects = objects(&prepared);
    let verified = source
        .verify_mask(prepared.hash(), |kind, hash| read(&objects, kind, hash))
        .unwrap();
    let next = successor(&base, prepared.hash(), 1);
    let view = verified.prepare_current_namespaces(&next).unwrap().view;
    let current = workspace();
    // This is a private metadata fixture. A production workspace additionally
    // needs the permanent admission receipt exercised by current_namespace_tests.
    let baseline = view.workspace_baseline(&engine.state, &current).unwrap();
    view.validate_workspace(&engine.state, &current, Some(&baseline))
        .unwrap();
    assert!(
        view.validate_workspace(&engine.state, &current, None)
            .is_err()
    );
    let mut old = current.clone();
    old.workspace_id = id("ws", 1);
    assert!(
        view.validate_workspace(&engine.state, &old, Some(&baseline))
            .is_err()
    );
    let mut old = current.clone();
    old.base_head = id("commit", 3);
    assert!(
        view.validate_workspace(&engine.state, &old, Some(&baseline))
            .is_err()
    );
    let mut old = current.clone();
    old.fencing_token += 1;
    assert!(
        view.validate_workspace(&engine.state, &old, Some(&baseline))
            .is_err()
    );
    let mut old = current.clone();
    old.principal = "other".into();
    assert!(
        view.validate_workspace(&engine.state, &old, Some(&baseline))
            .is_err()
    );
    let mut other_scope = current.clone();
    other_scope.branch_id = id("br", 2);
    assert!(
        view.validate_workspace(&engine.state, &other_scope, Some(&baseline))
            .is_err()
    );
    let other_decision = successor(&base, prepared.hash(), 2);
    let other_view = verified
        .prepare_current_namespaces(&other_decision)
        .unwrap()
        .view;
    assert!(
        other_view
            .validate_workspace(&engine.state, &current, Some(&baseline))
            .is_err()
    );
    let mut forged_generation = next.clone();
    forged_generation.commitment.view_generation += 1;
    assert!(matches!(
        verified.prepare_current_namespaces(&forged_generation),
        Err(ProjectionError::BaselineMismatch)
    ));
}

#[test]
fn membership_registry_occurrence_foreign_or_stale_registry_source_is_rejected() {
    let (_temp, mut engine, base) = fixture(true);
    let target = target(&base, "P01");
    engine.snapshot = Some("a".repeat(64));
    assert!(matches!(
        FrozenDetachSource::capture(&engine, &base, &target),
        Err(ProjectionError::SourceMismatch(_))
    ));
    engine.snapshot = base.state.source.snapshot_hash.clone();
    Arc::make_mut(&mut engine.state).sequence += 1;
    assert!(FrozenDetachSource::capture(&engine, &base, &target).is_err());
    Arc::make_mut(&mut engine.state).sequence -= 1;
    let mut changed = base.state.clone();
    changed.source.pool_id = "foreign".into();
    let foreign = registry(&encode_registry(changed).unwrap());
    assert!(FrozenDetachSource::capture(&engine, &foreign, &target).is_err());
    let mut target = target;
    target.binding_generation += 1;
    assert!(matches!(
        FrozenDetachSource::capture(&engine, &base, &target),
        Err(ProjectionError::Preview(_))
    ));
    engine.snapshot = None;
    let base = registry(&engine.prepare_disk_registry_base().unwrap());
    assert!(FrozenDetachSource::capture(&engine, &base, &target).is_err());
}

#[test]
fn membership_registry_occurrence_projection_preserves_history_and_original_file_bytes() {
    let (temp, engine, base) = fixture(true);
    fs::write(
        engine.config.disks[0].root.join("untouched-user-content"),
        b"physical bytes are not erased",
    )
    .unwrap();
    let before_files = files(temp.path());
    let before_state = canonical_json(engine.state()).unwrap();
    let before_config = canonical_json(engine.config()).unwrap();
    let history: Vec<_> = engine
        .state
        .commits
        .values()
        .map(|commit| canonical_json(commit).unwrap())
        .collect();
    let source = FrozenDetachSource::capture(&engine, &base, &target(&base, "P01")).unwrap();
    let prepared = source.prepare_mask().unwrap();
    let objects = objects(&prepared);
    let verified = source
        .verify_mask(prepared.hash(), |kind, hash| read(&objects, kind, hash))
        .unwrap();
    let next = successor(&base, prepared.hash(), 1);
    let view = verified.prepare_current_namespaces(&next).unwrap().view;
    for (id, branch) in &engine.state.branches {
        assert!(view.branch_entries(id, &branch.head_commit_id).is_ok());
    }
    for (id, branch) in &engine.state.directory_branches {
        assert!(view.directory_entries(id, &branch.head_tree_id).is_ok());
    }
    assert_eq!(canonical_json(engine.state()).unwrap(), before_state);
    assert_eq!(canonical_json(engine.config()).unwrap(), before_config);
    assert_eq!(
        engine
            .state
            .commits
            .values()
            .map(|commit| canonical_json(commit).unwrap())
            .collect::<Vec<_>>(),
        history
    );
    assert_eq!(files(temp.path()), before_files);
}

#[test]
fn membership_registry_occurrence_strict_root_rejects_future_intent_and_unknown_fields() {
    let (_temp, engine, base) = fixture(true);
    let source = FrozenDetachSource::capture(&engine, &base, &target(&base, "P01")).unwrap();
    let prepared = source.prepare_mask().unwrap();
    let mut objects = objects(&prepared);
    let root_bytes = read(&objects, MASK_KIND, prepared.hash()).unwrap();
    let mut root: json_codec::Value = crate::decode_strict(&root_bytes).unwrap();
    root.as_object_mut()
        .unwrap()
        .insert("intent_hash".into(), "1".repeat(64).into());
    let hash = object_hash(MASK_KIND, &root).unwrap();
    objects.insert(
        (MASK_KIND.into(), hash.clone()),
        canonical_json(&root).unwrap(),
    );
    assert!(
        source
            .verify_mask(&hash, |kind, hash| read(&objects, kind, hash))
            .is_err()
    );
    assert!(
        !String::from_utf8(root_bytes)
            .unwrap()
            .contains("intent_hash")
    );
}
