//! Physical root-marker collisions must fail before the publication decision.
use super::*;

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    engine: Engine,
    repo: Repository,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("system");
        let config = Config::development([
            temporary.path().join("p"),
            temporary.path().join("b"),
            temporary.path().join("c"),
        ]);
        let mut engine = Engine::initialize(&root, config).unwrap();
        let repo = engine
            .create_repository(
                "Projection paths",
                "P01",
                3,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        Self {
            _temporary: temporary,
            root,
            engine,
            repo,
        }
    }

    fn save(&mut self, path: &str) -> Result<OperationResult> {
        let workspace = self
            .engine
            .create_workspace(&self.repo.main_branch_id, "admin")?;
        self.engine
            .stage_bytes(&workspace.workspace_id, path, b"preserved body")?;
        self.engine.commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "Projection path regression".into(),
            request_digest: None,
        })
    }

    fn rename(&mut self, path: &str, destination: &str) -> Result<OperationResult> {
        self.engine.file_namespace(
            &self.repo.repository_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Rename,
                path,
                Some(destination),
            )
            .unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
    }

    fn backups(&mut self, prefix: &str) -> Result<OperationResult> {
        self.engine.configure_backups(BackupConfigurationRequest {
            repository_id: &self.repo.repository_id,
            path: "existing.txt",
            target_copies: 3,
            backup_prefixes: &BTreeMap::from([
                ("B01".into(), prefix.into()),
                ("C01".into(), "cold".into()),
            ]),
            expected_generation: 0,
            principal: "admin",
            operation_id: storage_contract::OperationId::generate(),
        })
    }

    fn assert_writable_after_reopen(mut self) {
        self.save("unrelated.txt").unwrap();
        drop(self.engine);
        self.engine = Engine::open(&self.root).unwrap();
        self.save("after-reopen.txt").unwrap();
        let report = self.engine.check().unwrap();
        assert!(report.issues.is_empty(), "{:?}", report.issues);
        for version in self.engine.state().versions.keys() {
            let mut content = self.engine.open_content(version).unwrap();
            let mut bytes = Vec::new();
            content.file.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, b"preserved body");
        }
    }
}

struct PublishedSnapshot {
    state: Vec<u8>,
    journals: Vec<(PathBuf, Vec<u8>)>,
    bodies: Vec<(PathBuf, u64, Vec<u8>)>,
}

impl PublishedSnapshot {
    fn capture(engine: &Engine) -> Self {
        let journals = engine
            .config()
            .disks
            .iter()
            .filter(|disk| disk.metadata_member)
            .map(|disk| {
                let path = crate::journal::log_path(disk);
                let bytes = fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect();
        let mut bodies = Vec::new();
        for version in engine.state().versions.values() {
            for slot in &engine.state().placements[&version.version_id].route_slot_ids {
                let disk = engine
                    .config()
                    .disks
                    .iter()
                    .find(|d| &d.slot_id == slot)
                    .unwrap();
                let directory = version_dir(disk, &version.history_tree_id, &version.version_id);
                let path = content_path(&directory, version).unwrap();
                bodies.push((
                    path.clone(),
                    fs::metadata(&path).unwrap().ino(),
                    fs::read(path).unwrap(),
                ));
            }
        }
        Self {
            state: canonical_json(engine.state()).unwrap(),
            journals,
            bodies,
        }
    }

    fn assert_unchanged(&self, engine: &Engine) {
        assert_eq!(canonical_json(engine.state()).unwrap(), self.state);
        for (path, bytes) in &self.journals {
            assert_eq!(
                &fs::read(path).unwrap(),
                bytes,
                "no decision may be appended"
            );
        }
        for (path, inode, bytes) in &self.bodies {
            assert_eq!(fs::metadata(path).unwrap().ino(), *inode);
            assert_eq!(&fs::read(path).unwrap(), bytes);
        }
    }
}

#[test]
fn save_rejects_root_marker_and_equivalent_spellings_before_decision() {
    for path in [
        ".zxs-namespace.json",
        ".zxs-namespace.json/child.txt",
        ".ZXS-NAMESPACE.JSON",
        ".zxſ-namespace.json/child.txt",
    ] {
        let mut fixture = Fixture::new();
        fixture.save("existing.txt").unwrap();
        let before = PublishedSnapshot::capture(&fixture.engine);
        assert_eq!(
            fixture.save(path).unwrap_err().code(),
            storage_contract::ErrorCode::NameCollision,
            "{path}"
        );
        before.assert_unchanged(&fixture.engine);
        fixture.assert_writable_after_reopen();
    }
}

#[test]
fn file_and_directory_rename_reject_root_marker_before_decision() {
    for (source, destination) in [
        ("source/existing.txt", ".zxs-namespace.json"),
        ("source", ".zxs-namespace.json"),
        ("source", ".ZXS-NAMESPACE.JSON"),
    ] {
        let mut fixture = Fixture::new();
        fixture.save("source/existing.txt").unwrap();
        let before = PublishedSnapshot::capture(&fixture.engine);
        assert_eq!(
            fixture.rename(source, destination).unwrap_err().code(),
            storage_contract::ErrorCode::NameCollision
        );
        before.assert_unchanged(&fixture.engine);
        fixture.assert_writable_after_reopen();
    }
}

#[test]
fn backup_prefix_rejects_root_marker_before_decision() {
    for prefix in [".zxs-namespace.json", ".ZXS-NAMESPACE.JSON/archive"] {
        let mut fixture = Fixture::new();
        fixture.save("existing.txt").unwrap();
        let before = PublishedSnapshot::capture(&fixture.engine);
        assert_eq!(
            fixture.backups(prefix).unwrap_err().code(),
            storage_contract::ErrorCode::NameCollision
        );
        before.assert_unchanged(&fixture.engine);
        fixture.assert_writable_after_reopen();
    }
}

#[test]
fn rejected_save_retains_workspace_without_installing_replicas() {
    let mut fixture = Fixture::new();
    fixture.save("existing.txt").unwrap();
    let workspace = fixture
        .engine
        .create_workspace(&fixture.repo.main_branch_id, "admin")
        .unwrap();
    let staged = fixture
        .engine
        .stage_bytes(&workspace.workspace_id, NAMESPACE_MARKER, b"preserved body")
        .unwrap();
    let inode = fs::metadata(&staged.source_path).unwrap().ino();
    let error = fixture
        .engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id.clone(),
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "Rejected path".into(),
            request_digest: None,
        })
        .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::NameCollision);
    assert_eq!(fs::metadata(&staged.source_path).unwrap().ino(), inode);
    assert_eq!(fs::read(&staged.source_path).unwrap(), b"preserved body");
    assert_eq!(
        fixture
            .engine
            .workspace(&workspace.workspace_id)
            .unwrap()
            .files
            .len(),
        1
    );
    let mut installed = Vec::new();
    for disk in &fixture.engine.config().disks {
        let versions = disk
            .root
            .join("histories")
            .join(&fixture.repo.history_tree_id)
            .join("versions");
        for entry in fs::read_dir(versions).unwrap() {
            let directory = entry.unwrap().path();
            let version: FileVersion = read_json(&directory.join("version.json")).unwrap();
            if !fixture
                .engine
                .state()
                .versions
                .contains_key(&version.version_id)
            {
                let body = content_path(&directory, &version).unwrap();
                installed.push((
                    disk.slot_id.clone(),
                    version.version_id,
                    fs::read(body).unwrap().len(),
                    directory.join("publication-proof.json").exists(),
                ));
            }
        }
    }
    let report = fixture.engine.check().unwrap();
    assert!(
        installed.is_empty(),
        "invalid names must be rejected before copying payloads"
    );
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    fixture.assert_writable_after_reopen();
}

#[test]
fn nested_marker_names_remain_valid_for_save_rename_and_backup_prefix() {
    let mut fixture = Fixture::new();
    fixture.save("existing.txt").unwrap();
    fixture.save("safe/.zxs-namespace.json").unwrap();
    fixture
        .save("nested/.zxs-namespace.json/child.txt")
        .unwrap();
    let before = PublishedSnapshot::capture(&fixture.engine);
    fixture
        .rename("safe/.zxs-namespace.json", "safe/.ZXS-NAMESPACE.JSON")
        .unwrap();
    fixture.rename("nested", "moved").unwrap();
    fixture.backups("archive/.zxs-namespace.json").unwrap();
    let after = PublishedSnapshot::capture(&fixture.engine);
    assert_eq!(
        before
            .bodies
            .iter()
            .map(|(_, inode, bytes)| (inode, bytes))
            .collect::<Vec<_>>(),
        after
            .bodies
            .iter()
            .map(|(_, inode, bytes)| (inode, bytes))
            .collect::<Vec<_>>(),
        "namespace and backup moves preserve every payload inode and byte"
    );
    fixture.assert_writable_after_reopen();
}
