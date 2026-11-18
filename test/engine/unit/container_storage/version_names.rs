use crate::container_storage::{
    current_container_name, installed_version_leaf, installed_version_name,
};
use crate::State;
use crate::container_storage::tests::version;

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
fn installed_leaf_uses_its_recorded_creation_name_and_all_fifteen_identity_bytes() {
    for original in ["first.txt", "latest.bin", "文件.txt"] {
        let item = version(&State::default(), &crate::new_id("history").unwrap(),
            &crate::new_id("file").unwrap(), original, b"sample");
        let text = installed_version_name(&item).unwrap();
        assert_eq!(item.storage_id.as_str().len(), 15);
        assert_eq!(text, format!("{}-{original}", item.storage_id));
        assert_eq!(installed_version_leaf(&item).unwrap().as_os_str(), text.as_str());
    }
}
