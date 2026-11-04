use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};

fn private(path: &Path) {
    std::fs::create_dir(path).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}
fn request(path: &Path) -> InspectRequest {
    let listing = browse(path.to_str().unwrap()).unwrap();
    InspectRequest {
        mount_path: listing.mount_path,
        root_path: listing.path,
    }
}

#[test]
fn empty_subdirectory_is_selected_and_held_without_mutation() {
    let tmp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let child = tmp.path().join("chosen storage");
    private(&child);
    let selected = inspect(&request(&child), &[]).unwrap();
    assert_eq!(selected.root_path, child.to_str().unwrap());
    assert_ne!(selected.mount_path, selected.root_path);
    let held = revalidate(&selected, &[]).unwrap();
    assert_eq!(
        &identity(&held.into_storage_directory()).unwrap().0,
        &selected.root_identity
    );
    assert_eq!(std::fs::read_dir(&child).unwrap().count(), 0);
}

#[test]
fn malformed_and_symbolic_link_paths_never_become_candidates() {
    for path in [
        "relative",
        "/tmp/../root",
        "/tmp/./child",
        "/tmp//child",
        "/tmp/\0",
    ] {
        assert!(absolute(path).is_err(), "{path:?}");
    }
    let tmp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let child = tmp.path().join("real");
    private(&child);
    let link = tmp.path().join("link");
    symlink(&child, &link).unwrap();
    assert!(browse(link.to_str().unwrap()).is_err());
    assert!(browse(link.join("descendant").to_str().unwrap()).is_err());
    let listing = browse(tmp.path().to_str().unwrap()).unwrap();
    let row = listing
        .entries
        .iter()
        .find(|row| row.name == "link")
        .unwrap();
    assert!(!row.can_browse);
    assert_eq!(row.problem, Some(DirectoryProblem::SymbolicLink));
}

#[test]
fn root_replacement_and_changed_mount_identity_require_a_new_inspection() {
    let tmp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let root = tmp.path().join("root");
    private(&root);
    let selected = inspect(&request(&root), &[]).unwrap();
    std::fs::rename(&root, tmp.path().join("old")).unwrap();
    private(&root);
    assert!(matches!(revalidate(&selected, &[]), Err(Error::Changed)));
    let mut selected = inspect(&request(&root), &[]).unwrap();
    selected.mount_identity.mount_id += 1;
    assert!(matches!(revalidate(&selected, &[]), Err(Error::Changed)));
    let mut selected = inspect(&request(&root), &[]).unwrap();
    selected.selection_id.push('0');
    assert!(matches!(revalidate(&selected, &[]), Err(Error::Changed)));
}

#[test]
fn permissions_nonempty_and_overlap_are_real_rejections() {
    let tmp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let root = tmp.path().join("root");
    private(&root);
    let input = request(&root);
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        inspect(&input, &[]),
        Err(Error::Boundary(fs_safety::Error::UnsafePermissions(_)))
    ));
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(
        inspect(&input, &[tmp.path().into()]),
        Err(Error::Overlap(_))
    ));
    assert!(matches!(
        inspect(&input, std::slice::from_ref(&root)),
        Err(Error::Overlap(_))
    ));
    assert!(matches!(
        inspect(&input, &[root.join("future-system")]),
        Err(Error::Overlap(_))
    ));
    std::fs::write(root.join("keep"), b"original").unwrap();
    assert!(matches!(inspect(&input, &[]), Err(Error::NotEmpty)));
    assert_eq!(std::fs::read(root.join("keep")).unwrap(), b"original");
}

#[test]
fn declared_mount_must_be_the_actual_mount_and_selection_is_strict_wire() {
    let tmp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let root = tmp.path().join("root");
    private(&root);
    let mut input = request(&root);
    input.mount_path = root.to_str().unwrap().into();
    assert!(matches!(inspect(&input, &[]), Err(Error::Changed)));
    let selected = inspect(&request(&root), &[]).unwrap();
    let value = json_codec::to_value(&selected).unwrap();
    assert!(value["mount_identity"]["mount_id"].is_string());
    assert_eq!(
        json_codec::from_value::<CandidateSelection>(value.clone()).unwrap(),
        selected
    );
    let mut unknown = value.clone();
    unknown["unsafe_override"] = true.into();
    assert!(json_codec::from_value::<CandidateSelection>(unknown).is_err());
    let mut missing_scope = value.clone();
    missing_scope
        .as_object_mut()
        .unwrap()
        .remove("discovery_root");
    assert!(json_codec::from_value::<CandidateSelection>(missing_scope).is_err());
    let mut numeric = value;
    numeric["root_identity"]["inode"] = 1.into();
    assert!(json_codec::from_value::<CandidateSelection>(numeric).is_err());
}

#[test]
fn mount_parser_preserves_bind_root_and_escaped_paths() {
    let records = parse_mounts("20 1 8:1 / /mnt/disk rw,relatime - ext4 /dev/sda1 rw\n21 1 8:1 /private/sub /mnt/alias\\040disk rw - ext4 /dev/sda1 rw\n").unwrap();
    assert_eq!(records[1].root, Path::new("/private/sub"));
    assert_eq!(records[1].path, Path::new("/mnt/alias disk"));
    for input in [
        "",
        "20 1 8:1 / /mnt rw - ext4 src",
        "20 1 8:1 / /bad\\123 rw - ext4 src rw",
        "20 1 8:1 / /mnt rw - ext4 src rw\n20 1 8:1 / /dup rw - ext4 src rw",
    ] {
        assert!(parse_mounts(input).is_err(), "{input:?}");
    }
    for fs in ["proc", "sysfs", "tmpfs", "devtmpfs", "cgroup2"] {
        assert!(reject_non_storage_filesystem(fs).is_err());
    }
    for fs in ["ext4", "xfs", "btrfs", "fuseblk"] {
        assert!(reject_non_storage_filesystem(fs).is_ok());
    }
}

#[test]
fn browser_inventory_is_bounded_and_does_not_read_files() {
    let tmp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    for n in 0..MAX_ENTRIES + 1 {
        private(&tmp.path().join(format!("folder-{n}")));
    }
    let listing = browse(tmp.path().to_str().unwrap()).unwrap();
    assert_eq!(listing.entries.len(), MAX_ENTRIES);
    assert!(listing.truncated);
    assert!(listing.entries.iter().all(|entry| entry.can_browse));
}

#[test]
fn bind_mount_aliases_map_to_the_same_filesystem_subtree() {
    let mounts = parse_mounts("20 1 8:1 / /mnt/disk rw - ext4 /dev/sda1 rw\n21 1 8:1 /chosen /mnt/alias rw - ext4 /dev/sda1 rw\n").unwrap();
    let direct = mount_relative_location(&mounts[0], Path::new("/mnt/disk/chosen/sub"));
    let alias = mount_relative_location(&mounts[1], Path::new("/mnt/alias/sub"));
    assert_eq!(direct, alias);
    assert!(overlaps(&direct, Path::new("/chosen")));
    assert!(overlaps(&alias, Path::new("/chosen/sub/future-system")));
    assert!(!overlaps(&alias, Path::new("/chosen/other")));
}

#[test]
fn revalidation_rejects_new_contents_and_a_held_result_stays_bound() {
    let tmp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let root = tmp.path().join("root");
    private(&root);
    let selected = inspect(&request(&root), &[]).unwrap();
    let held = revalidate(&selected, &[]).unwrap();
    std::fs::write(root.join("new"), b"preserve me").unwrap();
    assert!(matches!(revalidate(&selected, &[]), Err(Error::NotEmpty)));
    std::fs::rename(&root, tmp.path().join("original")).unwrap();
    private(&root);
    let actual = identity(&held.into_storage_directory()).unwrap().0;
    assert_eq!(actual.device, selected.root_identity.device);
    assert_eq!(actual.inode, selected.root_identity.inode);
    assert_ne!(
        inspect(&request(&root), &[]).unwrap().root_identity,
        selected.root_identity
    );
    assert_eq!(
        std::fs::read(tmp.path().join("original/new")).unwrap(),
        b"preserve me"
    );
}

#[test]
fn configured_discovery_root_only_lists_real_internal_directories() {
    let tmp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let root = tmp.path().join("disks");
    let sibling = tmp.path().join("disks-other");
    private(&root);
    private(&sibling);
    let child = root.join("nested");
    private(&child);
    private(&child.join("selected"));
    std::fs::write(root.join("ordinary-file"), b"not a directory").unwrap();
    symlink(&sibling, root.join("link-outside")).unwrap();
    symlink(&child, root.join("link-inside")).unwrap();
    let scope = root.to_str().unwrap();
    let listing = browse_within(scope, scope).unwrap();
    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.entries[0].path, child.to_str().unwrap());
    assert!(listing.entries[0].can_browse);
    assert_eq!(
        browse_within(scope, child.to_str().unwrap())
            .unwrap()
            .entries
            .len(),
        1
    );
    for outside in [sibling.as_path(), tmp.path()] {
        assert!(matches!(
            browse_within(scope, outside.to_str().unwrap()),
            Err(Error::OutsideDiscoveryRoot)
        ));
    }
    assert!(browse_within(scope, root.join("link-outside").to_str().unwrap()).is_err());
    assert!(browse_within(scope, root.join("link-inside/selected").to_str().unwrap()).is_err());
    assert!(matches!(
        inspect_within(scope, &request(&root), &[]),
        Err(Error::OutsideDiscoveryRoot)
    ));
}

#[test]
fn discovery_root_settings_accept_existing_readable_parents_but_not_system_overlap() {
    let tmp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let root = tmp.path().join("disks");
    private(&root);
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    let scope = root.to_str().unwrap();
    validate_discovery_root(scope, &[]).unwrap();
    assert_eq!(
        std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    assert!(validate_discovery_root(root.join("missing").to_str().unwrap(), &[]).is_err());
    assert!(!root.join("missing").exists());
    assert!(matches!(
        validate_discovery_root(scope, &[tmp.path().into()]),
        Err(Error::Overlap(_))
    ));
    assert!(matches!(
        validate_discovery_root(scope, &[root.join("future-system")]),
        Err(Error::Overlap(_))
    ));
    let alias = tmp.path().join("alias");
    symlink(&root, &alias).unwrap();
    assert!(validate_discovery_root(alias.to_str().unwrap(), &[]).is_err());
}

#[test]
fn scoped_selection_binds_discovery_identity_and_current_configuration() {
    let tmp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let root = tmp.path().join("disks");
    private(&root);
    let child = root.join("storage");
    private(&child);
    let scope = root.to_str().unwrap();
    let selected = inspect_within(scope, &request(&child), &[]).unwrap();
    assert_eq!(selected.discovery_root, scope);
    assert_eq!(
        selected.discovery_identity,
        browse_within(scope, scope).unwrap().identity
    );
    let held = revalidate_within(scope, &selected, &[]).unwrap();
    assert_eq!(
        identity(&held.into_storage_directory()).unwrap().0,
        selected.root_identity
    );
    // Even a newly configured ancestor that still contains the same child is
    // not the discovery scope this review bound. Explicit reinspection is needed.
    assert!(matches!(
        revalidate_within(tmp.path().to_str().unwrap(), &selected, &[]),
        Err(Error::Changed)
    ));
    let mut changed = selected.clone();
    changed.discovery_identity.inode += 1;
    assert!(matches!(
        revalidate_within(scope, &changed, &[]),
        Err(Error::Changed)
    ));
    private(&root.join("new-entry"));
    assert!(matches!(
        revalidate_within(scope, &selected, &[]),
        Err(Error::Changed)
    ));
    assert!(std::fs::read_dir(&child).unwrap().next().is_none());
}

#[test]
fn scoped_directory_replacement_does_not_redirect_held_storage() {
    let tmp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let root = tmp.path().join("disks");
    private(&root);
    let child = root.join("storage");
    private(&child);
    let scope = root.to_str().unwrap();
    let selected = inspect_within(scope, &request(&child), &[]).unwrap();
    let held = revalidate_within(scope, &selected, &[]).unwrap();
    std::fs::rename(&root, tmp.path().join("old-disks")).unwrap();
    private(&root);
    private(&child);
    assert!(matches!(
        revalidate_within(scope, &selected, &[]),
        Err(Error::Changed)
    ));
    let actual = identity(&held.into_storage_directory()).unwrap().0;
    assert_eq!(actual.device, selected.root_identity.device);
    assert_eq!(actual.inode, selected.root_identity.inode);
    assert_ne!(
        inspect_within(scope, &request(&child), &[])
            .unwrap()
            .discovery_identity,
        selected.discovery_identity
    );
}

#[test]
fn discovery_contract_default_and_strict_settings_fields_are_shared() {
    use storage_contract::storage_directories::{
        DEFAULT_DISCOVERY_ROOT, DiscoveryRootSettings, SetDiscoveryRootRequest,
    };
    assert_eq!(DEFAULT_DISCOVERY_ROOT, "/zxs/data/disks");
    let settings = DiscoveryRootSettings {
        discovery_root: DEFAULT_DISCOVERY_ROOT.into(),
    };
    assert_eq!(
        json_codec::from_value::<DiscoveryRootSettings>(json_codec::to_value(&settings).unwrap())
            .unwrap(),
        settings
    );
    let update = json_codec::json!({"expected_discovery_root":DEFAULT_DISCOVERY_ROOT,"discovery_root":"/srv/zxs-directories"});
    assert!(json_codec::from_value::<SetDiscoveryRootRequest>(update.clone()).is_ok());
    let mut unknown = update;
    unknown["create_if_missing"] = true.into();
    assert!(json_codec::from_value::<SetDiscoveryRootRequest>(unknown).is_err());
    assert!(
        json_codec::from_value::<SetDiscoveryRootRequest>(
            json_codec::json!({"discovery_root":"/srv/zxs-directories"})
        )
        .is_err()
    );
}
