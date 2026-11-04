//! Shared, test-only observation of real repair/recovery consumer syscalls.
use crate::{DiskConfig, Error, ErrorCode};
use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
pub(super) enum Point {
    FirstSync,
    SecondSync,
    Read,
    Close,
}

impl Point {
    fn name(self) -> &'static str {
        match self {
            Self::FirstSync => "first",
            Self::SecondSync => "second",
            Self::Read => "read",
            Self::Close => "close",
        }
    }
}

pub(super) struct SyscallGate(PathBuf);

impl SyscallGate {
    pub(super) fn arm(root: &Path, watched: &Path, point: Point, fail: bool) -> Self {
        assert!(
            std::env::var_os("LD_PRELOAD")
                .is_some_and(|value| value.to_string_lossy().contains("repair_io_tails")),
            "requires repair/io_tails.c compiled as repair_io_tails.so"
        );
        assert!(!fail || !matches!(point, Point::Close));
        fs::write(
            root.join(".repair-tail-watch"),
            watched.as_os_str().as_encoded_bytes(),
        )
        .unwrap();
        if fail {
            fs::write(root.join(".repair-tail-eio"), []).unwrap();
        }
        fs::write(root.join(format!(".repair-tail-arm-{}", point.name())), []).unwrap();
        Self(root.to_path_buf())
    }

    pub(super) fn skip_presence_close(&self) {
        // content_path_at first closes its regular-file presence probe. The
        // caller under review owns the next regular-file close of these bytes.
        fs::write(self.0.join(".repair-tail-skip-close"), []).unwrap();
    }

    pub(super) fn wait(&self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.0.join(".repair-tail-entered").exists() {
            assert!(
                Instant::now() < deadline,
                "consumer never reached its syscall"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    pub(super) fn release(&self) {
        fs::write(self.0.join(".repair-tail-release"), []).unwrap();
    }

    pub(super) fn synced(&self, point: Point) -> Option<PathBuf> {
        match fs::read_to_string(self.0.join(format!(".repair-tail-seen-{}", point.name()))) {
            Ok(path) => Some(PathBuf::from(path)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => panic!("cannot inspect sync order: {error}"),
        }
    }
}

impl Drop for SyscallGate {
    fn drop(&mut self) {
        // Release before scoped-thread teardown even when an assertion fails.
        let _ = fs::write(self.0.join(".repair-tail-release"), []);
    }
}

pub(super) fn descriptors(path: &Path) -> usize {
    fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter(
            |entry| match fs::read_link(entry.as_ref().unwrap().path()) {
                Ok(target) => target == path,
                Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                Err(error) => panic!("cannot inspect repair descriptor: {error}"),
            },
        )
        .count()
}

pub(super) fn competitor(disk: &DiskConfig) -> File {
    File::options()
        .read(true)
        .write(true)
        .open(disk.root.join(".writer.lock"))
        .unwrap()
}

pub(super) fn excluded(competitor: &File, expected: bool) {
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

pub(super) fn tail_error(error: Error, fail: bool) {
    if fail {
        let Error::Contract(storage_contract::Error::Io { source, .. }) = error else {
            panic!("repair fsync lost its original I/O source: {error}");
        };
        assert_eq!(source.raw_os_error(), Some(5));
    } else {
        assert_eq!(error.code(), ErrorCode::DiskUnavailable);
    }
}
