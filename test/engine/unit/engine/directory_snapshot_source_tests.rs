use super::*;
use crate::*;
use std::sync::{Mutex, MutexGuard};
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};

static SERIAL: Mutex<()> = Mutex::new(());

struct Fixture {
    _serial: MutexGuard<'static, ()>,
    _temp: tempfile::TempDir,
    engine: Engine,
    repository: Repository,
}
impl Fixture {
    fn new() -> Self {
        let serial = SERIAL.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut engine = Engine::initialize(
            &temp.path().join("system"),
            Config::development([temp.path().join("disk")]).unwrap(),
        ).unwrap();
        let repository = engine.create_repository(
            &RepositoryName::parse("snapshot sources").unwrap(),
            "P01", 1, "admin", OperationId::generate().unwrap(),
        ).unwrap();
        Self { _serial: serial, _temp: temp, engine, repository }
    }
    fn save(&mut self, path: &str, bytes: &[u8]) -> OperationResult {
        let workspace = self.engine.create_workspace(
            &self.repository.main_branch_id, "admin",
        ).unwrap();
        self.engine.stage_bytes(&workspace.workspace_id, path, bytes).unwrap();
        self.engine.commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "source fixture".into(),
            request_digest: None,
        }).unwrap()
    }
    fn capture(&self, root: Option<&str>, limits: DirectorySnapshotLimits)
        -> std::result::Result<DirectorySnapshotSourcePlan, DirectorySnapshotSourceError>
    {
        let path = root.map(|path| LookupPath::parse(path).unwrap());
        self.engine.prepare_directory_snapshot_source(
            self.repository.main_branch_id.parse().unwrap(),
            path.as_ref(), limits, HistoryWindowCancellation::new(),
        )
    }
    fn entry(&self, path: &str) -> Entry {
        self.engine.namespace_tree(&self.repository.main_branch_id).unwrap()
            .get(path).unwrap().clone()
    }
    fn delete(&mut self, path: &str) {
        let entry = self.entry(path);
        self.engine.soft_delete_file(DeleteFileRequest {
            target: FileDeletionTarget {
                repository_id: &HistoryRepositoryId::parse(
                    &self.repository.repository_id,
                ).unwrap(),
                file_id: &HistoryFileId::parse(&entry.file_id).unwrap(),
                path: &LookupPath::parse(path).unwrap(),
            },
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        }).unwrap();
    }
    fn no_leases(&self) {
        assert!(self.engine.reads.lock().unwrap().counts().unwrap().is_empty());
    }
}
fn verify(plan: DirectorySnapshotSourcePlan) -> VerifiedDirectorySnapshotSource {
    plan.verify(|_| Ok::<(), ()>(())).ok().unwrap()
}
fn file(entry: &DirectorySnapshotEntry) -> (FileId, VersionId, u64, ContentHash) {
    match entry.content {
        DirectorySnapshotContent::File { file_id, version_id, size, content_hash } => {
            (file_id, version_id, size, content_hash)
        }
        DirectorySnapshotContent::Directory => panic!("expected a file preview"),
    }
}

#[test]
fn directory_snapshot_preserves_smb_spelling_and_verifies_empty_and_nested_bodies() {
    let mut fixture = Fixture::new();
    fixture.save("Straße/Café.txt", b"original");
    fixture.save("Straße/Sub/EMPTY", b"");
    fixture.save("Outside", b"not in the selected root");
    let sequence = fixture.engine.state.sequence;
    let plan = fixture.capture(Some("STRASSE"), DirectorySnapshotLimits::default()).unwrap();
    let verified = verify(plan);
    assert_eq!(fixture.engine.reads.lock().unwrap().counts().unwrap().len(), 2);
    let preview = fixture.engine.finish_directory_snapshot_preview(verified).unwrap();
    let paths: Vec<_> = preview.entries.iter().map(|entry| entry.path.as_str()).collect();
    assert_eq!(paths, ["Café.txt", "Sub", "Sub/EMPTY"]);
    assert_eq!(preview.root.unwrap().as_str(), "Straße");
    assert_eq!(file(&preview.entries[0]).3, ContentHash::from_digest(blake3::hash(b"original")));
    assert_eq!(file(&preview.entries[2]).2, 0);
    assert_eq!(file(&preview.entries[2]).3, ContentHash::from_digest(blake3::hash(b"")));
    assert_eq!(preview.usage.unique_versions, 2);
    assert_eq!(fixture.engine.state.sequence, sequence);
    fixture.no_leases();
}

#[test]
fn directory_snapshot_freezes_selected_old_version_and_ignores_unpublished_workspace() {
    let mut fixture = Fixture::new();
    fixture.save("report.txt", b"old exposed body");
    let original = fixture.entry("report.txt");
    let branch = fixture.engine.create_file_branch(
        &original.file_id, &original.version_id, &BranchName::parse("old selection").unwrap(),
        "admin", OperationId::generate().unwrap(),
    ).unwrap().branch_id.unwrap();
    fixture.save("report.txt", b"newer saved body");
    fixture.engine.select_file_branch(
        &original.file_id, &branch, "admin", OperationId::generate().unwrap(),
    ).unwrap();
    assert_eq!(fixture.entry("report.txt").version_id, original.version_id);
    let plan = fixture.capture(None, DirectorySnapshotLimits::default()).unwrap();
    let workspace = fixture.engine.create_workspace(
        &fixture.repository.main_branch_id, "admin",
    ).unwrap();
    fixture.engine.stage_bytes(&workspace.workspace_id, "report.txt", b"unpublished").unwrap();
    let preview = fixture.engine.finish_directory_snapshot_preview(verify(plan)).unwrap();
    assert_eq!(file(&preview.entries[0]).1.to_string(), original.version_id);
    assert_eq!(file(&preview.entries[0]).3,
        ContentHash::from_digest(blake3::hash(b"old exposed body")));
    fixture.no_leases();
}

#[test]
fn directory_snapshot_normal_save_after_capture_never_reselects_the_new_body() {
    let mut fixture = Fixture::new();
    fixture.save("report.txt", b"captured");
    let original = fixture.entry("report.txt");
    let plan = fixture.capture(None, DirectorySnapshotLimits::default()).unwrap();
    fixture.save("report.txt", b"later");
    let preview = fixture.engine.finish_directory_snapshot_preview(verify(plan)).unwrap();
    assert_eq!(file(&preview.entries[0]).1.to_string(), original.version_id);
    assert_ne!(fixture.entry("report.txt").version_id, original.version_id);
    fixture.no_leases();
}

#[test]
fn directory_snapshot_soft_delete_hides_capture_and_revokes_a_verified_preview() {
    let mut fixture = Fixture::new();
    fixture.save("A", b"A");
    fixture.save("B", b"B");
    let entry = fixture.entry("A");
    let branch = fixture.engine.create_file_branch(
        &entry.file_id, &entry.version_id, &BranchName::parse("deletion source").unwrap(),
        "admin", OperationId::generate().unwrap(),
    ).unwrap().branch_id.unwrap();
    fixture.engine.select_file_branch(
        &entry.file_id, &branch, "admin", OperationId::generate().unwrap(),
    ).unwrap();
    // Establish the independent file branch before capture so only deletion
    // can revoke the verified source below.
    let verified = verify(fixture.capture(None, DirectorySnapshotLimits::default()).unwrap());
    fixture.delete("A");
    assert!(matches!(
        fixture.engine.finish_directory_snapshot_preview(verified),
        Err(DirectorySnapshotSourceError::SourceChanged),
    ));
    fixture.no_leases();
    let preview = fixture.engine.finish_directory_snapshot_preview(
        verify(fixture.capture(None, DirectorySnapshotLimits::default()).unwrap()),
    ).unwrap();
    assert_eq!(preview.entries.len(), 1);
    assert_eq!(preview.entries[0].path.as_str(), "B");
}

#[test]
fn directory_snapshot_rejects_visible_management_components_at_every_depth() {
    let mut fixture = Fixture::new();
    fixture.save("Work/Nested/.ZXS/body", b"not an admitted management tree");
    assert!(matches!(
        fixture.capture(Some("Work"), DirectorySnapshotLimits::default()),
        Err(DirectorySnapshotSourceError::ManagementConflict(_)),
    ));
    fixture.no_leases();
    fixture.engine.file_namespace(
        &fixture.repository.repository_id,
        NamespaceOperation::parse(NamespaceOperationKind::Mkdir, ".zxs", None).unwrap(),
        "admin", OperationId::generate().unwrap(),
    ).unwrap();
    assert!(matches!(
        fixture.capture(Some(".zxs"), DirectorySnapshotLimits::default()),
        Err(DirectorySnapshotSourceError::ManagementConflict(_)),
    ));
}

#[test]
fn directory_snapshot_resource_limits_fail_whole_capture_and_release_partial_leases() {
    let mut fixture = Fixture::new();
    fixture.save("A", b"a");
    fixture.save("B", b"b");
    let successful = fixture.engine.finish_directory_snapshot_preview(
        verify(fixture.capture(None, DirectorySnapshotLimits::default()).unwrap()),
    ).unwrap().usage;
    let cases = [
        (DirectorySnapshotBudget::Entries, successful.entries),
        (DirectorySnapshotBudget::PathBytes, successful.path_bytes),
        (DirectorySnapshotBudget::ManifestBytes, successful.manifest_bytes),
        (DirectorySnapshotBudget::UniqueVersions, successful.unique_versions),
        (DirectorySnapshotBudget::RouteReferences, successful.route_references),
        (DirectorySnapshotBudget::ProofBytes, successful.proof_bytes),
        (DirectorySnapshotBudget::ScannedEntries, successful.scanned_entries),
    ];
    for (kind, exact) in cases {
        for (maximum, accepted) in [(exact, true), (exact - 1, false)] {
            let mut limits = DirectorySnapshotLimits::default();
            match kind {
                DirectorySnapshotBudget::Entries => limits.entries = maximum,
                DirectorySnapshotBudget::PathBytes => limits.path_bytes = maximum,
                DirectorySnapshotBudget::ManifestBytes => limits.manifest_bytes = maximum,
                DirectorySnapshotBudget::UniqueVersions => limits.unique_versions = maximum,
                DirectorySnapshotBudget::RouteReferences => limits.route_references = maximum,
                DirectorySnapshotBudget::ProofBytes => limits.proof_bytes = maximum,
                DirectorySnapshotBudget::ScannedEntries => limits.scanned_entries = maximum,
                _ => panic!("the table contains only independent capture budgets"),
            }
            match fixture.capture(None, limits) {
                Ok(plan) if accepted => drop(plan),
                Err(DirectorySnapshotSourceError::Budget(actual)) if !accepted => {
                    assert_eq!(actual, kind);
                }
                _ => panic!("budget boundary was not enforced: {kind:?}"),
            }
            fixture.no_leases();
        }
    }
    let mut limits = DirectorySnapshotLimits::default();
    limits.logical_bytes = 2;
    drop(fixture.capture(None, limits).unwrap());
    limits.logical_bytes = 1;
    assert!(matches!(fixture.capture(None, limits),
        Err(DirectorySnapshotSourceError::Budget(DirectorySnapshotBudget::LogicalBytes))));
    limits = DirectorySnapshotLimits::default();
    limits.routes_per_version = 0;
    assert!(matches!(fixture.capture(None, limits),
        Err(DirectorySnapshotSourceError::Budget(DirectorySnapshotBudget::RoutesPerVersion))));
    limits = DirectorySnapshotLimits::default();
    limits.work_units = 0;
    assert!(matches!(fixture.capture(None, limits),
        Err(DirectorySnapshotSourceError::Budget(DirectorySnapshotBudget::WorkUnits))));
    fixture.no_leases();
}

#[test]
fn directory_snapshot_all_leases_survive_each_body_and_release_on_abort_drop_and_expiry() {
    let mut fixture = Fixture::new();
    fixture.save("A", &[1; 131_073]);
    fixture.save("B", b"b");
    let cancellation = HistoryWindowCancellation::new();
    let plan = fixture.engine.prepare_directory_snapshot_source(
        fixture.repository.main_branch_id.parse().unwrap(), None,
        DirectorySnapshotLimits::default(), cancellation.clone(),
    ).unwrap();
    let mut calls = 0;
    let outcome = plan.verify(|_| {
        assert_eq!(fixture.engine.reads.lock().unwrap().counts().unwrap().len(), 2);
        calls += 1;
        if calls == 3 { cancellation.cancel(); }
        Ok::<(), ()>(())
    });
    assert!(matches!(outcome,
        Err(DirectorySnapshotVerificationError::Source(DirectorySnapshotSourceError::Cancelled))));
    fixture.no_leases();
    fixture.capture(None, DirectorySnapshotLimits::default()).unwrap().cancel();
    fixture.no_leases();
    drop(fixture.capture(None, DirectorySnapshotLimits::default()).unwrap());
    fixture.no_leases();
    let mut limits = DirectorySnapshotLimits::default();
    limits.duration = Duration::ZERO;
    assert!(matches!(fixture.capture(None, limits), Err(DirectorySnapshotSourceError::Expired)));
    fixture.no_leases();
}

#[test]
fn directory_snapshot_reauthorizes_bounded_reads_and_never_returns_on_revocation() {
    let mut fixture = Fixture::new();
    fixture.save("A", &[9; 131_073]);
    let plan = fixture.capture(None, DirectorySnapshotLimits::default()).unwrap();
    let mut calls = 0;
    let outcome = plan.verify(|_| {
        calls += 1;
        if calls == 3 { Err("current identity revoked") } else { Ok(()) }
    });
    assert!(matches!(outcome,
        Err(DirectorySnapshotVerificationError::Authorization("current identity revoked"))));
    assert_eq!(calls, 3);
    fixture.no_leases();
}

#[test]
fn directory_snapshot_source_missing_or_corrupt_is_not_a_metadata_only_success() {
    let mut fixture = Fixture::new();
    fixture.save("A", b"body");
    let entry = fixture.entry("A");
    let plan = fixture.capture(None, DirectorySnapshotLimits::default()).unwrap();
    let (source, _) = fixture.engine.valid_replica(&entry.version_id).unwrap();
    drop(source);
    let disk = &fixture.engine.config.disks[0];
    let version = &fixture.engine.state.versions[&entry.version_id];
    let directory = crate::disk::version_dir(disk, &version.history_tree_id, &version.version_id);
    let handle = fixture.engine.disk_resources.get(disk).unwrap();
    let body = crate::container_storage::content_path_at(
        &handle.root, &directory, version, &mut |_| Ok(()),
    ).unwrap();
    let original_permissions = std::fs::metadata(&body).unwrap().permissions();
    std::fs::set_permissions(&body, std::os::unix::fs::PermissionsExt::from_mode(0o600))
        .unwrap();
    std::fs::write(&body, b"evil").unwrap();
    std::fs::set_permissions(&body, original_permissions).unwrap();
    assert_eq!(std::fs::read(&body).unwrap(), b"evil");
    assert!(plan.verify(|_| Ok::<(), ()>(())).is_err());
    fixture.no_leases();
    let plan = fixture.capture(None, DirectorySnapshotLimits::default()).unwrap();
    std::fs::remove_file(&body).unwrap();
    assert!(plan.verify(|_| Ok::<(), ()>(())).is_err());
    fixture.no_leases();
}

#[test]
fn directory_snapshot_task_admission_is_bounded_and_drop_releases_the_slot() {
    let fixture = Fixture::new();
    let first = fixture.capture(None, DirectorySnapshotLimits::default()).unwrap();
    assert!(matches!(fixture.capture(None, DirectorySnapshotLimits::default()),
        Err(DirectorySnapshotSourceError::Budget(DirectorySnapshotBudget::ConcurrentTasks))));
    drop(first);
    let second = fixture.capture(None, DirectorySnapshotLimits::default()).unwrap();
    let verified = verify(second);
    assert!(matches!(fixture.capture(None, DirectorySnapshotLimits::default()),
        Err(DirectorySnapshotSourceError::Budget(DirectorySnapshotBudget::ConcurrentTasks))));
    drop(verified);
    drop(fixture.capture(None, DirectorySnapshotLimits::default()).unwrap());
}

#[test]
fn directory_snapshot_split_placeholder_reads_its_actual_inherited_source_version() {
    let mut fixture = Fixture::new();
    let saved = fixture.save("report.txt", b"inherited body");
    let original = fixture.entry("report.txt");
    let pending = fixture.engine.create_branch(
        saved.commit_id.as_ref().unwrap(), &BranchName::parse("pending split").unwrap(),
        "admin", OperationId::generate().unwrap(),
    ).unwrap();
    let split = fixture.engine.create_split_plan(
        &fixture.repository.history_tree_id, &pending.head_commit_id, "admin",
    ).unwrap();
    let receipt = fixture.engine.commit_plan(
        &split.plan_id, "admin", OperationId::generate().unwrap(),
    ).unwrap();
    let repository = fixture.engine.state.repositories.values().find(|repository| {
        Some(&repository.history_tree_id) == receipt.history_tree_id.as_ref()
    }).unwrap();
    let placeholder = fixture.engine.namespace_tree(&repository.main_branch_id).unwrap()
        .get("report.txt").unwrap();
    let source = fixture.engine.pending_file_source(
        &repository.main_branch_id, &placeholder.file_id,
    ).unwrap();
    // Split remaps the retained body into the new history while the branch
    // still inherits that body instead of owning a first saved version.
    assert_ne!(placeholder.file_id, original.file_id);
    assert_ne!(source, original.version_id.as_str());
    assert_eq!(placeholder.version_id, source);
    let inherited = &fixture.engine.state.versions[source];
    assert_eq!(inherited.file_id, placeholder.file_id);
    assert_eq!(inherited.history_tree_id, repository.history_tree_id);
    let mut content = fixture.engine.open_content(source).unwrap();
    let mut bytes = Vec::new();
    content.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"inherited body");
    drop(content);
    let plan = fixture.engine.prepare_directory_snapshot_source(
        repository.main_branch_id.parse().unwrap(), None,
        DirectorySnapshotLimits::default(), HistoryWindowCancellation::new(),
    ).unwrap();
    let preview = fixture.engine.finish_directory_snapshot_preview(verify(plan)).unwrap();
    assert_eq!(preview.entries.len(), 1);
    assert_eq!(preview.entries[0].path.as_str(), "report.txt");
    assert_eq!(preview.entries[0].entry_id.to_string(), placeholder.entry_id);
    assert_eq!(file(&preview.entries[0]).0.to_string(), placeholder.file_id);
    assert_eq!(file(&preview.entries[0]).1.to_string(), source);
    assert_eq!(file(&preview.entries[0]).2, b"inherited body".len() as u64);
    assert_eq!(file(&preview.entries[0]).3,
        ContentHash::from_digest(blake3::hash(b"inherited body")));
    fixture.no_leases();
}

#[test]
fn directory_snapshot_keeps_all_alias_paths_and_counts_logical_bytes_per_occurrence() {
    let mut fixture = Fixture::new();
    fixture.save("Source", b"old");
    let old = fixture.entry("Source");
    fixture.save("Source", b"current");
    let current = fixture.entry("Source");
    let branch = fixture.repository.main_branch_id.clone();
    let operation = OperationId::generate().unwrap();
    // Publish an admitted alias namespace through the real Engine transaction,
    // rather than feeding an arbitrary vector directly to the capture routine.
    // Current occurrences share their FILE canonical leaf name, not their parent.
    fixture.engine.mutate("test.snapshot.aliases", "admin", operation,
        json_codec::json!({}), |_, state, _| {
            let previous = state.branches[&branch].head_commit_id.clone();
            let mut commit = state.commits[&previous].clone();
            let mut same = current.clone();
            same.entry_id = new_id("entry")?;
            same.relocate(PortablePath::parse("SameBody/Source").unwrap());
            crate::business::ensure_parents(&mut commit.entries, &same.path)?;
            let mut historical = old.clone();
            historical.entry_id = new_id("entry")?;
            historical.relocate(PortablePath::parse("OldBody/Source").unwrap());
            crate::business::ensure_parents(&mut commit.entries, &historical.path)?;
            commit.entries.extend([same, historical]);
            crate::business::validate_entries(&mut commit.entries, 2048)?;
            commit.commit_id = new_id("commit")?;
            commit.tree_id = new_id("tree")?;
            commit.parent_commit_ids = vec![previous];
            commit.operation_id = operation;
            commit.sequence = state.sequence;
            state.branches.get_mut(&branch).unwrap().head_commit_id = commit.commit_id.clone();
            state.commits.insert(commit.commit_id.clone(), commit);
            Ok(json_codec::json!({}))
        },
    ).unwrap();
    let plan = fixture.capture(None, DirectorySnapshotLimits::default()).unwrap();
    assert_eq!(fixture.engine.reads.lock().unwrap().counts().unwrap().len(), 2);
    let preview = fixture.engine.finish_directory_snapshot_preview(verify(plan)).unwrap();
    assert_eq!(preview.entries.len(), 5);
    assert_eq!(preview.usage.unique_versions, 2);
    assert_eq!(preview.usage.logical_bytes, 3 + 7 + 7);
    let paths: Vec<_> = preview.entries.iter().map(|entry| entry.path.as_str()).collect();
    assert_eq!(paths, ["OldBody", "OldBody/Source", "SameBody", "SameBody/Source", "Source"]);
    let files: Vec<_> = preview.entries.iter().filter(|entry| {
        matches!(entry.content, DirectorySnapshotContent::File { .. })
    }).collect();
    assert_eq!(files.len(), 3);
    assert_eq!(file(files[0]).0, file(files[1]).0);
    assert_eq!(file(files[1]).0, file(files[2]).0);
    assert_ne!(file(files[0]).1, file(files[1]).1);
    assert_eq!(file(files[1]).1, file(files[2]).1);
    assert_eq!(file(files[0]).2, 3);
    assert_eq!(file(files[1]).2, 7);
    assert_eq!(file(files[2]).2, 7);
    fixture.no_leases();
    let mut limits = DirectorySnapshotLimits::default();
    limits.logical_bytes = 3 + 7 + 7 - 1;
    assert!(matches!(fixture.capture(None, limits),
        Err(DirectorySnapshotSourceError::Budget(DirectorySnapshotBudget::LogicalBytes))));
    fixture.no_leases();
}

#[test]
fn directory_snapshot_root_bytes_are_charged_and_noncurrent_branches_are_rejected() {
    let mut fixture = Fixture::new();
    let root = "长".repeat(40);
    let saved = fixture.save(&format!("{root}/A"), b"a");
    let plan = fixture.capture(Some(&root), DirectorySnapshotLimits::default()).unwrap();
    let preview = fixture.engine.finish_directory_snapshot_preview(verify(plan)).unwrap();
    assert_eq!(preview.usage.manifest_bytes, 1024 + 6 * root.len() + 512 + 6);
    let mut limits = DirectorySnapshotLimits::default();
    limits.manifest_bytes = preview.usage.manifest_bytes - 1;
    assert!(matches!(fixture.capture(Some(&root), limits),
        Err(DirectorySnapshotSourceError::Budget(DirectorySnapshotBudget::ManifestBytes))));
    let branch = fixture.engine.create_branch(
        saved.commit_id.as_ref().unwrap(), &BranchName::parse("not exposed").unwrap(),
        "admin", OperationId::generate().unwrap(),
    ).unwrap();
    assert!(matches!(fixture.engine.prepare_directory_snapshot_source(
        branch.branch_id.parse().unwrap(), None, DirectorySnapshotLimits::default(),
        HistoryWindowCancellation::new(),
    ), Err(DirectorySnapshotSourceError::SourceChanged)));
    fixture.no_leases();
}

#[test]
fn directory_snapshot_global_slots_stop_at_two_distinct_engines() {
    let first = Fixture::new();
    let temp = tempfile::tempdir().unwrap();
    let mut second = Engine::initialize(
        &temp.path().join("second-system"),
        Config::development([temp.path().join("second-disk")]).unwrap(),
    ).unwrap();
    let second_repo = second.create_repository(
        &RepositoryName::parse("second").unwrap(), "P01", 1,
        "admin", OperationId::generate().unwrap(),
    ).unwrap();
    let mut third = Engine::initialize(
        &temp.path().join("third-system"),
        Config::development([temp.path().join("third-disk")]).unwrap(),
    ).unwrap();
    let third_repo = third.create_repository(
        &RepositoryName::parse("third").unwrap(), "P01", 1,
        "admin", OperationId::generate().unwrap(),
    ).unwrap();
    let one = first.capture(None, DirectorySnapshotLimits::default()).unwrap();
    let two = second.prepare_directory_snapshot_source(
        second_repo.main_branch_id.parse().unwrap(), None, DirectorySnapshotLimits::default(),
        HistoryWindowCancellation::new(),
    ).unwrap();
    let capture_third = || third.prepare_directory_snapshot_source(
        third_repo.main_branch_id.parse().unwrap(), None, DirectorySnapshotLimits::default(),
        HistoryWindowCancellation::new(),
    );
    assert!(matches!(capture_third(),
        Err(DirectorySnapshotSourceError::Budget(DirectorySnapshotBudget::ConcurrentTasks))));
    drop(one);
    drop(capture_third().unwrap());
    drop(two);
}

#[test]
fn directory_snapshot_initialization_retains_leases_and_requires_exact_current_inventory() {
    let mut fixture = Fixture::new();
    fixture.save("one.txt", b"first");
    let verified = verify(fixture.capture(None, DirectorySnapshotLimits::default()).unwrap());
    let frozen = fixture.engine.freeze_snapshot_initialization_source(verified).unwrap();
    assert_eq!(frozen.entries().len(), 1);
    assert!(!fixture.engine.reads.lock().unwrap().counts().unwrap().is_empty());
    drop(frozen);
    fixture.no_leases();

    let verified = verify(fixture.capture(None, DirectorySnapshotLimits::default()).unwrap());
    fixture.save("one.txt", b"changed after verification");
    assert!(matches!(fixture.engine.freeze_snapshot_initialization_source(verified),
        Err(DirectorySnapshotSourceError::SourceChanged)));
    fixture.no_leases();

    let verified = verify(fixture.capture(None, DirectorySnapshotLimits::default()).unwrap());
    fixture.save("added.txt", b"not in the captured full inventory");
    assert!(matches!(fixture.engine.freeze_snapshot_initialization_source(verified),
        Err(DirectorySnapshotSourceError::SourceChanged)));
    fixture.no_leases();
}
