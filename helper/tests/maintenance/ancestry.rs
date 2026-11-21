//! Static controls and future separately provisioned native evidence; no mount operations.
use super::*;
use crate::{path_rules::Kind, test_area::subtree_bind_directory};
use std::fs;

#[test]
fn stateless_discovery_uses_only_escaped_whole_local_mount_names_and_is_bounded(){
    let raw=b"41 41 8:1 / / rw - ext4 /dev/root rw\n\
        42 41 8:2 / /mnt/one\\040space\\134disk\\012line\\011tab rw - ext4 /dev/local rw\n\
        43 41 8:2 /subtree /mnt/bind rw - ext4 /dev/local rw\n\
        44 41 0:3 / /mnt/net rw - nfs4 vendor:/root rw\n\
        45 41 0:4 / /mnt/auto rw - autofs systemd-1 rw\n\
        46 41 0:5 / /mnt/virtual rw - tmpfs tmpfs rw\n\
        47 41 8:3 / /srv/other rw - xfs /dev/second rw\n\
        48 41 8:4 / /mnt/noncanonical\\141 rw - ext4 /dev/third rw\n\
        49 44 8:5 / /mnt/net/child rw - ext4 /dev/nested rw\n\
        50 41 8:6 / /mnt/stack rw - ext4 /dev/lower rw\n\
        51 50 0:10 / /mnt/stack rw - autofs top rw\n";
    let got=Mounts::discover(raw).unwrap();
    assert_eq!(got.len(),3); // stacked lower is a possible raw candidate, never opened
    assert_eq!(got[0],DiscoveryMount{path:"/mnt/one space\\disk\nline\ttab".into(),id:42,
        device:makedev(8,2),magic:EXT4_SUPER_MAGIC});
    assert_eq!(got[1].id,50);assert_eq!(got[2].id,47);
    let named=Mounts::discovery_table(raw,false).unwrap();
    assert!(named.roots.is_empty()); // explicit selection does not collect targets
    assert_eq!(named.named(Path::new("/"),None).unwrap(),Some(41));
    assert_eq!(named.named(&got[0].path,Some(41)).unwrap(),Some(42));
    assert!(named.named(&got[0].path,Some(47)).is_err());
    for p in ["/mnt/net","/mnt/net/child","/mnt/auto","/mnt/virtual","/mnt/stack"]{
        assert!(named.named(Path::new(p),Some(41)).is_err());
    }
    assert_eq!(named.named(Path::new("/mnt/ordinary-subdir"),Some(41)).unwrap(),None);
    assert!(Mounts::discover(&raw[..raw.len()-1]).is_err());
    let mut oversized=String::from("999 999 8:1 / / rw - ext4 /dev/root rw\n");
    use std::fmt::Write;
    for i in 1..=DISCOVERY_ROOTS+1{
        writeln!(oversized,"{i} 999 8:2 / /mnt/r{i} rw - ext4 /dev/local rw").unwrap();
    }
    assert!(Mounts::discover(oversized.as_bytes()).is_err());
}

#[test]
fn actual_filesystem_root_is_field_four_not_the_visible_mountpoint_or_source() {
    let table = Mounts::parse(b"41 1 8:2 / /home\\040area rw shared:9 - ext4 /dev/test rw\n42 41 8:2 /hidden / rw - ext4 /dev/test rw\n43 41 8:3 / /disk rw - xfs /dev/other rw\n").unwrap();
    assert_eq!(table.whole(41, makedev(8, 2)).unwrap().parent, 1);
    assert_eq!(table.whole(43, makedev(8, 3)).unwrap().parent, 41);
    for (id,dev) in [(42, makedev(8, 2)), (41, makedev(8, 3)), (9999, makedev(8, 2))] {
        assert_eq!(table.whole(id, dev).err().unwrap().kind(), io::ErrorKind::Unsupported);
    }
}

#[test]
fn a_self_parent_whole_mount_at_the_selected_root_positive_boundary_is_required() {
    let table = Mounts::parse(b"41 41 8:2 / / rw - ext4 /dev/test rw\n").unwrap();
    let root = Mount { id: 41, at_root: Some(true) };
    assert!(table.require_root_boundary(root, makedev(8, 2)).is_ok());
    for at_root in [None, Some(false)] {
        assert_eq!(table.require_root_boundary(Mount { at_root, ..root }, makedev(8, 2)).unwrap_err().kind(),
                   io::ErrorKind::Unsupported);
    }
}

#[test]
fn missing_parent_cannot_distinguish_a_chroot_at_a_full_mount_from_an_initramfs_host() {
    let table = Mounts::parse(b"41 87 8:2 / / rw - ext4 /dev/test rw\n").unwrap();
    let root = Mount { id: 41, at_root: Some(true) };
    assert!(!table.0.contains_key(&87));
    assert!(table.whole(41, makedev(8, 2)).is_ok());
    assert_eq!(table.require_root_boundary(root, makedev(8, 2)).unwrap_err().kind(), io::ErrorKind::Unsupported);
}

#[test]
fn partial_oversized_and_ambiguous_mount_evidence_is_never_a_fallback() {
    let row = b"41 1 8:2 / / rw - ext4 /dev/test rw\n";
    for bytes in [&b""[..], &row[..row.len()-1], b"1 1 8:2 / / rw - ext4\n",
                  b"+1 2 8:2 / / rw - ext4 /dev/test rw\n",
                  b"1 2 0:4294967296 / / rw - ext4 /dev/test rw\n",
                  b"1 2 8:2 / / rw ext4 /dev/test rw\n"] {
        assert!(Mounts::parse(bytes).is_err());
    }
    assert!(Mounts::parse(&row.repeat(2)).is_err(), "duplicate even when otherwise identical");
    assert!(Mounts::parse(&vec![b'x'; TABLE_BYTES+1]).is_err());
    let mut lines = Vec::new();
    for id in 1..=TABLE_ROWS {
        use std::io::Write;
        writeln!(&mut lines, "{id} 1 8:2 / / rw - ext4 /dev/test rw").unwrap();
    }
    assert!(Mounts::parse(&lines).is_ok());
    lines.extend_from_slice(b"999999 1 8:2 / / rw - ext4 /dev/test rw\n");
    assert!(Mounts::parse(&lines).is_err());
}

#[test]
fn independently_provisioned_subtree_bind_is_unsupported_not_an_empty_or_damaged_volume() {
    let private = subtree_bind_directory();
    let name = Pinned::open(private.path(), Kind::StorageRoot).unwrap();
    let held = name.storage_directory().unwrap();
    let table = Mounts::current().unwrap();
    let mount = Mount::on(&held).unwrap().unwrap();
    assert!(!table.0[&mount.id].whole,
        "test precondition: choose an actual subtree bind, not a directory on a full-root mount");
    assert_eq!(table.unclaimed_chain(&name, &held).unwrap_err().kind(), io::ErrorKind::Unsupported);
    assert_eq!(fs::read_dir(private.path()).unwrap().count(), 0);
}


#[test]
fn directly_matching_mount_ids_include_bind_rows_without_inventing_lower_devices() {
    let raw=b"12 1 8:1 / /a rw - ext4 /dev/sda1 rw\n79 12 8:1 /b /other rw - ext4 /dev/sda1 rw\n101 1 0:4 / /dev rw - devtmpfs devtmpfs rw\n103 101 0:4 /disk /hide rw - devtmpfs devtmpfs rw\n";
    let m=Mounts::parse(raw).unwrap();
    assert_eq!(m.ids_for_device(makedev(8,1)).collect::<Vec<_>>(),[12,79]);
    assert_eq!(m.ids_for_device(makedev(8,0)).count(),0); // no direct match is not evidence the underlying disk is unused
    assert!(m.is_full_devtmpfs(101,makedev(0,4)));
    assert!(!m.is_full_devtmpfs(101,makedev(0,7))); // same mount ID alone cannot authenticate the alias source
    assert!(!m.is_full_devtmpfs(103,makedev(0,4))&&!m.is_full_devtmpfs(12,makedev(8,1)));
}

#[test]
fn normal_fixed_os_parents_may_have_either_or_both_distinct_original_mounts(){
    let mounts=Mounts::parse(b"41 1 8:1 / / rw - ext4 /dev/root rw\n\
        52 41 8:2 / /var rw - xfs /dev/var rw\n\
        63 52 8:3 / /var/lib rw - ext4 /dev/lib rw\n").unwrap();
    let root=|id|Mount{id,at_root:Some(true)};
    let entity=|minor,ino|Entity{dev:makedev(8,minor),ino};
    mounts.require_instance_parent_mount(0,root(52),41,entity(2,2),XFS_SUPER_MAGIC).unwrap();
    mounts.require_instance_parent_mount(1,root(63),52,entity(3,2),EXT4_SUPER_MAGIC).unwrap();
    let independent_lib=Mounts::parse(b"41 1 8:1 / / rw - ext4 /dev/root rw\n\
         64 41 8:3 / /var/lib rw - ext4 /dev/lib rw\n").unwrap();
    independent_lib.require_instance_parent_mount(1,root(64),41,entity(3,2),EXT4_SUPER_MAGIC).unwrap();

    for (level,mount,parent,object,magic) in [
        (0,root(52),52,entity(2,2),XFS_SUPER_MAGIC), // cannot be its own parent
        (1,root(63),41,entity(3,2),EXT4_SUPER_MAGIC), // false parent
        (1,root(52),41,entity(2,2),XFS_SUPER_MAGIC), // wrong visible name
        (0,root(52),41,entity(1,2),XFS_SUPER_MAGIC), // wrong superblock device
        (0,root(52),41,entity(2,2),EXT4_SUPER_MAGIC), // wrong real fstatfs type
        (2,root(52),41,entity(2,2),XFS_SUPER_MAGIC), // not an OS parent level
        (0,Mount{id:52,at_root:None},41,entity(2,2),XFS_SUPER_MAGIC),
        (0,Mount{id:52,at_root:Some(false)},41,entity(2,2),XFS_SUPER_MAGIC),
    ]{assert!(mounts.require_instance_parent_mount(level,mount,parent,object,magic).is_err());}
}

#[test]
fn bind_subtree_and_alias_fail_at_named_boundary_but_btrfs_has_two_device_meanings(){
    let mounts=Mounts::parse(b"41 1 8:1 / / rw - ext4 /dev/root rw\n\
        51 41 8:1 /someone-elses-live-dir /var rw - ext4 /dev/root rw\n\
        52 41 0:20 /@varlib /var/lib rw - btrfs /dev/tree rw\n\
        53 41 0:20 /@varlib /var/lib/local-ops-helper rw - btrfs /dev/tree rw\n").unwrap();
    let root=|id|Mount{id,at_root:Some(true)};
    // Linux Btrfs reports the subvolume's anon_dev in stat, not 0:20.
    let subvolume=Entity{dev:makedev(0,42),ino:256};
    assert!(mounts.require_instance_parent_mount(0,root(51),41,Entity{dev:makedev(8,1),ino:888},
        EXT4_SUPER_MAGIC).is_err());
    assert!(mounts.require_instance_parent_mount(1,root(52),41,subvolume,BTRFS_SUPER_MAGIC).is_ok());
    assert!(mounts.require_instance_parent_mount(1,root(52),41,Entity{ino:257,..subvolume},BTRFS_SUPER_MAGIC).is_err());
    assert!(mounts.require_instance_parent_mount(1,root(53),41,subvolume,BTRFS_SUPER_MAGIC).is_err());
}

#[test]
fn nonempty_v17_rejects_remote_ancestors_and_ambiguous_crossings_before_a_path_open(){
    let raw=b"41 41 8:1 / / rw - ext4 /dev/root rw
\
        42 41 8:2 / /mnt/local-\xff rw - xfs /dev/local rw
\
        43 41 0:7 / /mnt/remote rw - nfs4 server:/data rw
\
        44 43 8:3 / /mnt/remote/real-local rw - ext4 /dev/nested rw
\
        45 41 0:9 / /mnt/auto rw - autofs systemd-1 rw
\
        46 41 8:5 / /mnt/stack rw - ext4 /dev/first rw
\
        47 46 0:10 / /mnt/stack rw - cifs //server/other rw
";
    let cut=Mounts::discovery_table(raw,false).unwrap();
    assert!(cut.roots.is_empty());
    assert_eq!(cut.named(Path::new("/"),None).unwrap(),Some(41));
    let local=Path::new(OsStr::from_bytes(b"/mnt/local-\xff"));
    assert_eq!(cut.named(local,Some(41)).unwrap(),Some(42));
    assert!(cut.named(local,Some(43)).is_err());
    assert!(cut.named(Path::new("/mnt/remote/real-local"),Some(43)).is_err());
    assert!(cut.named(Path::new("/mnt/remote"),Some(41)).is_err());
    assert!(cut.named(Path::new("/mnt/auto"),Some(41)).is_err());
    assert!(cut.named(Path::new("/mnt/stack"),Some(41)).is_err());
    assert_eq!(cut.named(Path::new("/mnt/next-same-mount"),Some(41)).unwrap(),None);
    assert!(Mounts::discover(raw).unwrap().iter().all(|r|r.id!=42&&r.id!=44));
}
