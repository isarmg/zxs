use super::*;
use std::fs;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, symlink};
use std::sync::atomic::{AtomicU64, Ordering};

static SERIAL: AtomicU64 = AtomicU64::new(0);
struct PrivateTemp(PathBuf, Entity);
impl PrivateTemp {
    fn under(parent: &Path) -> Self {
        for _ in 0..64 {
            let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("zxs-source-test-{}-{serial}",std::process::id()));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => {
                    let m = fs::metadata(&path).unwrap();
                    return Self(path, Entity { device:m.dev(), inode:m.ino() });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => panic!("create private fixture: {e}"),
            }
        }
        panic!("no fresh fixture name");
    }
    fn child(&self, name: &str) -> PathBuf {
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

#[test]
fn named_nonempty_root_is_not_adopted_or_changed() {
    let tmp = PrivateTemp::under(&std::env::temp_dir());
    let d = tmp.child("chosen");
    fs::write(d.join("original"),b"pre-existing bytes").unwrap();
    let held = HeldDir::open(&d).unwrap();
    assert!(held.require_fresh().is_err());
    assert_eq!(fs::read(d.join("original")).unwrap(),b"pre-existing bytes");
    assert_eq!(fs::read_dir(&d).unwrap().count(),1);
}

#[test]
fn symlinks_ancestor_aliases_and_replaced_names_are_not_authority() {
    let tmp = PrivateTemp::under(&std::env::temp_dir());
    let d = tmp.child("true");
    let inside = d.join("child");
    fs::DirBuilder::new().mode(0o700).create(&inside).unwrap();
    let alias = tmp.0.join("linked");
    symlink(&d,&alias).unwrap();
    assert!(HeldDir::open(&alias).is_err());
    assert!(HeldDir::open(&alias.join("child")).is_err());
    let held = HeldDir::open(&d).unwrap();
    let child = HeldDir::open(&inside).unwrap();
    assert!(child.reject_registered_ancestors(&BTreeSet::from([held.entity,child.entity])).is_err());
    fs::rename(&d,tmp.0.join("old")).unwrap();
    tmp.child("true");
    assert!(held.require_named().is_err());
}

#[test]
fn mandatory_dedicated_recycle_is_never_an_ordinary_shared_filesystem() {
    let tmp = PrivateTemp::under(&std::env::temp_dir());
    let system = tmp.child("system"); let data = tmp.child("file"); let recycle = tmp.child("recycle");
    assert!(inspect_fresh(&system,&recycle,&[]).is_err());
    assert!(inspect_fresh(&system,&data,std::slice::from_ref(&data)).is_err());
    assert!(inspect_fresh(&system,&recycle,std::slice::from_ref(&data)).is_err());
    for d in [system,data,recycle] { assert_eq!(fs::read_dir(d).unwrap().count(),0); }
}

#[test]
#[ignore = "set ZXS_DISTINCT_TEST_TMPDIR to a second, writable test filesystem with user xattrs"]
fn fresh_inspection_on_two_filesystems_returns_exact_held_identities_without_writes() {
    let first = PrivateTemp::under(&std::env::temp_dir());
    let second = PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    assert_ne!(first.1.device, second.1.device, "requires distinct test filesystems");
    let system = first.child("system"); let data = first.child("data"); let bin = second.child("recycle");
    let result = inspect_fresh(&system,&bin,std::slice::from_ref(&data)).unwrap();
    assert_eq!(result.iter().map(|v| v.role).collect::<Vec<_>>(),[Role::System,Role::Recycle,Role::File]);
    for v in result {
        let meta = fs::metadata(&v.path).unwrap();
        assert_eq!(v.entity, Entity { device:meta.dev(), inode:meta.ino() });
        assert_eq!(fs::read_dir(v.path).unwrap().count(),0);
    }
}
