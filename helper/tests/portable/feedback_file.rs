//! Deterministic post-sync boundary. Operates only on newly created fixture files.
use super::*;
use crate::portable_claim::{Claim,Role};
use std::{fs::{self,FileTimes},os::unix::fs::PermissionsExt,path::PathBuf,time::{Duration,SystemTime}};

#[test]
fn same_inode_same_bytes_or_a_changed_parent_do_not_inherit_the_previous_sync_cut() {
    let location=PathBuf::from(std::env::var_os("HELPER_TEST_PORTABLE_DATA_PARENT")
        .expect("supply the private native test location"));
    let area=tempfile::Builder::new().prefix("helper-feedback-").tempdir_in(location).unwrap();
    fs::set_permissions(area.path(),fs::Permissions::from_mode(0o700)).unwrap();
    let dir=File::open(area.path()).unwrap();
    let uid=dir.metadata().unwrap().uid();
    assert_ne!(uid,0,"use the dedicated non-root fixture owner");
    let output=Output::Marker;
    let original=output.create(&dir).unwrap();
    output.require_new(&dir,&Stamp::root(&dir,uid).unwrap(),&original).unwrap();
    let expected=Claim::from_fields(Role::Data,[1;16],[2;16],[3;16],[4;32]).unwrap().encode();
    original.write_all_at(&expected,0).unwrap();
    output.sync_and_readback(&dir,uid,&original,&expected).unwrap();

    let file=Stamp::on(&original).unwrap(); let parent=Stamp::root(&dir,uid).unwrap();
    original.sync_all().unwrap(); dir.sync_all().unwrap();
    assert!(output.readback(&dir,uid,&original,&expected,&parent,&file).is_ok());
    original.write_all_at(&expected,0).unwrap();
    // Set a distinct, representable mtime rather than relying on clock ticks.
    let time=|seconds|FileTimes::new().set_modified(SystemTime::UNIX_EPOCH+Duration::from_secs(seconds));
    original.set_times(time(86400)).unwrap();
    let after=Stamp::on(&original).unwrap();
    assert_eq!(file.entity,after.entity); assert_eq!(file.len,after.len);
    assert_ne!(file.modified,after.modified);
    let mut actual=[0;CLAIM_LEN]; original.read_exact_at(&mut actual,0).unwrap();
    assert_eq!(actual,expected);
    assert!(output.readback(&dir,uid,&original,&expected,&parent,&file).is_err());

    // Now the marker remains entirely unchanged, including its metadata. Only
    // the directory was modified after this independent synchronization cut.
    let parent=Stamp::root(&dir,uid).unwrap();
    original.sync_all().unwrap(); dir.sync_all().unwrap();
    assert!(output.readback(&dir,uid,&original,&expected,&parent,&after).is_ok());
    let scratch=area.path().join("this-fixtures-temporary-neighbour");
    File::create_new(&scratch).unwrap(); fs::remove_file(scratch).unwrap();
    dir.set_times(time(172800)).unwrap();
    assert!(Stamp::on(&original).unwrap()==after);
    assert_ne!(Stamp::root(&dir,uid).unwrap().modified,parent.modified);
    assert!(output.readback(&dir,uid,&original,&expected,&parent,&after).is_err());
}
