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
        let mut config = storage_contract::Config::development(crate::test_disks(
            (0..5).map(|index| root.path().join(format!("disk-{index}"))),
        ))
        .unwrap();
        config.disks[3].level = 1;
        config.disks[3].upstream_slot_id = None;
        config.disks[3].metadata_member = false;
        config.disks[4].level = 2;
        config.disks[4].upstream_slot_id = Some("D04".into());
        config.disks[4].metadata_member = false;
        let target_disk = config.disks[0].disk_id.clone();
        let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
        // Both the primary that exits and the one that keeps its namespace
        // have a real, retained registration on the initially enabled disk.
        engine.create_disk_root("P01", "owner", op()).unwrap();
        let repo = engine.create_disk_root("D04", "owner", op()).unwrap();
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
        let tree = engine.namespace_tree(&repo.main_branch_id).unwrap();
        let removed = tree.get("dir/a").unwrap().clone();
        let retained = tree.get("dir/b").unwrap().clone();
        // Each current pending source belongs to an actual separate FILE
        // branch, not to an inherited directory branch or mutable preview.
        for entry in [&removed, &retained] {
            let created = engine.create_file_branch(&entry.file_id, &entry.version_id,
                &storage_contract::BranchName::parse("pending-read").unwrap(), "owner", op()).unwrap();
            let branch_id = created.branch_id.as_deref().expect("new branch of this FILE");
            engine.select_file_branch(&entry.file_id, branch_id, "owner", op()).unwrap();
        }
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
    // A historical request uses its own frozen pending map, not the newer
    // selected FILE branch, even when it names the current structural HEAD.
    let original_history = fixture.backend.tree_snapshot(&fixture.identity, ReadTree {
        branch: &fixture.repo.main_branch_id, commit: Some(&fixture.head),
    }).unwrap();
    fixture.exit(MembershipExitAction::Deregister);
    let page = fixture
        .listing(
            "D04",
            "dir",
            &[
                ("limit", "1"),
                ("selected_path", "dir/a"),
                ("selected_entry_id", &fixture.removed.entry_id),
            ],
        )
        .unwrap();
    assert_eq!(names(&page), ["b"]);
    assert_eq!((page.total_entries, page.directory_file_count), (1, 1));
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
    assert_eq!(historical.entries, original_history.entries);
    assert_eq!(historical.pending_file_versions, original_history.pending_file_versions);
    assert!(historical.entries.iter().any(|entry| entry == &fixture.removed));
    assert_eq!(fixture.listing("P01", "", &[]).err().unwrap().code, ErrorCode::DiskNotFound);
    drop(fixture.backend);
    let reopened = EngineBackend::new(Engine::open(&fixture._root.path().join("system")).unwrap());
    let cold = reopened.listing(&fixture.identity, listing_request("D04", "dir", &[
        ("limit", "1"), ("selected_path", "dir/b"), ("selected_entry_id", &fixture.retained.entry_id)
    ])).unwrap();
    assert_eq!(names(&cold), ["b"]);
    assert_eq!((cold.total_entries, cold.directory_file_count), (1,1));
    assert_eq!(cold.selected_entry.as_ref().unwrap().entry_id, fixture.retained.entry_id);
    assert!(cold.previous_cursor.is_none() && cold.next_cursor.is_none());
    let cold_history = reopened
        .tree_snapshot(
            &fixture.identity,
            ReadTree {
                branch: &fixture.repo.main_branch_id,
                commit: Some(&fixture.head),
            },
        )
        .unwrap();
    assert_eq!(cold_history.entries, original_history.entries);
    assert_eq!(
        cold_history.pending_file_versions,
        original_history.pending_file_versions
    );
}

#[test]
fn disabled_registration_keeps_current_listing_while_content_io_is_closed() {
    let fixture = Fixture::new();
    fixture.exit(MembershipExitAction::PlannedOffline);
    let page = fixture.listing("D04", "dir", &[]).unwrap();
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
    let target = engine
        .config()
        .disks
        .iter()
        .find(|disk| disk.disk_id == fixture.target_disk)
        .unwrap();
    std::fs::rename(&target.root, target.root.with_extension("preserved")).unwrap();
    std::fs::write(&target.root, b"stopped directory must not be reopened").unwrap();
    // Planning retains metadata only. Actual replica admission must fail before
    // resolving the stopped target's now-invalid filesystem path.
    let plan = engine.prepare_content(&fixture.removed.version_id).unwrap();
    let unavailable = plan.open().err().unwrap();
    assert_eq!(unavailable.code(), ErrorCode::ContentUnavailable);
    let storage_engine::Error::Context { source, .. } = unavailable else {
        panic!("replica failure must retain its concrete admission cause")
    };
    assert_eq!(source.code(), ErrorCode::DiskUnavailable);
    assert_eq!(
        std::fs::read(&target.root).unwrap(),
        b"stopped directory must not be reopened"
    );
}

#[test]
fn deregistration_rejects_detached_listing_scan_captured_with_the_same_immutable_head() {
    let fixture = Fixture::new();
    let plan = {
        let engine = fixture.backend.engine.lock().unwrap();
        let listing::ListingRead::Primary(plan) =
            listing::capture(&engine, listing_request("D04", "dir", &[])).unwrap()
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
            .map(|entry| entry.path().file_name())
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
            .map(|entry| entry.path().file_name())
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
            "dir",
            &[
                ("limit", "1"),
                ("selected_path", "dir/a"),
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
            "dir",
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
