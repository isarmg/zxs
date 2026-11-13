use super::*;
use std::{fs, os::unix::fs::{MetadataExt, PermissionsExt}};

fn root() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    temp
}

#[test]
fn bound_system_preserves_control_and_reopens_exact_original_child() {
    let temp = root();
    let control = temp.path().join("setup-intent.json");
    fs::write(&control, b"existing protected control").unwrap();
    let original = fs::metadata(&control).unwrap().ino();
    let operation = OperationId::generate().unwrap();
    let (binding, system) = StorageSystemBinding::create(
        &PrivateDirectory::open_existing(temp.path()).unwrap(), operation,
    ).unwrap();
    assert_eq!(binding.operation, operation);
    assert_eq!(binding.open(temp.path()).unwrap().path(), system.path());
    assert_eq!(fs::metadata(&control).unwrap().ino(), original);
    assert_eq!(fs::read(&control).unwrap(), b"existing protected control");
}

#[test]
fn preexisting_unbound_empty_system_is_never_adopted() {
    let temp = root();
    let child = temp.path().join(layout::SYSTEM.as_path());
    fs::create_dir(&child).unwrap();
    fs::set_permissions(&child, fs::Permissions::from_mode(0o700)).unwrap();
    let inode = fs::metadata(&child).unwrap().ino();
    assert!(StorageSystemBinding::create(
        &PrivateDirectory::open_existing(temp.path()).unwrap(), OperationId::generate().unwrap(),
    ).is_err());
    assert_eq!(fs::metadata(&child).unwrap().ino(), inode);
    assert_eq!(fs::read_dir(child).unwrap().count(), 0);
}

#[test]
fn replaced_bound_system_is_refused_without_writing_either_directory() {
    let temp = root();
    let (binding, system) =
        StorageSystemBinding::create(
            &PrivateDirectory::open_existing(temp.path()).unwrap(), OperationId::generate().unwrap(),
        ).unwrap();
    let retained = temp.path().join("retained-system");
    fs::rename(system.path(), &retained).unwrap();
    fs::create_dir(system.path()).unwrap();
    fs::set_permissions(system.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let replacement = fs::metadata(system.path()).unwrap().ino();
    assert!(binding.open(temp.path()).is_err());
    assert_eq!(fs::metadata(system.path()).unwrap().ino(), replacement);
    assert_eq!(fs::read_dir(system.path()).unwrap().count(), 0);
    assert_eq!(fs::read_dir(retained).unwrap().count(), 0);
}

#[test]
fn replaced_control_parent_cannot_reuse_a_bound_child() {
    let temp = root();
    let control = temp.path().join("control");
    fs::create_dir(&control).unwrap();
    fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).unwrap();
    let (binding, system) =
        StorageSystemBinding::create(
            &PrivateDirectory::open_existing(&control).unwrap(), OperationId::generate().unwrap(),
        ).unwrap();
    let retained = temp.path().join("retained-control");
    fs::rename(&control, &retained).unwrap();
    fs::create_dir(&control).unwrap();
    fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).unwrap();
    // Move the original child back: matching child identity cannot replace its parent proof.
    fs::rename(retained.join(layout::SYSTEM.as_path()), system.path()).unwrap();
    let parent_inode = fs::metadata(&control).unwrap().ino();
    let child_inode = fs::metadata(system.path()).unwrap().ino();
    assert!(binding.open(&control).is_err());
    assert_eq!(fs::metadata(&control).unwrap().ino(), parent_inode);
    assert_eq!(fs::metadata(system.path()).unwrap().ino(), child_inode);
    assert_eq!(fs::read_dir(system.path()).unwrap().count(), 0);
    assert_eq!(fs::read_dir(retained).unwrap().count(), 0);
}

#[test]
fn captured_control_replacement_is_rejected_before_creating_any_system_child() {
    let temp = root();
    let control = temp.path().join("control");
    fs::create_dir(&control).unwrap();
    fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).unwrap();
    let held = PrivateDirectory::open_existing(&control).unwrap();
    let retained = temp.path().join("retained-control");
    fs::rename(&control, &retained).unwrap();
    fs::create_dir(&control).unwrap();
    fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).unwrap();
    let before = fs::metadata(&control).unwrap().ino();
    assert!(StorageSystemBinding::create(&held, OperationId::generate().unwrap()).is_err());
    assert_eq!(fs::metadata(&control).unwrap().ino(), before);
    assert_eq!(fs::read_dir(control).unwrap().count(), 0);
    assert_eq!(fs::read_dir(retained).unwrap().count(), 0);
}
