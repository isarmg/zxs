//! Exact real record/head publication retries. The plan is never a replacement
//! for independently authenticated observations and ordinary reload stays strict.
use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

struct Fixture {
    _temporary: tempfile::TempDir,
    path: PathBuf,
    config: Config,
    store: BootstrapStore,
    previous: LoadedLocator,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("control");
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let config =
            Config::development((0..3).map(|n| temporary.path().join(format!("root-{n}"))))
                .unwrap();
        for disk in &config.disks {
            fs::create_dir(&disk.root).unwrap();
            fs::set_permissions(&disk.root, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut store = BootstrapStore::hold(File::open(&path).unwrap(), path.clone()).unwrap();
        let roots = config
            .disks
            .iter()
            .take(2)
            .map(|disk| observe(&config, disk))
            .collect();
        let previous = store.initialize(&config.pool_id, roots).unwrap();
        Self {
            _temporary: temporary,
            path,
            config,
            store,
            previous,
        }
    }
    fn roots(&self) -> Vec<RootObservation> {
        vec![observe(&self.config, &self.config.disks[2])]
    }
    fn plan(&self) -> PreparedRootExtension {
        self.store
            .prepare_root_extension(&self.previous, self.roots())
            .unwrap()
            .unwrap()
    }
}
fn binding(config: &Config, disk: &crate::DiskConfig) -> DiskRegistrationIdentity {
    DiskRegistrationIdentity {
        pool_id: config.pool_id.clone(),
        disk_id: disk.disk_id.clone(),
        slot_id: disk.slot_id.clone(),
        binding_generation: disk.binding_generation,
    }
}
fn observe(config: &Config, disk: &crate::DiskConfig) -> RootObservation {
    RootObservation::capture(
        &binding(config, disk),
        &disk.root,
        &File::open(&disk.root).unwrap(),
        disk.metadata_member,
    )
    .unwrap()
}
fn operation() -> OperationId {
    OperationId::generate().unwrap()
}
fn digest() -> IntentDigest {
    IntentDigest::parse(&"a".repeat(64)).unwrap()
}

#[test]
fn prepared_extension_replays_every_real_record_and_head_barrier() {
    for phase in [
        WriteFault::BeforeRecord,
        WriteFault::AfterRecord,
        WriteFault::BeforeHead,
        WriteFault::AfterHead,
    ] {
        let mut fixture = Fixture::new();
        let plan = fixture.plan();
        let serialized = storage_contract::canonical_json(&plan).unwrap();
        let plan: PreparedRootExtension = storage_contract::decode_strict(&serialized).unwrap();
        fixture.store.inject(phase);
        let roots = fixture.roots();
        assert!(
            fixture.store.resume_root_extension(&plan, roots).is_err(),
            "{phase:?}"
        );
        if matches!(phase, WriteFault::AfterRecord | WriteFault::BeforeHead) {
            assert!(
                fixture.store.reload(&fixture.config.pool_id).is_err(),
                "ordinary reload cannot elect an orphan record"
            );
        }
        let expected = fixture.store.root_extension_locator(&plan).unwrap();
        assert_eq!(expected.record.roots.len(), 3);
        let roots = fixture.roots();
        let actual = fixture.store.resume_root_extension(&plan, roots).unwrap();
        assert_eq!(actual.head, expected.head);
        assert_eq!(
            fixture.store.reload(&fixture.config.pool_id).unwrap().head,
            actual.head
        );
    }
}

#[test]
fn prepared_extension_rejects_changed_observations_and_never_clears_old_stops() {
    let mut fixture = Fixture::new();
    let target = binding(&fixture.config, &fixture.config.disks[0]);
    fixture.previous = fixture
        .store
        .append_stop(&fixture.previous, &target, operation(), digest())
        .unwrap();
    let plan = fixture.plan();
    let mut roots = fixture.roots();
    roots[0].record.inode += 1;
    assert!(fixture.store.resume_root_extension(&plan, roots).is_err());
    let roots = fixture.roots();
    let next = fixture.store.resume_root_extension(&plan, roots).unwrap();
    assert_eq!(next.record.stops, fixture.previous.record.stops);
    assert!(next.excludes_binding(&target));
}

#[test]
fn prepared_extension_preserves_a_later_authenticated_stop_without_rollback() {
    let mut fixture = Fixture::new();
    let plan = fixture.plan();
    let roots = fixture.roots();
    let next = fixture.store.resume_root_extension(&plan, roots).unwrap();
    let target = binding(&fixture.config, &fixture.config.disks[0]);
    let stopped = fixture
        .store
        .append_stop(&next, &target, operation(), digest())
        .unwrap();
    assert!(
        fixture.store.root_extension_locator(&plan).is_err(),
        "cold probe selection cannot ignore a newer head"
    );
    let roots = fixture.roots();
    let repeated = fixture.store.resume_root_extension(&plan, roots).unwrap();
    assert_eq!(repeated.head, stopped.head);
    assert!(repeated.excludes_binding(&target));
}

#[test]
fn prepared_extension_never_adopts_foreign_ledger_entries() {
    let mut fixture = Fixture::new();
    let plan = fixture.plan();
    let foreign = fixture.path.join("unrelated.json");
    fs::write(&foreign, b"unrelated").unwrap();
    fs::set_permissions(&foreign, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(fixture.store.root_extension_locator(&plan).is_err());
    let roots = fixture.roots();
    assert!(fixture.store.resume_root_extension(&plan, roots).is_err());
    assert_eq!(fs::read(foreign).unwrap(), b"unrelated");
}

#[test]
fn ordinary_root_extension_cannot_authorize_a_terminal_record() {
    let mut fixture = Fixture::new();
    let target = binding(&fixture.config, &fixture.config.disks[0]);
    let stopped = fixture
        .store
        .append_stop(&fixture.previous, &target, operation(), digest())
        .unwrap();
    let final_target = binding(&fixture.config, &fixture.config.disks[1]);
    let record = Record::terminal_stop(&stopped, &final_target, operation(), digest()).unwrap();
    let terminal = fixture.store.publish(Some(&stopped.head), record).unwrap();
    assert!(
        fixture
            .store
            .prepare_root_extension(&terminal, fixture.roots())
            .is_err()
    );
    assert!(terminal.select(&fixture.config).is_err());
}
