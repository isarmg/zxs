//! Only for future tests: create a private, exclusive child; never use pre-existing data files.
use rustix::fs::{Mode, OFlags, fstatfs, open};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};
use tempfile::{Builder, TempDir};

pub fn data_directory() -> TempDir {
    private_directory(std::env::var_os("HELPER_TEST_DATA_PARENT").map(PathBuf::from).unwrap_or_else(std::env::temp_dir))
}

#[allow(dead_code)] // some standalone test targets use only the data directory
pub fn recycle_directory() -> TempDir {
    let parent = std::env::var_os("HELPER_TEST_RECYCLE_PARENT")
        .expect("supply a dedicated private test parent on a different supported dev AND filesystem UUID");
    private_directory(PathBuf::from(parent))
}

fn private_directory(parent: PathBuf) -> TempDir {
    use linux_raw_sys::general::{BTRFS_SUPER_MAGIC, EXT4_SUPER_MAGIC, F2FS_SUPER_MAGIC, XFS_SUPER_MAGIC};
    assert!(parent.is_absolute(), "data test area must be absolute");
    let dir = open(parent.as_path(), OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                   Mode::empty()).expect("data test parent must already be a real directory");
    let kind = u32::try_from(fstatfs(&dir).unwrap().f_type).ok();
    assert!(matches!(kind, Some(EXT4_SUPER_MAGIC | XFS_SUPER_MAGIC | BTRFS_SUPER_MAGIC | F2FS_SUPER_MAGIC)),
            "test precondition not met: set HELPER_TEST_DATA_PARENT to a dedicated directory on ext2/3/4, XFS, Btrfs or F2FS; positive case did not run");
    Builder::new().prefix("helper-test-").permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(parent).expect("could not create an exclusive private test directory")
}
