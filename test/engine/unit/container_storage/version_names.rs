use crate::container_storage::{
    current_container_name, installed_version_leaf, installed_version_name,
};
use crate::State;
use name_policy::Name;
use storage_contract::VersionStorageId;

#[test]
fn current_name_uses_current_location_and_rejects_an_independent_mirror() {
    let mut state = State::default();
    let file = "file_00000000000000000000000000000001";
    assert!(current_container_name(&state, file).is_err());
    state.file_names.insert(file.into(), "obsolete.txt".into());
    assert!(current_container_name(&state, file).is_err());
    state.file_locations.insert(file.into(), "folder/current.txt".into());
    assert!(current_container_name(&state, file).is_err());
    state.file_names.insert(file.into(), "current.txt".into());
    assert_eq!(current_container_name(&state, file).unwrap().as_str(), "current.txt");
    state.file_locations.insert(file.into(), "other/latest.txt".into());
    state.file_names.insert(file.into(), "latest.txt".into());
    assert_eq!(current_container_name(&state, file).unwrap().as_str(), "latest.txt");
}

#[test]
fn installed_leaf_changes_only_the_name_and_keeps_all_fifteen_identity_bytes() {
    let id: VersionStorageId = "0123456789abcde".parse().unwrap();
    for name in ["first.txt", "latest.bin", "文件.txt"] {
        let name = Name::parse(name).unwrap();
        let text = installed_version_name(id, &name).unwrap();
        assert_eq!(text, format!("0123456789abcde-{name}"));
        assert_eq!(installed_version_leaf(id, &name).unwrap().as_os_str(), text.as_str());
    }
}
