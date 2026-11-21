//! Pure access translation only. No test is an authenticated Core opener.
use super::*;

#[test]
fn root_grant_is_narrow_and_expands_only_actual_root_rights(){
    assert_eq!(grant(LIST_DIRECTORY).unwrap().0,LIST_DIRECTORY);
    assert_eq!(grant(TRAVERSE).unwrap().0,TRAVERSE|LIST_DIRECTORY);
    assert_eq!(grant(MAXIMUM_ALLOWED).unwrap().0,ROOT_RIGHTS);
    assert_eq!(grant(READ_ATTRIBUTES|SYNCHRONIZE).unwrap().0,READ_ATTRIBUTES|SYNCHRONIZE);
    for invalid in [0,2,4,8,0x0001_0000,0x0002_0000,0x8000_0000,MAXIMUM_ALLOWED|4]{
        assert!(matches!(grant(invalid),Err(RootOpenError::AccessDenied)));
    }
}

#[test]
fn root_maximum_follows_the_object_policy_without_a_fake_timestamp(){
    assert_eq!(root_maximum(None).unwrap(),None);
    assert_eq!(root_maximum(Some(MaximumAccessQuery::Unconditional)).unwrap(),Some(Grant(ROOT_RIGHTS)));
    // The requested OPEN may have fewer rights than the object's policy.
    assert_ne!(grant(READ_ATTRIBUTES|SYNCHRONIZE).unwrap(),Grant(ROOT_RIGHTS));
    for supplied in [0,1,u64::MAX]{
        assert!(matches!(root_maximum(Some(MaximumAccessQuery::CompareChangeTime(supplied))),
            Err(RootOpenError::ContextRequired)));
    }
}
