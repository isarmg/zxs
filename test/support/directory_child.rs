use std::{
    fs::File,
    io::{Read, Write},
    os::{fd::OwnedFd, unix::fs::MetadataExt, unix::net::UnixStream},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub(super) struct DirectoryChild {
    process: Child,
    channel: UnixStream,
}

impl DirectoryChild {
    pub(super) fn start(directory: File, shared_lock: bool, child_test: &str) -> Self {
        let metadata = directory.metadata().unwrap();
        let (parent, child) = UnixStream::pair().unwrap();
        parent
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        parent
            .set_write_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let process = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                child_test,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("ZXS_DIRECTORY_CHILD_DEVICE", metadata.dev().to_string())
            .env("ZXS_DIRECTORY_CHILD_INODE", metadata.ino().to_string())
            .env("ZXS_DIRECTORY_CHILD_SHARED", shared_lock.to_string())
            .stdin(Stdio::from(directory))
            .stdout(Stdio::null())
            .stderr(Stdio::from(OwnedFd::from(child)))
            .spawn()
            .unwrap();
        let mut result = Self {
            process,
            channel: parent,
        };
        let mut ready = [0_u8; 1];
        result.channel.read_exact(&mut ready).unwrap();
        assert_eq!(ready, *b"r");
        result
    }

    pub(super) fn release(&mut self) {
        self.channel.write_all(b"x").unwrap();
        let mut retained = [0_u8; 1];
        self.channel.read_exact(&mut retained).unwrap();
        assert_eq!(retained, *b"h");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.process.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "directory descriptor child failed: {status}"
                );
                return;
            }
            assert!(
                Instant::now() < deadline,
                "directory descriptor child did not exit"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

pub(super) fn hold_directory() {
    let directory = File::from(rustix::io::fcntl_dupfd_cloexec(std::io::stdin(), 3).unwrap());
    let mut channel =
        UnixStream::from(rustix::io::fcntl_dupfd_cloexec(std::io::stderr(), 3).unwrap());
    channel
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    channel
        .set_write_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let device: u64 = std::env::var("ZXS_DIRECTORY_CHILD_DEVICE")
        .unwrap()
        .parse()
        .unwrap();
    let inode: u64 = std::env::var("ZXS_DIRECTORY_CHILD_INODE")
        .unwrap()
        .parse()
        .unwrap();
    let shared: bool = std::env::var("ZXS_DIRECTORY_CHILD_SHARED")
        .unwrap()
        .parse()
        .unwrap();
    let metadata = directory.metadata().unwrap();
    assert!(metadata.is_dir());
    assert_eq!((metadata.dev(), metadata.ino()), (device, inode));
    match directory.try_lock() {
        // The parent still holds its lock: only the same open-file description can succeed.
        Ok(()) => assert!(shared),
        Err(std::fs::TryLockError::WouldBlock) => assert!(!shared),
        Err(std::fs::TryLockError::Error(error)) => panic!("child lock probe failed: {error}"),
    }
    channel.write_all(b"r").unwrap();
    let mut release = [0_u8; 1];
    channel.read_exact(&mut release).unwrap();
    assert_eq!(release, *b"x");
    let metadata = directory.metadata().unwrap();
    assert_eq!((metadata.dev(), metadata.ino()), (device, inode));
    channel.write_all(b"h").unwrap();
}

impl Drop for DirectoryChild {
    fn drop(&mut self) {
        if !matches!(self.process.try_wait(), Ok(Some(_))) {
            let _ = self.process.kill();
        }
        let _ = self.process.wait();
    }
}
