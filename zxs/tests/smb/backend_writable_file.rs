use super::*;

#[test]
fn writable_grant_does_not_invent_generic_or_other_write_rights(){
    assert!(!explicit_writer(MAXIMUM_ALLOWED));
    assert!(explicit_writer(WRITE_DATA));
    assert!(explicit_writer(0x4000_0000));
    assert_eq!(write_grant(WRITE_DATA).unwrap().0,WRITE_DATA);
    assert_eq!(write_grant(APPEND_DATA).unwrap().0,APPEND_DATA);
    assert_eq!(write_grant(WRITE_DATA|EXECUTE).unwrap().0,WRITE_DATA|EXECUTE|READ_DATA);
    assert_eq!(write_grant(MAXIMUM_ALLOWED).unwrap().0,FILE_RIGHTS);
    assert_eq!(write_grant(WRITE_DATA|DELETE_ACCESS).unwrap().0,WRITE_DATA|DELETE_ACCESS);
    for wrong in [0,READ_DATA,0x10,0x100,0x4000_0000|WRITE_DATA,0x8000_0000|WRITE_DATA,DELETE_ACCESS]{
        assert!(matches!(write_grant(wrong),Err(FileOpenError::AccessDenied)));
    }
}
