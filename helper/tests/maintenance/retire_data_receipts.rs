//! Isolated synthetic SYSTEM and R directories. No real-volume enrollment or writes.
use super::*;
use std::{
    fs::{self, DirBuilder, Permissions},
    os::unix::fs::{DirBuilderExt, PermissionsExt, symlink},
    path::PathBuf,
};
use tempfile::TempDir;

const POOL: [u8; 16] = [0x11; 16];
const CID: [u8; 16] = [0xa1; 16];
const OP: [u8; 16] = [0xb2; 16];
const DATA: [u8; 32] = [0x33; 32];
const RECYCLE: [u8; 32] = [0x44; 32];

struct Fixture {
    scratch: TempDir,
    system: File,
    recycle: File,
}

fn directory(path: &Path) {
    DirBuilder::new().mode(0o700).create(path).unwrap();
}

fn record(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, Permissions::from_mode(0o600)).unwrap();
}

impl Fixture {
    fn new() -> Self {
        let scratch = tempfile::tempdir().unwrap();
        for name in [
            "system",
            "system/container-routes",
            "system/container-moves",
            "r",
            "r/containers",
        ] {
            directory(&scratch.path().join(name));
        }
        let system = File::open(scratch.path().join("system")).unwrap();
        let recycle = File::open(scratch.path().join("r")).unwrap();
        let out = Self {
            scratch,
            system,
            recycle,
        };
        directory(&out.arrival());
        directory(&out.arrival().join("container"));
        let mut route = [0; 52];
        route[..4].copy_from_slice(b"CR1\0");
        route[4..20].copy_from_slice(&CID);
        route[20..].copy_from_slice(&DATA);
        record(
            &out.scratch
                .path()
                .join("system/container-routes")
                .join(hex(&CID)),
            &route,
        );
        out
    }

    fn arrival(&self) -> PathBuf {
        self.scratch
            .path()
            .join("r/containers")
            .join(format!("{}-{}", hex(&CID), hex(&OP)))
    }

    fn write(&self, receipt: &[u8]) {
        record(
            &self.scratch
                .path()
                .join("system/container-moves")
                .join(hex(&CID)),
            receipt,
        );
        record(&self.arrival().join("ready"), receipt);
    }

    fn stable(&self) -> [u8; STABLE_RECEIPT_BYTES] {
        let mut raw = [0; STABLE_RECEIPT_BYTES];
        raw[..4].copy_from_slice(b"RC2\0");
        raw[4..20].copy_from_slice(&CID);
        raw[20..36].copy_from_slice(&OP);
        raw[36..44].copy_from_slice(&1_u64.to_be_bytes());
        raw[44..60].copy_from_slice(&POOL);
        raw[60..92].copy_from_slice(&DATA);
        raw[92..124].copy_from_slice(&RECYCLE);
        raw[124..132].copy_from_slice(&123_u64.to_be_bytes());
        raw[132..140].copy_from_slice(&fs::metadata(self.arrival()).unwrap().ino().to_be_bytes());
        raw[140..172].fill(0xcc);
        raw
    }

    fn legacy(&self, source_device: u64) -> [u8; LEGACY_RECEIPT_BYTES] {
        let mut raw = [0; LEGACY_RECEIPT_BYTES];
        raw[..4].copy_from_slice(b"RC1\0");
        raw[4..20].copy_from_slice(&CID);
        raw[20..36].copy_from_slice(&OP);
        raw[36..44].copy_from_slice(&1_u64.to_be_bytes());
        raw[44..52].copy_from_slice(&source_device.to_be_bytes());
        raw[52..60].copy_from_slice(&123_u64.to_be_bytes());
        let arrival = fs::metadata(self.arrival()).unwrap();
        raw[60..68].copy_from_slice(&arrival.dev().to_be_bytes());
        raw[68..76].copy_from_slice(&arrival.ino().to_be_bytes());
        raw[76..108].fill(0xcc);
        raw
    }

    fn verify(&self, source_device: u64, initialized: bool) -> io::Result<()> {
        archive_routes(
            &self.system,
            &ArchiveReceipts {
                recycle: &self.recycle,
                pool: &POOL,
                data_token: &DATA,
                recycle_token: &RECYCLE,
                source_device,
                uid: self.system.metadata()?.uid(),
            },
            initialized,
        )
    }
}

#[test]
fn stable_selector_survives_device_renumbering_but_not_other_pool_or_role() {
    let tree = Fixture::new();
    let raw = tree.stable();
    tree.write(&raw);
    tree.verify(4, true).unwrap();
    tree.verify(9, true).unwrap();
    assert!(tree.verify(4, false).is_err());
    for offset in [4, 44, 60, 92] {
        let mut wrong = raw;
        wrong[offset] ^= 1;
        tree.write(&wrong);
        assert!(tree.verify(4, true).is_err(), "offset {offset}");
    }
    for range in [20..36, 36..44, 124..132, 132..140] {
        let mut wrong = raw;
        wrong[range].fill(0);
        tree.write(&wrong);
        assert!(tree.verify(4, true).is_err());
    }
    let mut wrong = raw;
    wrong[132] ^= 1;
    tree.write(&wrong);
    assert!(tree.verify(4, true).is_err());
    let mut oversized = raw.to_vec();
    oversized.push(0);
    tree.write(&oversized);
    assert!(tree.verify(4, true).is_err());
}

#[test]
fn stable_selector_requires_matching_ready_and_same_mount_directory() {
    let tree = Fixture::new();
    let raw = tree.stable();
    tree.write(&raw);
    let arrival = tree.arrival();
    let mut diverged = raw;
    diverged[171] ^= 1;
    record(&arrival.join("ready"), &diverged);
    assert!(tree.verify(0, true).is_err());
    record(&arrival.join("ready"), &raw);
    fs::rename(arrival.join("container"), arrival.join("saved")).unwrap();
    assert!(tree.verify(0, true).is_err());
    symlink("saved", arrival.join("container")).unwrap();
    assert!(tree.verify(0, true).is_err());
    fs::remove_file(arrival.join("container")).unwrap();
    fs::rename(arrival.join("saved"), arrival.join("container")).unwrap();
    tree.verify(0, true).unwrap();

    let other = tree.scratch.path().join("other");
    fs::rename(&arrival, &other).unwrap();
    symlink(other, arrival).unwrap();
    assert!(tree.verify(0, true).is_err());
}

#[test]
fn legacy_selector_has_no_token_or_device_number_fallback() {
    let tree = Fixture::new();
    let rdev = tree.recycle.metadata().unwrap().dev();
    let source = rdev ^ 1;
    let raw = tree.legacy(source);
    tree.write(&raw);
    tree.verify(source, true).unwrap();
    assert!(tree.verify(source ^ 2, true).is_err());

    let mut moved_recycle = raw;
    moved_recycle[60..68].copy_from_slice(&(rdev ^ 4).to_be_bytes());
    tree.write(&moved_recycle);
    assert!(tree.verify(source, true).is_err());

    tree.write(&raw);
    fs::remove_file(tree.arrival().join("ready")).unwrap();
    assert!(tree.verify(source, true).is_err());
    tree.write(&raw[..LEGACY_RECEIPT_BYTES - 1]);
    assert!(tree.verify(source, true).is_err());
}
