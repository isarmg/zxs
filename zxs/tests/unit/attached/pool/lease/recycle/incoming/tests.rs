use super::*;
#[test]
#[ignore="two caller-provided private aliases of exactly one directory through distinct existing Linux mount instances"]
fn two_mounts_of_the_same_directory_do_not_make_its_original_unnamed_file_linkable_on_both(){
    let get=|key:&str|fs::HeldDir::open(Path::new(&std::env::var_os(key).expect("two private test mount aliases required"))).unwrap();
    let original=get("ZXS_TEST_LINK_MOUNT_A");let other=get("ZXS_TEST_LINK_MOUNT_B");
    assert_eq!(original.entity,other.entity);assert_eq!(Birth::read(&original.file).unwrap(),Birth::read(&other.file).unwrap());
    assert!(!original.same_mount(&other).unwrap());
    let(f,_)=original.anonymous_record().unwrap();let before=stream::anonymous_stamp(&f).unwrap();
    assert!(original.shares_file_mount(&f).unwrap());assert!(!other.shares_file_mount(&f).unwrap());
    assert_eq!(stream::anonymous_stamp(&f).unwrap(),before); // still anonymous; no named output or mount operation
}
