use super::{Destination, NEW_WORKSPACE_DIRECTORY};
use crate::{invalid, native_test_support, session::Principal, workspace::Workspace};
use std::{fs, os::unix::fs::PermissionsExt as _, sync::Arc};

#[test]
#[ignore = "requires newly allocated unprivileged ext4 fixture scratch"]
fn canceled_occupied_and_replaced_parents_never_get_a_workspace() {
    let fixture = native_test_support::fixture("workspace-create-denials");
    let parent = fixture.join("parent");
    fs::create_dir(&parent).unwrap();
    let destination = Destination::select(&parent).unwrap();
    assert!(destination.still_vacant().is_ok());
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o775)).unwrap();
    assert!(Destination::select(&parent).is_err());
    assert!(
        destination
            .create(|| panic!("parent no longer owner-writable only"))
            .is_err()
    );
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o555)).unwrap();
    assert!(Destination::select(&parent).is_err());
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
    let child = parent.join(NEW_WORKSPACE_DIRECTORY);

    assert!(
        destination
            .create(|| Err(invalid("canceled after catalog")))
            .is_err()
    );
    assert!(!child.exists());
    let hidden = child.join(".unrelated");
    assert!(
        destination
            .create(|| {
                // A competing name appears after local permission but before the
                // syscall; exclusive mkdir still cannot adopt or replace it.
                fs::create_dir(&child)?;
                fs::write(&hidden, b"preserve this file")?;
                Ok(())
            })
            .is_err()
    );
    assert!(Destination::select(&parent).is_err());
    assert!(
        destination
            .create(|| panic!("occupied before create"))
            .is_err()
    );
    assert_eq!(fs::read(hidden).unwrap(), b"preserve this file");

    let moved = fixture.join("moved");
    fs::rename(&parent, &moved).unwrap();
    fs::create_dir(&parent).unwrap();
    assert!(
        destination
            .create(|| panic!("not the original held parent"))
            .is_err()
    );
    assert!(!child.exists());
    assert!(
        !moved
            .join(NEW_WORKSPACE_DIRECTORY)
            .join("workspace.sqlite")
            .exists()
    );
}

#[test]
#[ignore = "requires newly allocated unprivileged ext4 fixture scratch"]
fn one_new_private_child_uses_the_existing_enrollment_and_open_semantics() {
    let parent = native_test_support::fixture("workspace-create-same-init");
    let unrelated = parent.join("unrelated");
    fs::write(&unrelated, b"ordinary file in parent").unwrap();
    let destination = Destination::select(&parent).unwrap();
    let root = destination.create(|| Ok(())).unwrap();
    assert_eq!(root.path(), parent.join(NEW_WORKSPACE_DIRECTORY));
    assert_eq!(
        fs::metadata(root.path()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let principal: Principal = serde_json::from_str(
        r#"{"origin":"https://no-contact.invalid:443","account":"fixture","trust":"synthetic"}"#,
    )
    .unwrap();
    let owner = Arc::new(());
    let workspace = Workspace::initialize_held(root, &principal, Arc::clone(&owner)).unwrap();
    assert!(Arc::ptr_eq(&workspace.read_session_owner, &owner));
    assert_eq!(workspace.principal, principal);
    assert_eq!(fs::read(&unrelated).unwrap(), b"ordinary file in parent");
    let path = workspace.root.path().to_owned();
    drop(workspace);
    assert!(Workspace::open(&path, &principal).is_ok());
    assert!(Destination::select(&parent).is_err());
    assert!(Destination::select(&path).is_err()); // No child inside an existing workspace.
}

#[test]
#[ignore = "requires newly allocated unprivileged ext4 fixture scratch"]
fn newly_minted_database_must_not_be_substituted() {
    let parent = native_test_support::fixture("workspace-create-db-substitution");
    let destination = Destination::select(&parent).unwrap();
    let root = destination.create(|| Ok(())).unwrap();
    let created = root.new_private("workspace.sqlite").unwrap();
    let name = root.path().join("workspace.sqlite");
    let held_away = parent.join("old-synthetic-db");
    fs::rename(&name, &held_away).unwrap();
    fs::write(&name, b"").unwrap();
    fs::set_permissions(&name, fs::Permissions::from_mode(0o600)).unwrap();
    let fixture: Principal = serde_json::from_str(
        r#"{"origin":"https://no-contact.invalid:443","account":"fixture","trust":"synthetic"}"#,
    )
    .unwrap();
    assert!(Workspace::enroll_created_db(root, &created, &fixture, Arc::new(())).is_err());
    assert_eq!(fs::metadata(&name).unwrap().len(), 0);
    assert_eq!(fs::metadata(&held_away).unwrap().len(), 0);
}
