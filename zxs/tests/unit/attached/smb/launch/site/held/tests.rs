use super::*;
#[test]
fn only_the_product_root_can_cross_in_a_fixed_state_or_log_walk(){
    let fixed=Role::FixedDir;
    assert!(fixed.permits_crossing(Path::new("/zxs")));
    for n in ["/zxs/state","/zxs/state/112233","/zxs/log","/other"]{
        assert!(!fixed.permits_crossing(Path::new(n)));
    }
    let product=Entity{device:31,inode:11};
    for role in [Role::System(11,product),Role::Media(11,product)]{
        assert!(role.permits_crossing(Path::new("/zxs/disks/data/a")));
        assert!(role.permits_crossing(Path::new("/explicit/approved/volume")));
    }
}
#[test]
fn rust_site_mount_selection_has_no_unlisted_remote_or_private_slot_crossing(){
    let table=b"12 12 8:1 / / rw - ext4 /dev/fake-base rw\n\
            13 12 8:2 / /zxs rw - xfs /dev/fake-var rw\n\
            14 13 8:2 /borrowed /zxs/subtree rw - xfs /dev/fake-var rw\n\
            15 13 0:31 / /zxs/remote rw - nfs alpha:/fake rw\n\
            16 13 8:3 / /zxs/known\\040site rw - ext4 /dev/fake-known rw\n\
            17 13 8:3 / /zxs/staged rw - ext4 /dev/fake-known rw\n\
            18 17 8:4 / /zxs/staged rw - ext4 /dev/fake-other rw\n";
    let t=Mounts::parse(table).unwrap();
    assert_eq!(t.crossing(b"/zxs",12,true).unwrap(),Some(13));
    assert_eq!(t.crossing(b"/zxs/known site",13,true).unwrap(),Some(16));
    assert_eq!(t.crossing(b"/zxs/other",13,true).unwrap(),None);
    for n in [b"/zxs/subtree".as_slice(),b"/zxs/remote",b"/zxs/staged"]{
        assert!(t.crossing(n,13,true).is_err());
    }
    assert!(t.crossing(b"/zxs/known site",12,true).is_err());
    assert!(t.crossing(b"/zxs/known site",13,false).is_err());
}
