use super::*;
use storage_contract::{
    CommitRequest, Config, OperationId, RepositoryName,
    storage_directories::{DirectorySource, DiscoveredDiskState},
    storage_foreign::{ForeignDiskPreview, ForeignDiskPreviewRequest},
};

pub(super) struct Fixture {
    pub(super) _root: tempfile::TempDir,
    pub(super) backend: Arc<ManagementBackend>,
    pub(super) identity: Identity,
    pub(super) token: String,
    pub(super) source: PathBuf,
    pub(super) known: PathBuf,
}

fn save(engine: &mut storage_engine::Engine, name: &str, slot: &str, copies: u32) {
    let repository = engine
        .create_repository(
            &RepositoryName::parse(name).unwrap(),
            slot,
            copies,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "document.txt", name.as_bytes())
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap();
}

pub(super) fn fixture() -> Fixture {
    let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let discovery = root.path().join("disks");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&discovery)
        .unwrap();
    let data = root.path().join("system");
    let known = discovery.join("known");
    let engine =
        storage_engine::Engine::initialize(&data, Config::development([known.clone()]).unwrap())
            .unwrap();
    let source = discovery.join("foreign");
    let mut foreign_config = Config::development([
        root.path().join("primary-one"),
        root.path().join("primary-two"),
        source.clone(),
    ])
    .unwrap();
    foreign_config.disks[1].level = 1;
    foreign_config.disks[1].upstream_slot_id = None;
    foreign_config.disks[2].level = 2;
    foreign_config.disks[2].upstream_slot_id = Some("P01".into());
    let mut foreign =
        storage_engine::Engine::initialize(&root.path().join("foreign-system"), foreign_config)
            .unwrap();
    save(&mut foreign, "first-origin", "P01", 2);
    save(&mut foreign, "second-origin", "B01", 1);
    drop(foreign);
    let management = data.join("management");
    drop(AuthStore::initialize_management(&management, "admin", "foreign-test-password").unwrap());
    let (backend, mut auth) = open_service_with_options(
        &data,
        &management,
        ManagementOptions {
            discovery_root: discovery.to_str().unwrap().parse().unwrap(),
            ..Default::default()
        },
    )
    .unwrap();
    let token = auth.login("admin", "foreign-test-password").unwrap().token;
    let identity = auth.authenticate(&token).unwrap();
    let engine = Arc::new(EngineBackend::new(engine));
    engine.set_management_auth(auth.reader()).unwrap();
    *crate::lock(&backend.inner.auth) = Some(Arc::new(Mutex::new(auth)));
    backend.inner.set(Some(engine), StorageState::Ready);
    Fixture {
        _root: root,
        backend,
        identity,
        token,
        source,
        known,
    }
}

pub(super) fn source(f: &Fixture) -> DirectorySource {
    let disks = f.backend.discovered_disks(&f.identity).unwrap();
    let selected = disks
        .candidates
        .into_iter()
        .find(|candidate| candidate.name == "foreign")
        .unwrap();
    let DiscoveredDiskState::Zxs { source, .. } = selected.state else {
        panic!("fixture foreign source has an intact local registration seal")
    };
    source
}

pub(super) fn inspect(
    f: &Fixture,
    source: DirectorySource,
) -> Result<ForeignDiskPreview, ApiError> {
    let value = f.backend.setup_operation(
        "foreign_disk_preview",
        json_codec::to_value(ForeignDiskPreviewRequest { source }).unwrap(),
        None,
        Some(&f.identity),
    )?;
    Ok(json_codec::from_value(value).unwrap())
}

pub(super) fn bytes(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn collect(root: &Path, current: &Path, output: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(current).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                collect(root, &entry.path(), output);
            } else {
                output.insert(
                    entry.path().strip_prefix(root).unwrap().into(),
                    fs::read(entry.path()).unwrap(),
                );
            }
        }
    }
    let mut output = BTreeMap::new();
    collect(path, path, &mut output);
    output
}
