use super::*;
use crate::repair_io_tail_support::{
    Point, SyscallGate, competitor, descriptors, excluded, tail_error,
};
use std::{os::unix::fs::MetadataExt, path::Path, sync::Arc};

fn fixture() -> (tempfile::TempDir, Engine, Candidate) {
    let (temp, system, config, _, operation) = pending();
    let engine = Engine::open_for_recovery(&system).unwrap();
    let member = &config.disks[0];
    let root = &engine.disk_resources.get(member).unwrap().root;
    let scan = crate::journal::scan_at(root, member).unwrap();
    let candidate: Candidate = crate::journal::load_object_at(
        root,
        member,
        "candidate",
        &scan.records.last().unwrap().0.candidate_hash,
    )
    .unwrap();
    assert_eq!(candidate.operation_id, operation);
    (temp, engine, candidate)
}

fn paths(engine: &Engine, candidate: &Candidate) -> (DiskConfig, PathBuf, PathBuf) {
    let evidence = &candidate.versions[0];
    let disk = engine
        .disk(&evidence.placement.route_slot_ids[0])
        .unwrap()
        .clone();
    let directory = version_dir(
        &disk,
        &evidence.version.history_tree_id,
        &evidence.version.version_id,
    );
    let content = crate::container_storage::content_path_at(
        &engine.disk_resources.get(&disk).unwrap().root,
        &directory,
        &evidence.version,
        &mut |_| Ok(()),
    )
    .unwrap();
    (disk, directory, content)
}

fn bundle(disk: &DiskConfig, version: &FileVersion) -> PathBuf {
    let bundles = fs::read_dir(disk.root.join("quarantine"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&format!("{}-repair-", version.version_id))
        })
        .collect::<Vec<_>>();
    assert_eq!(bundles.len(), 1);
    bundles.into_iter().next().unwrap()
}

fn journals(engine: &Engine) -> Vec<(PathBuf, Vec<u8>)> {
    engine
        .members()
        .iter()
        .map(|disk| {
            let path = crate::journal::log_path(disk);
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect()
}

#[test]
fn stopped_repair_preserves_candidate_payload_metadata_and_journals() {
    let (_temp, engine, candidate) = fixture();
    let (disk, directory, content) = paths(&engine, &candidate);
    let bytes = fs::read(&content).unwrap();
    let metadata = fs::read(directory.join("version.json")).unwrap();
    let checksums = fs::read(directory.join("checksums.bin")).unwrap();
    let frozen = canonical_json(&candidate).unwrap();
    let logs = journals(&engine);
    let handle = engine.disk_resources.get(&disk).unwrap();
    drop(
        handle
            .root
            .io_binding()
            .unwrap()
            .begin_quiesce()
            .unwrap()
            .unwrap(),
    );
    assert!(engine.repair_pending_payloads(&candidate).is_err());
    assert_eq!(canonical_json(&candidate).unwrap(), frozen);
    assert_eq!(fs::read(&content).unwrap(), bytes);
    assert_eq!(fs::read(directory.join("version.json")).unwrap(), metadata);
    assert_eq!(
        fs::read(directory.join("checksums.bin")).unwrap(),
        checksums
    );
    assert_eq!(
        fs::read_dir(disk.root.join("quarantine")).unwrap().count(),
        0
    );
    for (path, bytes) in logs {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn repair_keeps_quarantined_inodes_and_rebuilds_the_frozen_checksum_manifest() {
    let (_temp, engine, candidate) = fixture();
    let (disk, directory, content) = paths(&engine, &candidate);
    let version = &candidate.versions[0].version;
    let frozen = canonical_json(&candidate).unwrap();
    let metadata = fs::read(directory.join("version.json")).unwrap();
    let checksums = fs::read(directory.join("checksums.bin")).unwrap();
    let payload_inode = fs::metadata(&content).unwrap().ino();
    let metadata_inode = fs::metadata(&directory).unwrap().ino();
    let logs = journals(&engine);
    fs::remove_file(directory.join("checksums.bin")).unwrap();
    engine.repair_pending_payloads(&candidate).unwrap();
    let bundle = bundle(&disk, version);
    let retained = bundle.join("content").join(&version.creation_name);
    assert_eq!(fs::metadata(&retained).unwrap().ino(), payload_inode);
    assert_eq!(fs::read(retained).unwrap(), b"verified original bytes");
    assert_eq!(
        fs::metadata(bundle.join("metadata")).unwrap().ino(),
        metadata_inode
    );
    assert_eq!(
        fs::read(bundle.join("metadata/version.json")).unwrap(),
        metadata
    );
    assert_eq!(
        fs::read(directory.join("checksums.bin")).unwrap(),
        checksums
    );
    assert_ne!(fs::metadata(&content).unwrap().ino(), payload_inode);
    engine
        .disk_resources
        .get(&disk)
        .unwrap()
        .verify_payload(&directory, version)
        .unwrap();
    assert_eq!(canonical_json(&candidate).unwrap(), frozen);
    for (path, bytes) in logs {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[derive(Clone, Copy, Debug)]
enum Quarantine {
    Payload,
    Metadata,
}

impl Quarantine {
    fn watched<'a>(self, content: &'a Path, directory: &'a Path) -> &'a Path {
        match self {
            Self::Payload => content,
            Self::Metadata => directory,
        }
    }

    fn destination(self, bundle: &Path) -> PathBuf {
        match self {
            Self::Payload => bundle.join("content"),
            Self::Metadata => bundle.to_path_buf(),
        }
    }
}

#[test]
#[ignore = "requires repair/io_tails.c as repair_io_tails.so in LD_PRELOAD"]
fn quarantine_sync_tails_keep_source_first_order_bytes_and_writer_fence() {
    for kind in [Quarantine::Payload, Quarantine::Metadata] {
        for point in [Point::FirstSync, Point::SecondSync] {
            for fail in [false, true] {
                quarantine_tail(kind, point, fail);
            }
        }
    }
}

fn quarantine_tail(kind: Quarantine, point: Point, fail: bool) {
    let (_temp, engine, candidate) = fixture();
    let (disk, directory, content) = paths(&engine, &candidate);
    let version = candidate.versions[0].version.clone();
    let frozen = canonical_json(&candidate).unwrap();
    let logs = journals(&engine);
    let metadata = fs::read(directory.join("version.json")).unwrap();
    let inode = fs::metadata(&content).unwrap().ino();
    let metadata_inode = fs::metadata(&directory).unwrap().ino();
    fs::remove_file(directory.join("checksums.bin")).unwrap();
    let owner = Arc::clone(engine.disk_resources.get(&disk).unwrap());
    let binding = owner.root.io_binding().unwrap();
    let competitor = competitor(&disk);
    let watched = kind.watched(&content, &directory);
    let source = watched.parent().unwrap();
    std::thread::scope(|threads| {
        let gate = SyscallGate::arm(&disk.root, watched, point, fail);
        let work = threads.spawn(move || {
            let result = engine.repair_pending_payloads(&candidate);
            (engine, candidate, result)
        });
        gate.wait();
        let bundle = bundle(&disk, &version);
        let destination = kind.destination(&bundle);
        let retained = bundle.join("content").join(&version.creation_name);
        assert!(!watched.exists());
        assert_eq!(fs::metadata(&retained).unwrap().ino(), inode);
        assert_eq!(fs::read(&retained).unwrap(), b"verified original bytes");
        assert_eq!(gate.synced(Point::FirstSync).as_deref(), Some(source));
        assert_eq!(
            gate.synced(Point::SecondSync),
            matches!(point, Point::SecondSync).then(|| destination.clone())
        );
        drop(binding.begin_quiesce().unwrap().unwrap());
        assert_eq!(binding.status().unwrap().ordinary, 1);
        assert!(!binding.status().unwrap().drained());
        assert!(descriptors(source) > 0);
        assert!(descriptors(&destination) > 0);
        assert!(descriptors(&disk.root) > 0);
        excluded(&competitor, true);
        assert!(owner.root.open_read_regular(&retained).is_err());
        gate.release();
        let (engine, candidate, result) = work.join().unwrap();
        tail_error(result.unwrap_err(), fail);
        assert_eq!(
            gate.synced(Point::SecondSync),
            (!fail || matches!(point, Point::SecondSync)).then_some(destination.clone())
        );
        assert!(binding.status().unwrap().drained());
        assert_eq!(descriptors(source), 0);
        assert_eq!(descriptors(&destination), 0);
        assert_eq!(descriptors(&disk.root), 0);
        excluded(&competitor, false);
        assert_eq!(canonical_json(&candidate).unwrap(), frozen);
        assert!(
            !engine
                .state
                .operations
                .contains_key(&candidate.operation_id)
        );
        let retained_metadata = match kind {
            Quarantine::Payload => directory.clone(),
            Quarantine::Metadata => bundle.join("metadata"),
        };
        assert_eq!(
            fs::metadata(&retained_metadata).unwrap().ino(),
            metadata_inode
        );
        assert_eq!(
            fs::read(retained_metadata.join("version.json")).unwrap(),
            metadata
        );
        assert_eq!(fs::read(retained).unwrap(), b"verified original bytes");
        for (path, bytes) in logs {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
    });
}

#[test]
#[ignore = "requires repair/io_tails.c as repair_io_tails.so in LD_PRELOAD"]
fn source_discovery_read_and_close_stay_owned_until_real_syscall_returns() {
    for point in [Point::Read, Point::Close] {
        let (_temp, engine, candidate) = fixture();
        let (disk, _, content) = paths(&engine, &candidate);
        let owner = Arc::clone(engine.disk_resources.get(&disk).unwrap());
        let binding = owner.root.io_binding().unwrap();
        let competitor = competitor(&disk);
        let logs = journals(&engine);
        std::thread::scope(|threads| {
            let gate = SyscallGate::arm(&disk.root, &content, point, false);
            if matches!(point, Point::Close) {
                gate.skip_presence_close();
            }
            let work = threads.spawn(move || {
                let evidence = &candidate.versions[0];
                engine.pending_payload_source(&evidence.version, &evidence.placement)
            });
            gate.wait();
            drop(binding.begin_quiesce().unwrap().unwrap());
            assert_eq!(binding.status().unwrap().ordinary, 1);
            assert!(descriptors(&content) > 0);
            excluded(&competitor, true);
            gate.release();
            // A close after successful verification may return an inert source;
            // it must never reopen the revoked disk or retain a live descriptor.
            if let Ok(Some(source)) = work.join().unwrap() {
                assert!(source.open().is_err());
            }
            assert!(binding.status().unwrap().drained());
            assert_eq!(descriptors(&content), 0);
            assert_eq!(descriptors(&disk.root), 0);
            excluded(&competitor, false);
        });
        assert_eq!(fs::read(&content).unwrap(), b"verified original bytes");
        for (path, bytes) in logs {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
    }
}

#[test]
#[ignore = "requires repair/io_tails.c as repair_io_tails.so in LD_PRELOAD"]
fn quarantine_sync_eio_retries_without_replacing_retained_bytes() {
    for kind in [Quarantine::Payload, Quarantine::Metadata] {
        for point in [Point::FirstSync, Point::SecondSync] {
            let (_temp, engine, candidate) = fixture();
            let (disk, directory, content) = paths(&engine, &candidate);
            let version = candidate.versions[0].version.clone();
            let logs = journals(&engine);
            let inode = fs::metadata(&content).unwrap().ino();
            fs::remove_file(directory.join("checksums.bin")).unwrap();
            std::thread::scope(|threads| {
                let gate =
                    SyscallGate::arm(&disk.root, kind.watched(&content, &directory), point, true);
                let work = threads.spawn(move || {
                    let result = engine.repair_pending_payloads(&candidate);
                    (engine, candidate, result)
                });
                gate.wait();
                gate.release();
                let (engine, candidate, result) = work.join().unwrap();
                tail_error(result.unwrap_err(), true);
                let retained = bundle(&disk, &version)
                    .join("content")
                    .join(&version.creation_name);
                engine.repair_pending_payloads(&candidate).unwrap();
                assert_eq!(fs::metadata(&retained).unwrap().ino(), inode);
                assert_eq!(fs::read(&retained).unwrap(), b"verified original bytes");
                for member in &engine.config.disks {
                    engine
                        .disk_resources
                        .get(member)
                        .unwrap()
                        .verify_payload(
                            &version_dir(member, &version.history_tree_id, &version.version_id),
                            &version,
                        )
                        .unwrap();
                }
                for (path, bytes) in logs {
                    assert_eq!(fs::read(path).unwrap(), bytes);
                }
            });
        }
    }
}

#[test]
#[ignore = "requires repair/io_tails.c as repair_io_tails.so in LD_PRELOAD"]
fn metadata_quarantine_collision_keeps_both_original_and_destination_bytes() {
    let (_temp, engine, candidate) = fixture();
    let (disk, directory, content) = paths(&engine, &candidate);
    let version = candidate.versions[0].version.clone();
    let metadata = fs::read(directory.join("version.json")).unwrap();
    let original_inode = fs::metadata(&directory).unwrap().ino();
    let payload_inode = fs::metadata(&content).unwrap().ino();
    let logs = journals(&engine);
    fs::remove_file(directory.join("checksums.bin")).unwrap();
    std::thread::scope(|threads| {
        // Hold the content rename's final barrier, before repair can attempt
        // its metadata rename. No production hook or raw descriptor is used.
        let gate = SyscallGate::arm(&disk.root, &content, Point::SecondSync, false);
        let work = threads.spawn(move || engine.repair_pending_payloads(&candidate));
        gate.wait();
        let bundle = bundle(&disk, &version);
        let collision = bundle.join("metadata");
        private_dir(&collision).unwrap();
        fs::write(collision.join("unrelated"), b"existing inspection evidence").unwrap();
        let collision_inode = fs::metadata(&collision).unwrap().ino();
        gate.release();
        let error = work.join().unwrap().unwrap_err();
        let Error::Contract(storage_contract::Error::Io { source, .. }) = error else {
            panic!("quarantine collision lost its I/O source: {error}");
        };
        assert_eq!(source.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::metadata(&directory).unwrap().ino(), original_inode);
        assert_eq!(fs::read(directory.join("version.json")).unwrap(), metadata);
        assert_eq!(fs::metadata(&collision).unwrap().ino(), collision_inode);
        assert_eq!(
            fs::read(collision.join("unrelated")).unwrap(),
            b"existing inspection evidence"
        );
        let retained = bundle.join("content").join(&version.creation_name);
        assert_eq!(fs::metadata(&retained).unwrap().ino(), payload_inode);
        assert_eq!(fs::read(retained).unwrap(), b"verified original bytes");
        for (path, bytes) in logs {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
    });
}
