use super::*;

#[test]
fn retained_mask_never_invents_mutation_or_purge_authority(){
    assert_eq!(retained_grant(MAXIMUM_ALLOWED,false).unwrap().0,FILE_READ_RIGHTS);
    assert_eq!(retained_grant(EXECUTE,false).unwrap().0,EXECUTE|READ_DATA);
    for forbidden in [0,2,4,0x100,0x1_0000,0x4000_0000,MAXIMUM_ALLOWED|0x1_0000]{
        assert!(matches!(retained_grant(forbidden,false),Err(OpenError::AccessDenied)));
    }
}

#[test]
fn the_restore_lane_only_adds_its_exact_delete_right(){
    assert_eq!(retained_grant(MAXIMUM_ALLOWED,true).unwrap().0,FILE_READ_RIGHTS|RESTORE_DELETE);
    assert_eq!(retained_grant(RESTORE_DELETE,true).unwrap().0,RESTORE_DELETE);
    assert!(retained_grant(RESTORE_DELETE|2,true).is_err());
    assert!(retained_grant(RESTORE_DELETE|4,true).is_err());
}

#[test]
fn retained_root_round_and_underlying_file_have_distinct_vote_domains(){
    let pool=crate::PoolId([0xbb;16]);let a=crate::OperationId([0xc1;16]);let b=crate::OperationId([0xc2;16]);
    assert_eq!(directory(pool,None),Object::RecycleRoot(pool));
    assert_eq!(directory(pool,Some(a)),Object::DeleteRound(pool,a));
    assert_ne!(directory(pool,Some(a)),directory(pool,Some(b)));
    assert_ne!(directory(pool,Some(a)),Object::File(pool,crate::FileId(a.0)));
}

#[test]
fn retained_maximal_answer_is_not_the_open_requested_rights_or_share_ceiling(){
    assert_eq!(ceiling(false),FILE_READ_RIGHTS);
    assert_eq!(ceiling(true),FILE_READ_RIGHTS|RESTORE_DELETE);
    assert_eq!(retained_grant(READ_DATA,true).unwrap().0,READ_DATA);
    assert_ne!(retained_grant(READ_DATA,true).unwrap().0,ceiling(true));
}
