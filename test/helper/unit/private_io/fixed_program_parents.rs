use super::*;
use crate::private_io::ProgramDestination;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

#[test]
fn the_same_bin_and_program_cannot_be_carried_through_a_replaced_system_ancestor() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("usr");
    fs::create_dir_all(base.join("local/bin")).unwrap();
    let paths = FixedProgramParents::capture(
        &base,
        &["local", "bin"],
        rustix::process::geteuid().as_raw(),
    )
    .unwrap();
    let path = base.join("local/bin/zxs");
    let destination = ProgramDestination::fixed(&path, paths).unwrap();
    let (created, held) = destination
        .install_captured(b"program", None, false)
        .unwrap();
    assert!(created);
    fs::rename(base.join("local"), base.join("original-local")).unwrap();
    fs::create_dir(base.join("local")).unwrap();
    fs::rename(base.join("original-local/bin"), base.join("local/bin")).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"program");
    // In particular, the current named leaf and its immediate parent are still
    // the exact original inodes. The intermediate "local" is not.
    assert!(destination.require_held(&held).is_err());
    assert!(destination.matching_file(b"program").is_err());
    fs::rename(base.join("local/bin"), base.join("original-local/bin")).unwrap();
    fs::rename(base.join("local"), base.join("other-local")).unwrap();
    fs::rename(base.join("original-local"), base.join("local")).unwrap();
    destination.require_held(&held).unwrap();
}

#[test]
fn unsafe_or_linked_ancestor_denies_install_before_creating_a_leaf() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("usr");
    fs::create_dir_all(base.join("local/bin")).unwrap();
    let owner = rustix::process::geteuid().as_raw();
    let captured = FixedProgramParents::capture(&base, &["local", "bin"], owner).unwrap();
    let path = base.join("local/bin/zxs");
    let destination = ProgramDestination::fixed(&path, captured).unwrap();
    fs::set_permissions(base.join("local"), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(
        destination
            .install_captured(b"program", None, false)
            .is_err()
    );
    assert!(!path.exists());
    assert!(FixedProgramParents::capture(&base, &["local", "bin"], owner).is_err());
    fs::set_permissions(base.join("local"), fs::Permissions::from_mode(0o700)).unwrap();
    symlink(base.join("local"), base.join("alias")).unwrap();
    assert!(FixedProgramParents::capture(&base, &["alias", "bin"], owner).is_err());
}
