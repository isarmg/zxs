use super::*;
use std::{cell::Cell, fs, os::unix::fs::symlink};

struct Fixture {
    _temporary: tempfile::TempDir,
    host: BundleDestination,
    parent: OpenAt2Root,
    path: std::path::PathBuf,
    owner: (u32, u32),
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let host = temporary.path().join("host");
        let path = temporary.path().join("disks");
        for path in [&host, &path] {
            fs::create_dir(path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self {
            host: BundleDestination::open(&host).unwrap(),
            parent: OpenAt2Root::open(&path, MountPolicy::SameMount).unwrap(),
            owner: (
                rustix::process::geteuid().as_raw(),
                rustix::process::getegid().as_raw(),
            ),
            path,
            _temporary: temporary,
        }
    }
    fn publish(&self, target: &Target) {
        publish(
            &self.host,
            &self.parent,
            self.owner,
            target,
            "digest",
            || Ok(()),
        )
        .unwrap();
    }
    fn observations(&self, targets: &[Target]) -> Vec<Entry> {
        observe(&self.host, &self.parent, self.owner, targets, "digest").unwrap()
    }
}

#[test]
fn empty_root_batch_preserves_existing_directory_and_retries_original_inodes() {
    let fixture = Fixture::new();
    let existing = fixture.path.join("existing");
    fs::create_dir(&existing).unwrap();
    fs::set_permissions(&existing, fs::Permissions::from_mode(0o700)).unwrap();
    let original = fs::metadata(&existing).unwrap().ino();
    let roots = [
        "/zxs/data/disks/new".into(),
        "/zxs/data/disks/existing".into(),
    ];
    let targets = capture(&fixture.parent, fixture.owner, &roots).unwrap();
    let before = fixture.observations(&targets);
    assert!(before[0].is_absent());
    assert!(before[1].completed().is_some());
    for target in &targets {
        fixture.publish(target);
    }
    let complete = fixture.observations(&targets);
    let bindings: Vec<_> = complete
        .iter()
        .map(|entry| entry.completed().unwrap())
        .collect();
    for target in &targets {
        fixture.publish(target);
    }
    let repeated: Vec<_> = fixture
        .observations(&targets)
        .iter()
        .map(|entry| entry.completed().unwrap())
        .collect();
    assert!(bindings == repeated);
    assert_eq!(fs::metadata(existing).unwrap().ino(), original);
    for leaf in ["new", "existing"] {
        let path = fixture.path.join(leaf);
        let metadata = fs::metadata(&path).unwrap();
        assert_eq!((metadata.uid(), metadata.gid()), fixture.owner);
        assert_eq!(metadata.mode() & 0o7777, 0o700);
        assert_eq!(fs::read_dir(path).unwrap().count(), 0);
    }
}

#[test]
fn interrupted_root_publication_resumes_the_bound_pending_inode() {
    let fixture = Fixture::new();
    let targets = capture(
        &fixture.parent,
        fixture.owner,
        &["/zxs/data/disks/new".into()],
    )
    .unwrap();
    let count = Cell::new(0);
    let result = publish(
        &fixture.host,
        &fixture.parent,
        fixture.owner,
        &targets[0],
        "digest",
        || {
            count.set(count.get() + 1);
            if count.get() == 3 {
                Err("interrupted after durable inode binding".into())
            } else {
                Ok(())
            }
        },
    );
    assert!(result.is_err());
    let (pending, _) = names(&targets[0], "digest").unwrap();
    let pending_inode = fs::metadata(fixture.path.join(pending.as_path()))
        .unwrap()
        .ino();
    assert!(!fixture.path.join("new").exists());
    assert!(matches!(
        fixture.observations(&targets)[0].location,
        Location::Pending(_)
    ));
    fixture.publish(&targets[0]);
    assert_eq!(
        fs::metadata(fixture.path.join("new")).unwrap().ino(),
        pending_inode
    );
    assert!(!fixture.path.join(pending.as_path()).exists());
}

#[test]
fn published_root_replacement_and_nonempty_original_are_preserved() {
    let fixture = Fixture::new();
    let targets = capture(
        &fixture.parent,
        fixture.owner,
        &["/zxs/data/disks/new".into()],
    )
    .unwrap();
    fixture.publish(&targets[0]);
    let selected = fixture.path.join("new");
    fs::write(selected.join("keep"), b"existing data").unwrap();
    assert!(
        observe(
            &fixture.host,
            &fixture.parent,
            fixture.owner,
            &targets,
            "digest"
        )
        .is_err()
    );
    assert_eq!(fs::read(selected.join("keep")).unwrap(), b"existing data");
    fs::rename(&selected, fixture.path.join("original")).unwrap();
    fs::create_dir(&selected).unwrap();
    fs::set_permissions(&selected, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        publish(
            &fixture.host,
            &fixture.parent,
            fixture.owner,
            &targets[0],
            "digest",
            || Ok(())
        )
        .is_err()
    );
    assert_eq!(
        fs::read(fixture.path.join("original/keep")).unwrap(),
        b"existing data"
    );
}

#[test]
fn first_root_capture_refuses_symlinks_and_unselected_material() {
    let fixture = Fixture::new();
    let roots = ["/zxs/data/disks/new".into()];
    fs::write(fixture.path.join("keep"), b"unselected").unwrap();
    assert!(capture(&fixture.parent, fixture.owner, &roots).is_err());
    assert_eq!(fs::read(fixture.path.join("keep")).unwrap(), b"unselected");
    fs::remove_file(fixture.path.join("keep")).unwrap();
    symlink(fixture.path.parent().unwrap(), fixture.path.join("new")).unwrap();
    assert!(capture(&fixture.parent, fixture.owner, &roots).is_err());
    assert!(
        fs::symlink_metadata(fixture.path.join("new"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}
