use super::*;
use crate::test_support::PrivateTemp;

#[test]
fn reserved_route_survives_reopen_and_cannot_change_disks() {
    let temp = PrivateTemp::under(&std::env::temp_dir());
    let system = HeldDir::trusted_storage(&temp.0).unwrap();
    let _owner = system.opaque_owner(c"container-owner").unwrap();
    let routes = Routes::open(&system).unwrap();
    let id = [3; 16];
    let token = [4; 32];
    let stage = routes.directory.path.join(format!(".{}.incoming", hex(id)));
    super::super::write_file(&stage, b"interrupted").unwrap();
    assert_eq!(routes.get(id).unwrap(), None);
    assert!(routes.ids().unwrap().is_empty());
    routes.reserve(id, token).unwrap();
    assert!(!stage.exists());
    routes.reserve(id, token).unwrap();
    assert!(routes.reserve(id, [5; 32]).is_err());
    drop(routes);

    let reopened = Routes::open(&system).unwrap();
    assert_eq!(reopened.get(id).unwrap(), Some(token));
    assert_eq!(reopened.ids().unwrap(), [id]);
}

#[test]
fn selected_route_binds_both_the_container_name_and_disk_token() {
    let temp = PrivateTemp::under(&std::env::temp_dir());
    let system = HeldDir::trusted_storage(&temp.0).unwrap();
    let _owner = system.opaque_owner(c"container-owner").unwrap();
    let routes = Routes::open(&system).unwrap();
    let id = [3; 16];
    let other = encode([2; 16], [4; 32]).unwrap();
    super::super::write_file(&routes.directory.path.join(hex(id)), &other).unwrap();
    assert!(routes.get(id).is_err());
    assert!(routes.reserve(id, [4; 32]).is_err());
    assert!(encode(id, [0; 32]).is_err());
}

// An unconfirmed sync is retried before readers can use the committed token.
#[test]
fn route_read_requalifies_an_uncertain_namespace() {
    let temp = PrivateTemp::under(&std::env::temp_dir());
    let system = HeldDir::trusted_storage(&temp.0).unwrap();
    let _owner = system.opaque_owner(c"container-owner").unwrap();
    let routes = Routes::open(&system).unwrap();
    let id = [3; 16];
    let token = [4; 32];
    routes.reserve(id, token).unwrap();
    *routes.unsure.lock().unwrap() = true;
    assert_eq!(routes.get(id).unwrap(), Some(token));
    assert!(!*routes.unsure.lock().unwrap());
}
