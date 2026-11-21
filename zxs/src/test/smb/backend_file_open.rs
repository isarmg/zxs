//! Policy and allocation examples only; no fake authority or storage OPEN.
use super::*;

#[test]
fn a_read_only_file_cannot_turn_a_requested_write_or_zero_mask_into_a_grant(){
    assert!(matches!(read_grant(0,false),Err(FileOpenError::AccessDenied)));
    for blocked in [0x2,0x4,0x1_0000,0x4000_0000,MAXIMUM_ALLOWED|0x2]{
        assert!(matches!(read_grant(blocked,false),Err(FileOpenError::AccessDenied)));
    }
    assert_eq!(read_grant(EXECUTE,false).unwrap().0,EXECUTE|READ_DATA);
    assert_eq!(read_grant(READ_ATTRIBUTES,false).unwrap().0,READ_ATTRIBUTES);
    assert_eq!(read_grant(MAXIMUM_ALLOWED,false).unwrap().0,FILE_READ_RIGHTS);
    assert_eq!(read_grant(DELETE_ACCESS,true).unwrap().0,DELETE_ACCESS);
    assert_eq!(read_grant(EXECUTE|DELETE_ACCESS,true).unwrap().0,EXECUTE|READ_DATA|DELETE_ACCESS);
}
#[test]
fn ordinary_file_maximal_policy_is_not_the_permission_granted_to_the_handle(){
    let just_delete=0x0011_0080;
    assert_eq!(read_grant(just_delete,true).unwrap().0,just_delete);
    assert_eq!(file_ceiling(true),0x0011_00a7);
    assert_eq!(file_ceiling(false),0x0010_00a1);
    let advertised=file_ceiling(true);
    let granted=read_grant(just_delete,true).unwrap().0;
    assert_eq!(granted&0x7,0);
    assert_eq!(advertised&0x7,0x7);
}
#[test]
fn logical_allocation_is_not_a_disk_size_measurement(){
    for (eof,bytes) in [(0,0),(1,4096),(4096,4096),(4097,8192),(i64::MAX as u64,(i64::MAX as u64)&!4095)]{
        assert_eq!(allocation(eof),bytes);
    }
}
