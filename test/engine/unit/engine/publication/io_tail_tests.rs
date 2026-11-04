use super::*;
use std::{
    fs::{self, File},
    io,
    os::unix::fs::{MetadataExt, symlink},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

struct Fixture {
    temp: tempfile::TempDir,
    config: ValidatedConfig,
    resources: Arc<DiskResources>,
    state: Arc<State>,
    namespace_baselines: BTreeMap<String, crate::engine::WorkspaceNamespaceBaseline>,
    snapshot: Option<String>,
    integrity: Arc<crate::engine::provenance::MetadataIntegrity>,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("disk");
        private_dir(&path).unwrap();
        let config = ValidatedConfig::try_from(Config::development([path]).unwrap()).unwrap();
        let resources = Arc::new(DiskResources::open(&config, false).unwrap());
        Self {
            temp,
            config,
            resources,
            state: Arc::new(State::default()),
            namespace_baselines: BTreeMap::new(),
            snapshot: None,
            integrity: Arc::default(),
        }
    }

    fn context(&self) -> crate::engine::PlanningContext<'_> {
        crate::engine::PlanningContext {
            root: self.temp.path(),
            config: &self.config,
            disk_resources: &self.resources,
            state: &self.state,
            current_namespaces: None,
            workspace_namespace_baselines: &self.namespace_baselines,
            snapshot: &self.snapshot,
            next_slot: 1,
            blocked: false,
            metadata_integrity: &self.integrity,
            admission: crate::engine::planning::Admission::Detached,
            integrity_scrub_schedule: None,
            published_state_base: None,
        }
    }

    fn handle(&self) -> &DiskHandle {
        self.resources.get(&self.config.disks[0]).unwrap()
    }

    fn payload(&self, bytes: &[u8]) -> (Prepared, PathBuf) {
        let source = self.temp.path().join("external-body");
        fs::write(&source, bytes).unwrap();
        let mut state = self.state.as_ref().clone();
        state.sequence = 1;
        let mut pending = Vec::new();
        let version = self
            .context()
            .build_version(
                &mut state,
                &mut pending,
                crate::business::VersionBuild {
                    history: &new_id("history").unwrap(),
                    file_id: &new_id("file").unwrap(),
                    path: &name_policy::PortablePath::parse("body").unwrap(),
                    source: &crate::engine::ReadSource::external(source),
                    primary: "P01",
                    copies: 1,
                    source_ids: Vec::new(),
                    checkpoint: true,
                },
            )
            .unwrap();
        let directory = version_dir(
            &self.config.disks[0],
            &version.history_tree_id,
            &version.version_id,
        );
        (pending.pop().unwrap(), directory)
    }
}

fn proof_fixture(fixture: &Fixture) -> (PathBuf, PathBuf) {
    let parent = fixture.handle().root.path().join("proofs");
    fixture.handle().root.private_dir(&parent).unwrap();
    let proof = parent.join("commit-proof.json");
    fixture
        .handle()
        .root
        .write_atomic(&proof, b"corrupt local proof", false)
        .unwrap();
    (proof, parent.join("proof-quarantine"))
}

fn retained_proofs(quarantine: &Path) -> Vec<PathBuf> {
    fs::read_dir(quarantine)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .as_encoded_bytes()
                .starts_with(b"commit-proof.json-")
        })
        .collect()
}

#[test]
fn proof_repair_preserves_conflicting_inode_and_matching_retry_is_read_only() {
    let fixture = Fixture::new();
    let root = &fixture.handle().root;
    let (proof, quarantine) = proof_fixture(&fixture);
    let original = fs::metadata(&proof).unwrap().ino();
    repair_local_proof(root, &proof, &17u64).unwrap();
    let retained = retained_proofs(&quarantine);
    assert_eq!(retained.len(), 1);
    assert_eq!(fs::metadata(&retained[0]).unwrap().ino(), original);
    assert_eq!(fs::read(&retained[0]).unwrap(), b"corrupt local proof");
    assert_eq!(fs::read(&proof).unwrap(), b"17");
    let repaired = fs::metadata(&proof).unwrap().ino();
    repair_local_proof(root, &proof, &17u64).unwrap();
    assert_eq!(fs::metadata(&proof).unwrap().ino(), repaired);
    assert_eq!(retained_proofs(&quarantine), retained);
    assert_eq!(root.io_binding().unwrap().status().unwrap().ordinary, 0);
}

#[test]
fn proof_repair_retains_oversized_material_and_rejects_symlinks() {
    let fixture = Fixture::new();
    let root = &fixture.handle().root;
    let (proof, quarantine) = proof_fixture(&fixture);
    let size = MAX_METADATA as u64 + 1;
    File::options()
        .write(true)
        .open(&proof)
        .unwrap()
        .set_len(size)
        .unwrap();
    let original = fs::metadata(&proof).unwrap().ino();
    repair_local_proof(root, &proof, &19u64).unwrap();
    let retained = retained_proofs(&quarantine);
    assert_eq!(retained.len(), 1);
    let metadata = fs::metadata(&retained[0]).unwrap();
    assert_eq!((metadata.ino(), metadata.len()), (original, size));
    let link = proof.parent().unwrap().join("publication-proof.json");
    symlink(&proof, &link).unwrap();
    assert!(repair_local_proof(root, &link, &23u64).is_err());
    assert_eq!(fs::read(&proof).unwrap(), b"19");
    assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
    assert_eq!(retained_proofs(&quarantine), retained);
}

#[test]
fn payload_install_retry_preserves_formal_content_and_rejects_source_change() {
    let fixture = Fixture::new();
    let (payload, directory) = fixture.payload(b"immutable payload");
    let context = fixture.context();
    context.prepare_payload(&payload).unwrap();
    let target = crate::container_storage::content_path_at(
        &fixture.handle().root,
        &directory,
        &payload.version,
        &mut |_| Ok(()),
    )
    .unwrap();
    let inode = fs::metadata(&target).unwrap().ino();
    context.prepare_payload(&payload).unwrap();
    assert_eq!(fs::metadata(&target).unwrap().ino(), inode);
    assert_eq!(fs::read(&target).unwrap(), b"immutable payload");
    let (changed, absent) = fixture.payload(b"new payload");
    fs::write(changed.source.path(), b"wrong bytes").unwrap();
    assert_eq!(
        context.prepare_payload(&changed).unwrap_err().code(),
        ErrorCode::SourceChanged
    );
    assert!(!absent.exists());
    assert_eq!(fs::read(&target).unwrap(), b"immutable payload");
}

#[test]
fn payload_directory_collision_never_replaces_or_cleans_either_copy() {
    let fixture = Fixture::new();
    let (payload, directory) = fixture.payload(b"new payload");
    let root = &fixture.handle().root;
    root.private_dir(&directory).unwrap();
    fs::write(directory.join("existing"), b"formal payload").unwrap();
    let inode = fs::metadata(&directory).unwrap().ino();
    let error = fixture.context().prepare_payload(&payload).unwrap_err();
    let Error::Contract(storage_contract::Error::Io { source, .. }) = error else {
        panic!("rename collision lost its I/O source: {error}");
    };
    assert_eq!(source.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::metadata(&directory).unwrap().ino(), inode);
    assert_eq!(
        fs::read(directory.join("existing")).unwrap(),
        b"formal payload"
    );
    let staged = fs::read_dir(root.path().join("staging"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(staged.len(), 1);
    assert_eq!(
        fs::read(staged[0].join("content/body")).unwrap(),
        b"new payload"
    );
}

#[test]
fn stopped_publication_cannot_repair_proofs_or_install_payloads() {
    let fixture = Fixture::new();
    let (payload, directory) = fixture.payload(b"retained source");
    let (proof, quarantine) = proof_fixture(&fixture);
    let root = &fixture.handle().root;
    drop(root.io_binding().unwrap().begin_quiesce().unwrap().unwrap());
    for result in [
        fixture.context().prepare_payload(&payload),
        repair_local_proof(root, &proof, &29u64),
    ] {
        assert_eq!(result.unwrap_err().code(), ErrorCode::DiskUnavailable);
    }
    assert!(!directory.exists());
    assert!(!quarantine.exists());
    assert_eq!(fs::read(proof).unwrap(), b"corrupt local proof");
    assert_eq!(fs::read(payload.source.path()).unwrap(), b"retained source");
}

fn descriptors(path: &Path) -> usize {
    fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter(
            |entry| match fs::read_link(entry.as_ref().unwrap().path()) {
                Ok(target) => target == path,
                Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                Err(error) => panic!("cannot inspect descriptor: {error}"),
            },
        )
        .count()
}

fn fence(handle: &DiskHandle) -> (DiskWriter, File) {
    let writer = handle.root.lock_file().unwrap();
    rustix::fs::flock(
        &writer,
        rustix::fs::FlockOperation::NonBlockingLockExclusive,
    )
    .unwrap();
    let owner = DiskWriter::adopt(&handle.root, FenceOwner::new(writer, None)).unwrap();
    let competitor = File::options()
        .read(true)
        .write(true)
        .open(handle.root.path().join(".writer.lock"))
        .unwrap();
    (owner, competitor)
}

fn excluded(competitor: &File, expected: bool) {
    let result = rustix::fs::flock(
        competitor,
        rustix::fs::FlockOperation::NonBlockingLockExclusive,
    );
    if expected {
        assert_eq!(result.unwrap_err(), rustix::io::Errno::WOULDBLOCK);
    } else {
        result.unwrap();
    }
}

struct SyscallGate(PathBuf);

impl SyscallGate {
    fn watch(directory: &Path, target: &Path, missing: bool) {
        fs::write(
            directory.join(".publication-watch"),
            target.as_os_str().as_encoded_bytes(),
        )
        .unwrap();
        if missing {
            fs::write(directory.join(".publication-missing"), []).unwrap();
        }
    }

    fn arm(directory: &Path, read: bool, fail: bool) -> Self {
        assert!(
            std::env::var_os("LD_PRELOAD")
                .is_some_and(|value| value.to_string_lossy().contains("publication_io_tails")),
            "requires publication/io_tails.c compiled as publication_io_tails.so"
        );
        if fail {
            fs::write(directory.join(".publication-eio"), []).unwrap();
        }
        fs::write(
            directory.join(if read {
                ".publication-read-arm"
            } else {
                ".publication-arm"
            }),
            [],
        )
        .unwrap();
        Self(directory.to_path_buf())
    }

    fn wait(&self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.0.join(".publication-entered").exists() {
            assert!(
                Instant::now() < deadline,
                "real syscall did not reach its gate"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn release(&self) {
        fs::write(self.0.join(".publication-release"), []).unwrap();
    }
}

impl Drop for SyscallGate {
    fn drop(&mut self) {
        let _ = fs::write(self.0.join(".publication-release"), []);
    }
}

fn assert_tail_error(error: Error, fail: bool) {
    if fail {
        let Error::Contract(storage_contract::Error::Io { source, .. }) = error else {
            panic!("fsync lost its EIO source: {error}");
        };
        assert_eq!(source.raw_os_error(), Some(5));
    } else {
        assert_eq!(error.code(), ErrorCode::DiskUnavailable);
    }
}

#[test]
#[ignore = "requires publication/io_tails.c as publication_io_tails.so in LD_PRELOAD"]
fn staging_rename_cancellation_and_eio_hold_fence_and_preserve_formal_payload() {
    for fail in [false, true] {
        let fixture = Fixture::new();
        let (payload, directory) = fixture.payload(b"formal bytes survive");
        let root = &fixture.handle().root;
        let source = root.path().join("staging");
        let destination = directory.parent().unwrap();
        root.private_dir(&source).unwrap();
        root.private_dir(destination).unwrap();
        SyscallGate::watch(&source, &directory, false);
        SyscallGate::watch(destination, &directory, false);
        let (owner, competitor) = fence(fixture.handle());
        let binding = root.io_binding().unwrap();
        let context = fixture.context();
        std::thread::scope(|threads| {
            let gate = SyscallGate::arm(destination, false, fail);
            let work = threads.spawn(|| context.prepare_payload(&payload));
            gate.wait();
            assert_eq!(
                fs::read(directory.join("content/body")).unwrap(),
                b"formal bytes survive"
            );
            assert!(
                !source.join(".publication-seen").exists(),
                "destination must sync first"
            );
            drop(owner);
            drop(binding.begin_quiesce().unwrap().unwrap());
            assert_eq!(binding.status().unwrap().ordinary, 1);
            assert_eq!(descriptors(&directory.join("content/body")), 0);
            assert!(descriptors(&source) > 0);
            assert!(descriptors(destination) > 0);
            excluded(&competitor, true);
            gate.release();
            assert_tail_error(work.join().unwrap().unwrap_err(), fail);
            assert_eq!(source.join(".publication-seen").exists(), !fail);
            assert!(binding.status().unwrap().drained());
            assert_eq!(descriptors(&source), 0);
            assert_eq!(descriptors(destination), 0);
            excluded(&competitor, false);
        });
        assert_eq!(
            fs::read(directory.join("content/body")).unwrap(),
            b"formal bytes survive"
        );
        assert_eq!(
            fs::read(payload.source.path()).unwrap(),
            b"formal bytes survive"
        );
    }
}

#[test]
#[ignore = "requires publication/io_tails.c as publication_io_tails.so in LD_PRELOAD"]
fn proof_quarantine_cancellation_and_eio_keep_retained_inode_and_sync_order() {
    for fail in [false, true] {
        let fixture = Fixture::new();
        let root = &fixture.handle().root;
        let (proof, quarantine) = proof_fixture(&fixture);
        let original = fs::metadata(&proof).unwrap().ino();
        root.private_dir(&quarantine).unwrap();
        let source = proof.parent().unwrap();
        SyscallGate::watch(source, &proof, true);
        SyscallGate::watch(&quarantine, &proof, true);
        let (owner, competitor) = fence(fixture.handle());
        let binding = root.io_binding().unwrap();
        std::thread::scope(|threads| {
            let gate = SyscallGate::arm(&quarantine, false, fail);
            let work = threads.spawn(|| repair_local_proof(root, &proof, &31u64));
            gate.wait();
            let retained = retained_proofs(&quarantine);
            assert_eq!(retained.len(), 1);
            assert_eq!(fs::metadata(&retained[0]).unwrap().ino(), original);
            assert_eq!(descriptors(&retained[0]), 1);
            assert!(
                !source.join(".publication-seen").exists(),
                "quarantine must sync first"
            );
            drop(owner);
            drop(binding.begin_quiesce().unwrap().unwrap());
            assert_eq!(binding.status().unwrap().ordinary, 1);
            assert_eq!(descriptors(&retained[0]), 0);
            assert!(descriptors(source) > 0);
            assert!(descriptors(&quarantine) > 0);
            excluded(&competitor, true);
            gate.release();
            assert_tail_error(work.join().unwrap().unwrap_err(), fail);
            assert_eq!(source.join(".publication-seen").exists(), !fail);
            assert!(binding.status().unwrap().drained());
            assert_eq!(descriptors(source), 0);
            assert_eq!(descriptors(&quarantine), 0);
            excluded(&competitor, false);
        });
        assert!(!proof.exists());
        let retained = retained_proofs(&quarantine);
        assert_eq!(retained.len(), 1);
        assert_eq!(fs::read(&retained[0]).unwrap(), b"corrupt local proof");
    }
}

#[test]
#[ignore = "requires publication/io_tails.c as publication_io_tails.so in LD_PRELOAD"]
fn proof_quarantine_sync_failure_retries_without_replacing_retained_material() {
    let fixture = Fixture::new();
    let root = &fixture.handle().root;
    let (proof, quarantine) = proof_fixture(&fixture);
    let original = fs::metadata(&proof).unwrap().ino();
    root.private_dir(&quarantine).unwrap();
    SyscallGate::watch(&quarantine, &proof, true);
    std::thread::scope(|threads| {
        let gate = SyscallGate::arm(&quarantine, false, true);
        let work = threads.spawn(|| repair_local_proof(root, &proof, &41u64));
        gate.wait();
        assert!(!proof.exists());
        gate.release();
        assert_tail_error(work.join().unwrap().unwrap_err(), true);
    });
    let retained = retained_proofs(&quarantine);
    assert_eq!(retained.len(), 1);
    assert_eq!(fs::metadata(&retained[0]).unwrap().ino(), original);
    repair_local_proof(root, &proof, &41u64).unwrap();
    assert_eq!(fs::read(&proof).unwrap(), b"41");
    assert_eq!(retained_proofs(&quarantine), retained);
    assert_eq!(fs::read(&retained[0]).unwrap(), b"corrupt local proof");
    assert_eq!(root.io_binding().unwrap().status().unwrap().ordinary, 0);
}

#[test]
#[ignore = "requires publication/io_tails.c as publication_io_tails.so in LD_PRELOAD"]
fn proof_read_cancellation_closes_idle_original_but_holds_inflight_read_and_fence() {
    let fixture = Fixture::new();
    let root = &fixture.handle().root;
    let (proof, quarantine) = proof_fixture(&fixture);
    let (owner, competitor) = fence(fixture.handle());
    let binding = root.io_binding().unwrap();
    std::thread::scope(|threads| {
        let gate = SyscallGate::arm(proof.parent().unwrap(), true, false);
        let work = threads.spawn(|| repair_local_proof(root, &proof, &37u64));
        gate.wait();
        assert_eq!(descriptors(&proof), 2);
        drop(owner);
        drop(binding.begin_quiesce().unwrap().unwrap());
        assert_eq!(binding.status().unwrap().ordinary, 1);
        assert_eq!(descriptors(&proof), 1);
        excluded(&competitor, true);
        gate.release();
        assert_tail_error(work.join().unwrap().unwrap_err(), false);
        assert!(binding.status().unwrap().drained());
        assert_eq!(descriptors(&proof), 0);
        excluded(&competitor, false);
    });
    assert!(!quarantine.exists());
    assert_eq!(fs::read(&proof).unwrap(), b"corrupt local proof");
}
