//! Actual Genesis/namespace source and held-root tests, not a BODY-copy success claim.
use super::*;
use crate::{Engine, ErrorCode, OperationId, Repository, ValidatedConfig};
use std::{collections::BTreeMap, fs, os::unix::fs::MetadataExt, os::unix::fs::PermissionsExt};
use std::path::{Path, PathBuf};

struct Fixture {
    _temporary: tempfile::TempDir,
    system: PathBuf,
    roots: [PathBuf; 3],
    engine: Engine,
    repository: Repository,
    ids: [DiskId; 3],
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let system = temporary.path().join("system");
        let roots = ["mount-one", "mount-two", "mount-three"]
            .map(|path| temporary.path().join(path));
        for path in std::iter::once(&system).chain(roots.iter()) {
            fs::create_dir(path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let named = ["Alpha", "Beta", "Gamma"].into_iter().zip(roots.iter()).map(|(name, path)| {
            (name_policy::Name::parse(name).unwrap(), path.clone())
        });
        let config = Config::development(named).unwrap();
        let ids = std::array::from_fn(|index| config.disks[index].disk_id.parse().unwrap());
        let mut engine = Engine::initialize_held(
            &system,
            ValidatedConfig::try_from(config).unwrap(),
            roots.iter().map(|root| fs::File::open(root).unwrap()).collect(),
        ).unwrap();
        let repository = engine.create_disk_root("P01", "admin", OperationId::generate().unwrap())
            .unwrap();
        Self { _temporary: temporary, system, roots, engine, repository, ids }
    }

    fn namespace(&self) -> (RepositoryId, BranchId) {
        (self.repository.repository_id.parse().unwrap(),
            self.repository.main_branch_id.parse().unwrap())
    }

    fn media(&self) -> Vec<BTreeMap<PathBuf, (u64, u64, Vec<u8>)>> {
        fn visit(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, (u64, u64, Vec<u8>)>) {
            let metadata = fs::symlink_metadata(path).unwrap();
            assert!(!metadata.file_type().is_symlink());
            let bytes = if metadata.is_file() { fs::read(path).unwrap() } else { Vec::new() };
            out.insert(path.strip_prefix(root).unwrap().to_owned(),
                (metadata.dev(), metadata.ino(), bytes));
            if metadata.is_dir() {
                for entry in fs::read_dir(path).unwrap() {
                    visit(root, &entry.unwrap().path(), out);
                }
            }
        }
        std::iter::once(&self.system).chain(self.roots.iter()).map(|root| {
            let mut result = BTreeMap::new();
            visit(root, root, &mut result);
            result
        }).collect()
    }
}

fn component(name: &str, id: DiskId) -> String {
    format!("{name}--{}", &id.to_string()["disk_".len()..])
}

#[test]
fn actual_genesis_backup_source_keeps_direct_uuid_and_held_common_roots() {
    let fixture = Fixture::new();
    let before = fixture.media();
    let context = fixture.engine.planning_context();
    let (repository, branch) = fixture.namespace();
    let b = context.prepare_backup_source_path(repository, branch, fixture.ids[1]).unwrap();
    assert_eq!(b.target_registration().disk_id, fixture.ids[1].to_string());
    assert_eq!(b.direct_source_registration().disk_id, fixture.ids[0].to_string());
    assert_eq!(b.destination().components().collect::<Vec<_>>(),
        [component("Alpha", fixture.ids[0])]);
    assert_eq!(b.origin().relative_prefix(), None);
    b.require_context(&context).unwrap();
    b.verify_new_component_budget().unwrap();

    let c = context.prepare_backup_source_path(repository, branch, fixture.ids[2]).unwrap();
    assert_eq!(c.direct_source_registration().disk_id, fixture.ids[1].to_string());
    assert_eq!(c.destination().components().collect::<Vec<_>>(),
        [component("Beta", fixture.ids[1]), component("Alpha", fixture.ids[0])]);
    assert_eq!(c.origin().components().collect::<Vec<_>>(),
        [component("Alpha", fixture.ids[0])]);
    c.require_context(&context).unwrap();
    c.verify_new_component_budget().unwrap();
    for (plan, target) in [(&b, 1usize), (&c, 2usize)] {
        let target_root = plan.target_common().unwrap();
        let direct_root = plan.direct_source_common().unwrap();
        let target_metadata = fs::metadata(fixture.roots[target].join("common-data")).unwrap();
        let direct_metadata = fs::metadata(fixture.roots[target - 1].join("common-data")).unwrap();
        let actual = target_root.directory_identity(target_root.path()).unwrap();
        assert_eq!((actual.device, actual.inode), (target_metadata.dev(), target_metadata.ino()));
        let actual = direct_root.directory_identity(direct_root.path()).unwrap();
        assert_eq!((actual.device, actual.inode), (direct_metadata.dev(), direct_metadata.ino()));
        assert_eq!(plan.target_binding(),
            &fixture.engine.config.storage_roots[&fixture.ids[target]]);
    }
    let cold = read_backup_source_path(
        &fixture.engine.state, &fixture.engine.config, &fixture.engine.disk_resources,
        repository, branch, fixture.ids[2],
    ).unwrap();
    assert_eq!(cold.destination().relative_prefix(), c.destination().relative_prefix());
    assert_eq!(fixture.media(), before);
}

#[test]
fn actual_backup_source_rejects_a_same_value_rebound_source_and_nonbackup_target() {
    let fixture = Fixture::new();
    let before = fixture.media();
    let context = fixture.engine.planning_context();
    let (repository, branch) = fixture.namespace();
    let plan = context.prepare_backup_source_path(repository, branch, fixture.ids[1]).unwrap();
    let state = (*fixture.engine.state).clone();
    assert!(matches!(plan.require_source(&state, &fixture.engine.config,
        &fixture.engine.disk_resources), Err(Error::FreshLayout(Failure::NamespaceSource))));
    let config = (*fixture.engine.config).clone();
    assert!(matches!(plan.require_source(&fixture.engine.state, &config,
        &fixture.engine.disk_resources), Err(Error::FreshLayout(Failure::NamespaceSource))));
    plan.require_context(&context).unwrap();
    assert!(matches!(context.prepare_backup_source_path(repository, branch, fixture.ids[0]),
        Err(Error::FreshLayout(Failure::NamespaceSource))));
    assert_eq!(fixture.media(), before);
}

#[test]
fn prepared_backup_source_does_not_adopt_a_replacement_common_directory() {
    let fixture = Fixture::new();
    let context = fixture.engine.planning_context();
    let (repository, branch) = fixture.namespace();
    let plan = context.prepare_backup_source_path(repository, branch, fixture.ids[1]).unwrap();
    let common = fixture.roots[1].join("common-data");
    let parked = fixture.roots[1].join("original-common");
    fs::rename(&common, &parked).unwrap();
    fs::create_dir(&common).unwrap();
    fs::set_permissions(&common, fs::Permissions::from_mode(0o700)).unwrap();
    let before = fixture.media();
    assert_eq!(plan.target_common().err().unwrap().code(), ErrorCode::DiskIdentityMismatch);
    assert_eq!(fixture.media(), before);
    fs::remove_dir(common).unwrap();
    fs::rename(parked, fixture.roots[1].join("common-data")).unwrap();
    plan.require_context(&context).unwrap();
}
