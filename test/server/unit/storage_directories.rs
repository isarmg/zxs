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
    assert_eq!(
        records[1].root.directory().unwrap(),
        Path::new("/private/sub")
    );
    assert_eq!(records[1].path, Path::new("/mnt/alias disk"));
    for input in [
        "",
        "20 1 8:1 / /mnt rw - ext4 src",
        "20 1 8:1 / /bad\\123 rw - ext4 src rw",
        "20 1 8:1 / /mnt rw - ext4 src rw\n20 1 8:1 / /dup rw - ext4 src rw",
    ] {
        assert!(parse_mounts(input).is_err(), "{input:?}");
    }
    for fs in ["proc", "sysfs", "tmpfs", "devtmpfs", "cgroup2", "nsfs"] {
        assert!(reject_non_storage_filesystem(fs).is_err());
    }
    for fs in ["ext4", "xfs", "btrfs", "fuseblk"] {
        assert!(reject_non_storage_filesystem(fs).is_ok());
    }
}

#[test]
fn namespace_mount_roots_remain_in_inventory_without_becoming_storage_paths() {
    let text = "20 1 8:1 / / rw - ext4 /dev/sda1 rw\n21 20 0:4 net:[4026531840] /run/docker/netns/default rw - nsfs nsfs rw\n22 20 8:1 /private/sub /mnt/alias rw - ext4 /dev/sda1 rw\n";
    let mounts = parse_mounts(text).unwrap();
    assert_eq!(mounts.len(), 3);
    assert_eq!(
        mounts[1].root,
        MountRoot::Namespace("net:[4026531840]".into())
    );
    assert_eq!(mounts[1].id, 21);
    assert_eq!((mounts[1].major, mounts[1].minor), (0, 4));
    let ordinary = mapped_location(Path::new("/srv/storage"), &mounts).unwrap();
    assert_eq!(ordinary.device, (8, 1));
    assert_eq!(ordinary.path, Path::new("/srv/storage"));
    assert_eq!(
        mount_relative_location(&mounts[2], Path::new("/mnt/alias/child")).unwrap(),
        Path::new("/private/sub/child")
    );
    let namespace = Path::new("/run/docker/netns/default");
    assert!(matches!(
        mapped_location(namespace, &mounts),
        Err(Error::UnsupportedFilesystem(filesystem)) if filesystem == "nsfs"
    ));
    assert!(matches!(
        mount_relative_location(&mounts[1], namespace),
        Err(Error::UnsupportedFilesystem(filesystem)) if filesystem == "nsfs"
    ));
    assert!(matches!(
        StoppedRoots::new(&[namespace.to_owned()], &mounts),
        Err(Error::UnsupportedFilesystem(filesystem)) if filesystem == "nsfs"
    ));
    assert_ne!(
        mounts,
        parse_mounts(&text.replace("net:[4026531840]", "net:[4026531841]")).unwrap(),
        "namespace identity changes must remain visible to inventory revalidation"
    );
}

#[test]
fn namespace_mount_parsing_keeps_format_identity_and_count_limits() {
    use std::fmt::Write as _;

    for root in [
        "/",
        ":[1]",
        "Net:[1]",
        "net/:[1]",
        "net:[]",
        "net:[0]",
        "net:[-1]",
        "net:[+1]",
        "net:[1]/child",
        "net:[18446744073709551616]",
        "../net:[1]",
        "net\\123:[1]",
    ] {
        let text = format!("21 20 0:4 {root} /run/docker/netns/default rw - nsfs nsfs rw\n");
        assert!(
            matches!(parse_mounts(&text), Err(Error::InvalidMounts)),
            "{root}"
        );
    }
    assert_eq!(
        MountRoot::parse("user:[4026531837]", "nsfs").unwrap(),
        MountRoot::Namespace("user:[4026531837]".into())
    );
    assert!(matches!(
        MountRoot::parse(&format!("{}:[1]", "n".repeat(4096)), "nsfs"),
        Err(Error::InvalidMounts)
    ));
    for text in [
        "21 20 8:1 net:[1] /mnt rw - ext4 /dev/sda1 rw\n",
        "21 20 0:4 net:[1] relative rw - nsfs nsfs rw\n",
        "21 20 0:4 net:[1] /run/../netns/default rw - nsfs nsfs rw\n",
        "21 20 0:4 net:[1] /run/netns/default rw - nsfs nsfs\n",
        "21 20 8:1 / / rw - ext4 /dev/sda1 rw\n21 20 0:4 net:[1] /run/netns/default rw - nsfs nsfs rw\n",
    ] {
        assert!(
            matches!(parse_mounts(text), Err(Error::InvalidMounts)),
            "{text}"
        );
    }
    let mut text = String::new();
    for id in 1..=MAX_MOUNTS {
        writeln!(text, "{id} 0 0:4 net:[1] /run/netns/{id} rw - nsfs nsfs rw").unwrap();
    }
    assert_eq!(parse_mounts(&text).unwrap().len(), MAX_MOUNTS);
    writeln!(
        text,
        "{} 0 8:1 / /mnt rw - ext4 /dev/sda1 rw",
        MAX_MOUNTS + 1
    )
    .unwrap();
    assert!(matches!(parse_mounts(&text), Err(Error::InvalidMounts)));
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
    let direct = mount_relative_location(&mounts[0], Path::new("/mnt/disk/chosen/sub")).unwrap();
    let alias = mount_relative_location(&mounts[1], Path::new("/mnt/alias/sub")).unwrap();
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

#[test]
fn stopped_paths_are_excluded_before_directory_io_but_same_filesystem_siblings_work() {
    let tmp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let root = tmp.path().join("directories");
    private(&root);
    let stopped = root.join("stopped");
    // A stopped root need not exist or be a directory to be excluded.
    std::fs::write(&stopped, b"must not be inspected as a directory").unwrap();
    let fresh = root.join("fresh");
    private(&fresh);
    let listing = browse_fresh_within(
        root.to_str().unwrap(),
        root.to_str().unwrap(),
        std::slice::from_ref(&stopped),
    )
    .unwrap();
    assert!(
        !listing
            .entries
            .iter()
            .any(|entry| entry.path == stopped.to_str().unwrap())
    );
    assert!(
        listing
            .entries
            .iter()
            .any(|entry| entry.path == fresh.to_str().unwrap())
    );
    assert!(matches!(
        browse_fresh_within(
            root.to_str().unwrap(),
            stopped.to_str().unwrap(),
            std::slice::from_ref(&stopped)
        ),
        Err(Error::Overlap(_))
    ));
    let selected = inspect_fresh_within(
        root.to_str().unwrap(),
        &request(&fresh),
        &[],
        std::slice::from_ref(&stopped),
    )
    .unwrap();
    assert!(
        revalidate_fresh_within(
            root.to_str().unwrap(),
            &selected,
            &[],
            std::slice::from_ref(&stopped)
        )
        .is_ok()
    );
    assert_eq!(
        std::fs::read(stopped).unwrap(),
        b"must not be inspected as a directory"
    );
}

#[test]
fn historical_mount_alias_filter_is_path_specific_not_a_whole_filesystem_ban() {
    let inventory = parse_mounts(
        "1 0 8:1 / / rw - ext4 /dev/root rw\n2 1 8:1 /data/old /alias rw - ext4 /dev/root rw\n",
    )
    .unwrap();
    let paths = vec![PathBuf::from("/data/old")];
    let guard = StoppedRoots::new(&paths, &inventory).unwrap();
    assert!(matches!(
        guard.check(Path::new("/alias/child"), true),
        Err(Error::Overlap(_))
    ));
    assert!(matches!(
        guard.check(Path::new("/data"), true),
        Err(Error::Overlap(_))
    ));
    assert!(guard.check(Path::new("/data"), false).is_ok());
    assert!(guard.check(Path::new("/data/fresh"), true).is_ok());
}

#[cfg(feature = "core")]
#[test]
fn pending_fresh_resume_reopens_held_scope_despite_enrollment_ctime_change() {
    let tmp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let root = tmp.path().join("discovery");
    private(&root);
    let fresh = root.join("fresh");
    private(&fresh);
    let selected =
        inspect_fresh_within(root.to_str().unwrap(), &request(&fresh), &[], &[]).unwrap();
    std::fs::write(fresh.join("preparation"), b"retained control preparation").unwrap();
    let held = reopen_fresh_addition(&selected, &[]).unwrap();
    assert_eq!(
        identity(&held).unwrap().0.inode,
        selected.root_identity.inode
    );
    drop(held);
    std::fs::rename(&root, tmp.path().join("old-discovery")).unwrap();
    private(&root);
    private(&fresh);
    assert!(reopen_fresh_addition(&selected, &[]).is_err());
}
