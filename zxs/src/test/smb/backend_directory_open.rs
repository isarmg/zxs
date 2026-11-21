use super::*;

#[test]
fn ordinary_directory_rights_do_not_turn_list_or_maximum_into_writes(){
    assert_eq!(readonly_grant(TRAVERSE).unwrap().0,TRAVERSE|LIST);
    assert_eq!(readonly_grant(MAXIMUM_ALLOWED).unwrap().0,RIGHTS);
    assert_eq!(readonly_grant(READ_ATTRIBUTES).unwrap().0,READ_ATTRIBUTES);
    for unsupported in [0,2,4,0x10000,0x40000000,MAXIMUM_ALLOWED|2]{
        assert!(matches!(readonly_grant(unsupported),Err(DirectoryOpenError::AccessDenied)));
    }
}

#[test]
fn only_explicit_delete_on_a_qualified_existing_directory_can_request_the_relabel_lane(){
    assert!(matches!(existing_grant(DELETE_ACCESS,false),Err(DirectoryOpenError::AccessDenied)));
    assert_eq!(existing_grant(DELETE_ACCESS,true).unwrap().0,DELETE_ACCESS);
    assert_eq!(existing_grant(MAXIMUM_ALLOWED,true).unwrap().0,RIGHTS);
    assert_eq!(existing_grant(DELETE_ACCESS|READ_ATTRIBUTES|SYNCHRONIZE,true).unwrap().0,
        DELETE_ACCESS|READ_ATTRIBUTES|SYNCHRONIZE);
    assert!(matches!(existing_grant(DELETE_ACCESS|2,true),Err(DirectoryOpenError::AccessDenied)));
}
#[test]
fn ordinary_volume_root_and_nested_directory_are_in_distinct_object_domains(){
    let pool=PoolId([1;16]);let volume=VolumeId([2;16]);
    let nested=DirectoryId([2;16]); // equal bytes aren't an equal object
    assert_eq!(object(pool,volume,None),Object::OrdinaryRoot(pool,volume));
    assert_eq!(object(pool,volume,Some(nested)),Object::Directory(pool,nested));
    assert_ne!(object(pool,volume,None),object(pool,volume,Some(nested)));
    assert_ne!(object(pool,volume,Some(nested)),Object::File(pool,crate::FileId([2;16])));
}
