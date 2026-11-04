//! Real current-view consumers across the retained-membership D3 publication.
//! Temporary directory tests only; no physical-device or deployment coverage.
use super::*;
use crate::{Backend, DirectorySelection, ReadDirectory, ReadTree};
use storage_contract::{CommitRequest, Entry, ErrorCode, OperationId};
use storage_engine::{MembershipExitAction, PlacementRuleUpdate};

struct Fixture {
    _root: tempfile::TempDir,
    backend: EngineBackend,
    identity: Identity,
    repo: Repository,
    removed: Entry,
    retained: Entry,
    head: String,
    frozen_branch: String,
    frozen_tree: String,
    target_disk: String,
}

fn op() -> OperationId {
    OperationId::generate().unwrap()
}
fn save(engine: &mut Engine, repo: &Repository, path: &str, bytes: &[u8]) {
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, bytes)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: op(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "current listing fixture".into(),
            request_digest: None,
        })
        .unwrap();
}
impl Fixture {
    fn new() -> Self {
        let root = crate::tests::private_tempdir();
        let mut config = storage_contract::Config::development(
            (0..5).map(|index| root.path().join(format!("disk-{index}"))),
        )
        .unwrap();
        config.disks[3].level = 1;
        config.disks[3].upstream_slot_id = None;
        config.disks[3].metadata_member = false;
        config.disks[4].level = 2;
        config.disks[4].upstream_slot_id = Some("D04".into());
        config.disks[4].metadata_member = false;
        let target_disk = config.disks[0].disk_id.clone();
        let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
        let repo = engine
            .create_repository(
                &storage_contract::RepositoryName::parse("Files").unwrap(),
                "D04",
                1,
                "owner",
                op(),
            )
            .unwrap();
        let rule = storage_contract::new_id("rule").unwrap();
        engine
            .update_placement_rule(PlacementRuleUpdate {
                rule_id: &rule,
                repository_id: &repo.repository_id,
                path_prefix: "dir/a",
                primary_slot_id: "P01",
                expected_generation: 0,
                principal: "owner",
                operation_id: op(),
            })
            .unwrap();
        save(&mut engine, &repo, "dir/a", b"exclusive target body");
        save(&mut engine, &repo, "dir/b", b"retained original");
        let backup = engine
            .backup_configuration(&repo.repository_id, "dir/b")
            .unwrap();
        engine
            .configure_backups(storage_engine::BackupConfigurationRequest {
                repository_id: &repo.repository_id,
                path: "dir/b",
                target_copies: 2,
                backup_prefixes: &BTreeMap::from([("D05".into(), "archive".into())]),
                expected_generation: backup.generation,
                principal: "owner",
                operation_id: op(),
            })
            .unwrap();
        save(&mut engine, &repo, "dir/b", b"retained replicated body");
        // Future writes use a retained primary; the exit itself never reroutes.
        engine
            .update_placement_rule(PlacementRuleUpdate {
                rule_id: &rule,
                repository_id: &repo.repository_id,
                path_prefix: "dir/a",
                primary_slot_id: "D04",
                expected_generation: 1,
                principal: "owner",
                operation_id: op(),
            })
            .unwrap();
        let path = name_policy::LookupPath::parse("dir").unwrap();
        engine
            .initialize_directory_scope(&repo.repository_id, Some(&path), "owner", op())
            .unwrap();
        let branch_name = storage_contract::BranchName::parse("frozen").unwrap();
        engine
            .create_directory_branches(
                &repo.repository_id,
                Some(&path),
                &branch_name,
                "owner",
                op(),
            )
            .unwrap();
        engine
            .select_directory_branches(
                &repo.repository_id,
                Some(&path),
                &branch_name,
                "owner",
                op(),
            )
            .unwrap();
        let frozen = engine
            .state()
            .directory_branches
            .values()
            .find(|branch| branch.repository_id == repo.repository_id && branch.name == "frozen")
            .unwrap();
        let frozen_branch = frozen.directory_branch_id.clone();
        let frozen_tree = frozen.head_tree_id.clone();
        let tree = engine.namespace_tree(&repo.main_branch_id).unwrap();
        let removed = tree.get("dir/a").unwrap().clone();
        let retained = tree.get("dir/b").unwrap().clone();
        let head = engine.state().branches[&repo.main_branch_id]
            .head_commit_id
            .clone();
        let identity = super::super::install_test_administrator(&mut engine, "owner");
        Self {
            _root: root,
            backend: EngineBackend::new(engine),
            identity,
            repo,
            removed,
            retained,
            head,
            frozen_branch,
            frozen_tree,
            target_disk,
        }
    }

    fn exit(&self, action: MembershipExitAction) {
        let mut engine = self.backend.engine.lock().unwrap();
        engine
            .begin_disk_exit(&self.target_disk, action, "owner", op())
            .unwrap();
        let archive = engine.build_managed_membership_control_archive().unwrap();
        engine
            .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
            .unwrap();
        engine.resume_membership_joint_decision().unwrap();
        engine.activate_pending_disk_exit().unwrap();
    }
    fn listing(
        &self,
        slot: &str,
        path: &str,
        fields: &[(&str, &str)],
    ) -> Result<crate::ListingSnapshot, ApiError> {
        self.backend
            .listing(&self.identity, listing_request(slot, path, fields))
    }
    fn directory(
        &self,
        selection: DirectorySelection<'_>,
    ) -> Result<crate::DirectorySnapshot, ApiError> {
        let entry_id = {
            let engine = self.backend.engine.lock().unwrap();
            engine
                .namespace_tree(&self.repo.main_branch_id)
                .unwrap()
                .get("dir")
                .unwrap()
                .entry_id
                .clone()
        };
        self.backend.read_directory(
            &self.identity,
            ReadDirectory {
                branch: &self.repo.main_branch_id,
                entry_id: &entry_id,
                path: Some(&name_policy::PortablePath::parse("dir").unwrap()),
                selection,
            },
        )
    }
}
fn listing_request(slot: &str, path: &str, fields: &[(&str, &str)]) -> crate::ReadListing {
    let mut query: BTreeMap<_, _> = fields
        .iter()
        .map(|(key, value)| ((*key).into(), (*value).into()))
        .collect();
    query.insert("path".into(), path.into());
    crate::ReadListing::parse(slot.into(), query).unwrap()
}
fn names(snapshot: &crate::ListingSnapshot) -> Vec<&str> {
    snapshot
        .entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect()
}

#[test]
fn deregistered_current_listing_tree_selection_and_pending_sources_share_projected_indices() {
    let fixture = Fixture::new();
    let before = fixture
        .backend
        .tree_snapshot(
            &fixture.identity,
            ReadTree {
                branch: &fixture.repo.main_branch_id,
                commit: None,
            },
        )
        .unwrap();
    assert!(
        before
            .pending_file_versions
            .contains_key(&fixture.removed.file_id)
    );
    fixture.exit(MembershipExitAction::Deregister);
    let page = fixture
        .listing(
            "D04",
            "Files/dir",
            &[
                ("limit", "1"),
                ("selected_path", "Files/dir/a"),
                ("selected_entry_id", &fixture.removed.entry_id),
            ],
        )
        .unwrap();
    assert_eq!(names(&page), ["b"]);
    assert_eq!((page.total_entries, page.directory_file_count), (1, 1));
    assert_eq!(page.directory_branches.len(), 2);
    assert!(
        page.directory_branches
            .iter()
            .all(|branch| branch.file_count == 1)
    );
    assert!(page.selected_entry.is_none());
    assert!(page.previous_cursor.is_none() && page.next_cursor.is_none());
    assert_eq!(page.entries[0].entry_id, fixture.retained.entry_id);
    let crate::ListingVersion::File { content, .. } = &page.entries[0].version else {
        panic!("file expected")
    };
    assert_eq!(
        content.source_version_id.as_deref(),
        Some(fixture.retained.version_id.as_str())
    );
    let current = fixture
        .backend
        .tree_snapshot(
            &fixture.identity,
            ReadTree {
                branch: &fixture.repo.main_branch_id,
                commit: None,
            },
        )
        .unwrap();
    assert!(
        !current
            .entries
            .iter()
            .any(|entry| entry.entry_id == fixture.removed.entry_id)
    );
    assert!(
        !current
            .pending_file_versions
            .contains_key(&fixture.removed.file_id)
    );
    assert_eq!(
        current.pending_file_versions[&fixture.retained.file_id],
        fixture.retained.version_id
    );
    assert_eq!(
        fixture
            .backend
            .resolve_entry(
                &fixture.identity,
                crate::ResolveEntry {
                    branch: &fixture.repo.main_branch_id,
                    commit: None,
                    path: &name_policy::LookupPath::parse("dir/a").unwrap(),
                }
            )
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    let branches = fixture
        .backend
        .branches_snapshot(&fixture.identity, &fixture.repo.repository_id)
        .unwrap();
    for branch in branches {
        assert!(
            !branch
                .pending_file_versions
                .contains_key(&fixture.removed.file_id)
        );
        if branch.file_id.as_deref() == Some(fixture.removed.file_id.as_str()) {
            assert!(branch.file.unwrap().content.version_id.is_none());
        }
    }
    let branch = fixture
        .backend
        .branch_snapshot(&fixture.identity, &fixture.repo.main_branch_id)
        .unwrap();
    assert_eq!(branch.pending_file_versions, current.pending_file_versions);
    let historical = fixture
        .backend
        .tree_snapshot(
            &fixture.identity,
            ReadTree {
                branch: &fixture.repo.main_branch_id,
                commit: Some(&fixture.head),
            },
        )
        .unwrap();
    assert_eq!(historical.entries, before.entries);
    assert_eq!(
        historical.pending_file_versions,
        before.pending_file_versions
    );
    let preview = fixture
        .listing(
            "D04",
            "Files/dir",
            &[
                ("directory_branch_id", &fixture.frozen_branch),
                ("expected_tree_id", &fixture.frozen_tree),
            ],
        )
        .unwrap();
    assert_eq!(names(&preview), ["a", "b"]);
    assert!(preview.preview);
    assert!(
        preview
            .directory_branches
            .iter()
            .all(|branch| branch.file_count == 2)
    );
    assert_eq!(
        fixture.listing("P01", "", &[]).err().unwrap().code,
        ErrorCode::DiskNotFound
    );
}

#[test]
fn disabled_registration_keeps_current_listing_while_content_io_is_closed() {
    let fixture = Fixture::new();
    fixture.exit(MembershipExitAction::PlannedOffline);
    let page = fixture.listing("D04", "Files/dir", &[]).unwrap();
    assert_eq!(names(&page), ["a", "b"]);
    assert_eq!(page.directory_file_count, 2);
    assert!(fixture.listing("P01", "", &[]).is_ok());
    let engine = fixture.backend.engine.lock().unwrap();
    assert_eq!(
        engine
            .registered_disk(&fixture.target_disk)
            .unwrap()
            .registration_mode,
        storage_engine::RegistrationMode::Disabled
    );
    assert_eq!(
        engine
            .prepare_content(&fixture.removed.version_id)
            .err()
            .unwrap()
            .code(),
        ErrorCode::DiskUnavailable
    );
}

#[test]
fn deregistration_rejects_detached_listing_scan_captured_with_the_same_immutable_head() {
    let fixture = Fixture::new();
    let plan = {
        let engine = fixture.backend.engine.lock().unwrap();
        let listing::ListingRead::Primary(plan) =
            listing::capture(&engine, listing_request("D04", "Files/dir", &[])).unwrap()
        else {
            panic!("primary expected")
        };
        plan
    };
    fixture.exit(MembershipExitAction::Deregister);
    let selected = plan.select().unwrap();
    let engine = fixture.backend.engine.lock().unwrap();
    assert_eq!(
        engine.state().branches[&fixture.repo.main_branch_id].head_commit_id,
        fixture.head
    );
    assert_eq!(
        selected.project(&engine).err().unwrap().code,
        ErrorCode::ListingChanged
    );
}

#[test]
fn deregistered_backup_listing_uses_retained_current_identity_and_pending_source() {
    let fixture = Fixture::new();
    fixture.exit(MembershipExitAction::Deregister);
    let page = fixture.listing("D05", "archive", &[]).unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].entry_id, fixture.retained.entry_id);
    assert_eq!(page.entries[0].historical_only, Some(false));
    let crate::ListingVersion::File {
        content,
        backup_version,
    } = &page.entries[0].version
    else {
        panic!("file expected")
    };
    assert_eq!(
        backup_version.as_deref(),
        Some(fixture.retained.version_id.as_str())
    );
    assert_eq!(
        content.source_version_id.as_deref(),
        Some(fixture.retained.version_id.as_str())
    );
}

#[test]
fn smb_current_directory_rebinds_after_d3_and_keeps_projected_owner_across_normal_writes() {
    let fixture = Fixture::new();
    let old = fixture.directory(DirectorySelection::Capture).unwrap();
    fixture.exit(MembershipExitAction::Deregister);
    assert_eq!(
        fixture
            .directory(DirectorySelection::Page {
                snapshot: Some(&old.snapshot),
                after: None,
            })
            .unwrap_err()
            .code,
        ErrorCode::SmbRebindRequired
    );
    let captured = fixture.directory(DirectorySelection::Capture).unwrap();
    {
        let mut engine = fixture.backend.engine.lock().unwrap();
        save(&mut engine, &fixture.repo, "dir/c", b"new current body");
    }
    let pinned = fixture
        .directory(DirectorySelection::Page {
            snapshot: Some(&captured.snapshot),
            after: None,
        })
        .unwrap();
    assert_eq!(
        pinned
            .entries
            .iter()
            .map(|entry| entry.entry.name())
            .collect::<Vec<_>>(),
        ["b"]
    );
    let current = fixture
        .directory(DirectorySelection::Page {
            snapshot: None,
            after: None,
        })
        .unwrap();
    assert_eq!(
        current
            .entries
            .iter()
            .map(|entry| entry.entry.name())
            .collect::<Vec<_>>(),
        ["b", "c"]
    );
    assert_ne!(current.commit_id, captured.commit_id);
}

#[test]
fn recreated_path_uses_new_entry_identity_without_reviving_deregistered_selection() {
    let fixture = Fixture::new();
    fixture.exit(MembershipExitAction::Deregister);
    {
        let mut engine = fixture.backend.engine.lock().unwrap();
        save(
            &mut engine,
            &fixture.repo,
            "dir/a",
            b"independent new identity",
        );
    }
    let first = fixture
        .listing(
            "D04",
            "Files/dir",
            &[
                ("limit", "1"),
                ("selected_path", "Files/dir/a"),
                ("selected_entry_id", &fixture.removed.entry_id),
            ],
        )
        .unwrap();
    assert_eq!(names(&first), ["a"]);
    assert_ne!(first.entries[0].entry_id, fixture.removed.entry_id);
    assert!(first.selected_entry.is_none());
    let next = fixture
        .listing(
            "D04",
            "Files/dir",
            &[
                ("limit", "1"),
                ("cursor", first.next_cursor.as_deref().unwrap()),
            ],
        )
        .unwrap();
    assert_eq!(names(&next), ["b"]);
    assert_eq!(next.entries[0].entry_id, fixture.retained.entry_id);
    assert_eq!(next.total_entries, 2);
}

#[test]
fn directory_scope_queries_do_not_fall_back_to_removed_or_reused_path_identity() {
    let fixture = Fixture::new();
    fixture.exit(MembershipExitAction::Deregister);
    let filter =
        crate::DirectoryBranchFilter::Directory(name_policy::LookupPath::parse("dir").unwrap());
    let request = || crate::ReadDirectoryScopes {
        repository_id: &fixture.repo.repository_id,
        filter: &filter,
        projection: crate::DirectoryBranchProjection::Summary,
    };
    let before = fixture
        .backend
        .directory_scopes(&fixture.identity, request())
        .unwrap();
    let crate::DirectoryScopeRows::Summary(before) = before.branches else {
        panic!("summary expected")
    };
    assert_eq!(before.len(), 2);
    let previous_id = before[0].directory_id.clone();
    {
        let mut engine = fixture.backend.engine.lock().unwrap();
        engine
            .file_namespace(
                &fixture.repo.repository_id,
                NamespaceOperation::parse(
                    storage_contract::NamespaceOperationKind::DeleteTree,
                    "dir",
                    None,
                )
                .unwrap(),
                "owner",
                op(),
            )
            .unwrap();
    }
    let absent = fixture
        .backend
        .directory_scopes(&fixture.identity, request())
        .unwrap();
    let crate::DirectoryScopeRows::Summary(absent) = absent.branches else {
        panic!("summary expected")
    };
    assert!(
        absent.is_empty(),
        "missing current identity must not revive an old branch by path"
    );
    {
        let mut engine = fixture.backend.engine.lock().unwrap();
        save(
            &mut engine,
            &fixture.repo,
            "dir/new",
            b"new directory identity",
        );
        engine
            .initialize_directory_scope(
                &fixture.repo.repository_id,
                Some(&name_policy::LookupPath::parse("dir").unwrap()),
                "owner",
                op(),
            )
            .unwrap();
    }
    let current = fixture
        .backend
        .directory_scopes(&fixture.identity, request())
        .unwrap();
    let crate::DirectoryScopeRows::Summary(current) = current.branches else {
        panic!("summary expected")
    };
    assert!(!current.is_empty());
    assert!(
        current
            .iter()
            .all(|branch| branch.directory_id != previous_id)
    );
}

#[test]
fn deregistration_rejects_backup_page_prepared_before_the_registry_changed() {
    let fixture = Fixture::new();
    let selected = {
        let engine = fixture.backend.engine.lock().unwrap();
        let listing::ListingRead::Backup(plan) =
            listing::capture(&engine, listing_request("D05", "archive", &[])).unwrap()
        else {
            panic!("backup expected")
        };
        plan.select().unwrap()
    };
    fixture.exit(MembershipExitAction::Deregister);
    let engine = fixture.backend.engine.lock().unwrap();
    assert_eq!(
        selected.project(&engine).err().unwrap().code,
        ErrorCode::ListingChanged
    );
}

#[test]
fn deregistered_scopes_project_payloads_but_keep_explicit_history() {
    let fixture = Fixture::new();
    let frozen = {
        let engine = fixture.backend.engine.lock().unwrap();
        engine.state().directory_branches[&fixture.frozen_branch].clone()
    };
    assert!(frozen.file_branches.contains_key(&fixture.removed.file_id));
    assert!(frozen.file_contexts.contains_key("dir/a"));
    fixture.exit(MembershipExitAction::Deregister);
    let filter =
        crate::DirectoryBranchFilter::Directory(name_policy::LookupPath::parse("dir").unwrap());
    let request = |projection| crate::ReadDirectoryScopes {
        repository_id: &fixture.repo.repository_id,
        filter: &filter,
        projection,
    };
    let full = fixture
        .backend
        .directory_scopes(
            &fixture.identity,
            request(crate::DirectoryBranchProjection::Full),
        )
        .unwrap();
    let crate::DirectoryScopeRows::Full(rows) = full.branches else {
        panic!("full scopes expected")
    };
    assert_eq!(rows.len(), 2);
    for row in rows {
        let branch = row.branch;
        assert_eq!(
            branch.entries.as_slice(),
            std::slice::from_ref(&fixture.retained)
        );
        assert!(!branch.file_branches.contains_key(&fixture.removed.file_id));
        assert!(!branch.file_contexts.contains_key("dir/a"));
        assert!(
            !branch
                .inherited_files
                .contains_key(&fixture.removed.file_id)
        );
        assert_eq!(branch.file_branches.len(), 1);
        assert_eq!(branch.file_contexts.len(), 1);
        if branch.directory_branch_id == fixture.frozen_branch {
            assert_eq!(branch.head_tree_id, fixture.frozen_tree);
            assert_eq!(
                branch.file_branches[&fixture.retained.file_id],
                frozen.file_branches[&fixture.retained.file_id]
            );
            assert_eq!(branch.file_contexts["dir/b"], frozen.file_contexts["dir/b"]);
            assert_eq!(
                branch.inherited_files.get(&fixture.retained.file_id),
                frozen.inherited_files.get(&fixture.retained.file_id)
            );
        }
    }
    let summary = fixture
        .backend
        .directory_scopes(
            &fixture.identity,
            request(crate::DirectoryBranchProjection::Summary),
        )
        .unwrap();
    let crate::DirectoryScopeRows::Summary(rows) = summary.branches else {
        panic!("summary scopes expected")
    };
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|branch| branch.file_count == 1));
    let selection = fixture
        .backend
        .directory_selection(
            &fixture.identity,
            crate::ReadDirectorySelection::parse(
                fixture.repo.repository_id.clone(),
                "dir",
                vec![crate::DirectorySelectionSourceInput {
                    directory_branch_id: fixture.frozen_branch.clone(),
                    expected_tree_id: fixture.frozen_tree.clone(),
                }],
                vec![crate::DirectorySelectionEntryInput {
                    entry_id: fixture.removed.entry_id.clone(),
                    path: "dir/a".into(),
                }],
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(selection.branches.len(), 1);
    assert_eq!(selection.branches[0].summary.file_count, 2);
    assert_eq!(selection.branches[0].entries.len(), 1);
    assert_eq!(
        selection.branches[0].entries[0].entry_id,
        fixture.removed.entry_id
    );
    let engine = fixture.backend.engine.lock().unwrap();
    assert_eq!(
        engine.state().directory_branches[&fixture.frozen_branch],
        frozen
    );
    assert_eq!(
        engine.state().directory_trees[&fixture.frozen_tree].entries,
        frozen.entries
    );
}

#[test]
fn all_current_scopes_hide_deregistered_directory_identities_and_retain_repository_root() {
    let fixture = Fixture::new();
    let (repo, hidden_id, hidden_branch, hidden_tree) = {
        let mut engine = fixture.backend.engine.lock().unwrap();
        let repo = engine
            .create_repository(
                &storage_contract::RepositoryName::parse("Removed").unwrap(),
                "P01",
                1,
                "owner",
                op(),
            )
            .unwrap();
        save(
            &mut engine,
            &repo,
            "only/file",
            b"target-exclusive directory",
        );
        for path in [None, Some(name_policy::LookupPath::parse("only").unwrap())] {
            engine
                .initialize_directory_scope(&repo.repository_id, path.as_ref(), "owner", op())
                .unwrap();
            engine
                .create_directory_branches(
                    &repo.repository_id,
                    path.as_ref(),
                    &storage_contract::BranchName::parse("frozen").unwrap(),
                    "owner",
                    op(),
                )
                .unwrap();
        }
        let hidden_id = engine
            .namespace_tree(&repo.main_branch_id)
            .unwrap()
            .get("only")
            .unwrap()
            .entry_id
            .clone();
        let hidden = engine
            .state()
            .directory_branches
            .values()
            .find(|branch| branch.directory_id == hidden_id && branch.name == "frozen")
            .unwrap();
        let branch = hidden.directory_branch_id.clone();
        let tree = hidden.head_tree_id.clone();
        (repo, hidden_id, branch, tree)
    };
    fixture.exit(MembershipExitAction::Deregister);
    {
        let engine = fixture.backend.engine.lock().unwrap();
        assert!(
            engine
                .namespace_tree(&repo.main_branch_id)
                .unwrap()
                .entries()
                .is_empty()
        );
        assert!(
            !engine.state().directory_trees[&hidden_tree]
                .entries
                .is_empty()
        );
    }
    for projection in [
        crate::DirectoryBranchProjection::Full,
        crate::DirectoryBranchProjection::Summary,
    ] {
        let snapshot = fixture
            .backend
            .directory_scopes(
                &fixture.identity,
                crate::ReadDirectoryScopes {
                    repository_id: &repo.repository_id,
                    filter: &crate::DirectoryBranchFilter::All,
                    projection,
                },
            )
            .unwrap();
        let ids: Vec<_> = match snapshot.branches {
            crate::DirectoryScopeRows::Full(rows) => rows
                .into_iter()
                .map(|row| {
                    assert!(row.branch.entries.is_empty());
                    (row.branch.directory_branch_id, row.branch.directory_id)
                })
                .collect(),
            crate::DirectoryScopeRows::Summary(rows) => rows
                .into_iter()
                .map(|row| {
                    assert_eq!(row.file_count, 0);
                    (row.directory_branch_id, row.directory_id)
                })
                .collect(),
        };
        assert_eq!(ids.len(), 2);
        assert!(ids.iter().all(|(_, id)| id == &repo.repository_id));
        assert!(
            ids.iter()
                .all(|(branch, id)| branch != &hidden_branch && id != &hidden_id)
        );
    }
    let history = fixture
        .backend
        .directory_history(
            &fixture.identity,
            storage_contract::directory_history::DirectoryHistoryRequest::parse(
                &repo.repository_id,
                &hidden_id,
                storage_contract::directory_history::DirectoryHistoryCollection::Branches,
                BTreeMap::new(),
            )
            .unwrap(),
            storage_contract::history_window::HistoryWindowCancellation::new(),
        )
        .unwrap();
    assert!(
        history
            .branches
            .iter()
            .any(|branch| branch.directory_branch_id == hidden_branch)
    );
}
