use super::*;
use platform_io::fs_safety::linux::MountPolicy;
use std::{fs, path::Path};

fn directory(parent: &Path, name: &str) -> OpenAt2Root {
    let path = parent.join(name);
    fs::create_dir(&path).unwrap();
    OpenAt2Root::open(&path, MountPolicy::SameMount).unwrap()
}

#[test]
fn two_held_fixed_roles_cannot_be_the_same_directory() {
    let temp = tempfile::tempdir().unwrap();
    let state = directory(temp.path(), "state");
    let discovery = directory(temp.path(), "discovery");
    let data = OpenAt2Root::open(temp.path(), MountPolicy::SameMount).unwrap();
    let other = OpenAt2Root::open(&temp.path().join("state"), MountPolicy::SameMount).unwrap();
    let result = DistinctRoots::from_held([&data, &state, &other, &discovery]);
    assert_eq!(
        result.err().unwrap().downcast_ref::<AliasedRoot>(), Some(&AliasedRoot::Control),
    );
}

#[test]
fn selected_roots_must_differ_from_every_control_and_each_other_not_every_device() {
    let temp = tempfile::tempdir().unwrap();
    let data = OpenAt2Root::open(temp.path(), MountPolicy::SameMount).unwrap();
    let state = directory(temp.path(), "state");
    let management = directory(temp.path(), "management");
    let discovery = directory(temp.path(), "disks");
    let first = directory(&temp.path().join("disks"), "A");
    let second = directory(&temp.path().join("disks"), "B");
    let original = temp.path().join("disks/A/preserve");
    fs::write(&original, b"existing material").unwrap();
    let mut observed = DistinctRoots::from_held([&data, &state, &management, &discovery]).unwrap();
    assert_eq!(first.identity().device, second.identity().device);
    observed.storage(first.directory()).unwrap();
    observed.storage(second.directory()).unwrap();
    for fixed in [&data, &state, &management, &discovery] {
        assert_eq!(
            observed.storage(fixed.directory()).unwrap_err().downcast_ref::<AliasedRoot>(),
            Some(&AliasedRoot::StorageControl),
        );
    }
    let alias = OpenAt2Root::open(&temp.path().join("disks/A"), MountPolicy::SameMount).unwrap();
    assert_eq!(
        observed.storage(alias.directory()).unwrap_err().downcast_ref::<AliasedRoot>(),
        Some(&AliasedRoot::Storage),
    );
    assert_eq!(fs::read(original).unwrap(), b"existing material");
}
