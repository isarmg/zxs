use super::*;
use crate::{fs::HeldDir,test_support::PrivateTemp};
#[test]
#[ignore="isolated actual owned S directory and separate OFD, without SQLite or privileged mounts"]
fn projected_original_is_checked_before_system_lock_or_any_member(){
    let temp=PrivateTemp::under(&std::env::temp_dir());
    let root=HeldDir::trusted_system(&temp.0).unwrap();
    root.exclusive().unwrap();
    let mut other=root.entity;
    other.inode=other.inode.checked_add(1).unwrap();
    let reject=SystemDir::hold_qualified(&temp.0,4096,|d|require_projection(&d.file,d.entity,other,false));
    assert_eq!(reject.err().unwrap().kind(),io::ErrorKind::PermissionDenied);
    let reject=SystemDir::hold_qualified(&temp.0,4096,|d|require_projection(&d.file,d.entity,root.entity,true));
    assert_eq!(reject.err().unwrap().kind(),io::ErrorKind::PermissionDenied);
    let blocked=SystemDir::hold_qualified(&temp.0,4096,|d|require_projection(&d.file,d.entity,root.entity,false));
    assert_eq!(blocked.err().unwrap().kind(),io::ErrorKind::WouldBlock);
}
