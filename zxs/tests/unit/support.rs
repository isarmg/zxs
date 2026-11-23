use crate::Entity;
use std::{
    fs, io,
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static SERIAL: AtomicU64 = AtomicU64::new(0);
pub(crate) struct PrivateTemp(pub(crate) PathBuf, pub(crate) Entity);
impl PrivateTemp {
    pub(crate) fn under(parent: &Path) -> Self {
        for _ in 0..64 {
            let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("zxs-source-test-{}-{serial}", std::process::id()));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => {
                    let m = fs::metadata(&path).unwrap();
                    return Self(
                        path,
                        Entity {
                            device: m.dev(),
                            inode: m.ino(),
                        },
                    );
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => panic!("create private fixture: {e}"),
            }
        }
        panic!("no fresh fixture name");
    }
    pub(crate) fn child(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        path
    }
}
impl Drop for PrivateTemp {
    fn drop(&mut self) {
        if let Ok(m) = fs::symlink_metadata(&self.0) {
            if m.is_dir() && m.dev() == self.1.device && m.ino() == self.1.inode {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
    }
}
