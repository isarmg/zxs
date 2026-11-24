use rustix::{
    fs::{self, CWD, Mode, OFlags, ResolveFlags},
    process::{getegid, geteuid, getgid, getuid},
};
use std::{env, fs::File, os::unix::fs::MetadataExt, path::PathBuf, process::Command};

/// All native fixtures are opt-in. Never use the working tree, a generic temporary
/// directory, an existing fixture name, an alias, or a fallback filesystem.
pub fn fixture(name: &str) -> PathBuf {
    assert_ne!(getuid().as_raw(), 0, "fixture must run as an ordinary user");
    assert_eq!(getuid(), geteuid(), "fixture must not be setuid");
    assert_eq!(getgid(), getegid(), "fixture must not be setgid");
    let status = std::fs::read_to_string("/proc/thread-self/status").unwrap();
    let cap = status
        .lines()
        .find_map(|line| line.strip_prefix("CapEff:"))
        .expect("fixture needs native procfs");
    assert_eq!(u64::from_str_radix(cap.trim(), 16).unwrap(), 0);

    let root = PathBuf::from(
        env::var_os("ZXS_CORE_NATIVE_TEST_ROOT")
            .expect("provide a fresh ext4 scratch from mktemp -d .../zxs-core-native.XXXXXXXX"),
    );
    let nonce = root
        .file_name()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_prefix("zxs-core-native."));
    assert!(nonce.is_some_and(|s| s.len() == 8 && s.bytes().all(|b| b.is_ascii_alphanumeric())));
    assert!(root.is_absolute());
    assert_eq!(root.as_os_str(), root.canonicalize().unwrap().as_os_str());

    let held: File = fs::openat2(
        CWD,
        &root,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
    )
    .unwrap()
    .into();
    let meta = held.metadata().unwrap();
    assert_eq!(meta.uid(), getuid().as_raw());
    assert_eq!(meta.mode() & 0o7777, 0o700);

    let mount = Command::new("findmnt")
        .args(["-rno", "FSTYPE", "-T"])
        .arg(&root)
        .output()
        .expect("findmnt is required to distinguish ext4");
    assert!(mount.status.success() && mount.stdout == b"ext4\n");
    assert_eq!(fs::fstatfs(&held).unwrap().f_type, 0xef53);

    fs::mkdirat(&held, name, Mode::RUSR | Mode::WUSR | Mode::XUSR)
        .expect("a new, never reused disposable fixture subdirectory is required");
    root.join(name)
}
