use super::*;
use crate::file_container::retained::{RetainedDiskNumber,RetainedDiskRegistration,RetainedName};
use crate::{DiskId,PoolId,Timestamp};

fn op(n:u8)->OperationId {OperationId::from_random([n;16])}
fn allocation()->RetainedAllocation {
    RetainedAllocation{registration:RetainedDiskRegistration{
        pool:PoolId::from_random([1;16]),disk:DiskId::from_random([2;16]),
        number:RetainedDiskNumber::new(0).unwrap(),operation:op(4),
    },created_at:Timestamp::from_unix_seconds(1_794_878_700),
        name:RetainedName::parse("00126bh125abcde-private.txt").unwrap()}
}

#[test]
fn one_registered_file_site_changes_only_from_its_exact_predecessor() {
    let file=FileId::from_random([3;16]);
    let other=FileId::from_random([7;16]);
    let created=FileSite::Created{};
    assert!(created.restore(op(4),op(5)).is_err());
    assert!(created.hide(op(4),allocation(),"renamed.txt").is_err());
    let hidden=created.hide(op(4),allocation(),"private.txt").unwrap();
    assert_eq!(hidden.generation().get(),2);
    assert!(hidden.hide(op(5),allocation(),"private.txt").is_err());
    assert!(hidden.restore(op(6),op(5)).is_err());
    assert!(hidden.restore(op(4),op(4)).is_err());
    let restored=hidden.restore(op(4),op(5)).unwrap();
    assert_eq!(restored.generation().get(),3);
    assert!(restored.hide(op(4),allocation(),"private.txt").is_err());
    assert_eq!(restored.hide(op(6),allocation(),"private.txt").unwrap().generation().get(),4);
    for value in [&created,&hidden,&restored] {
        assert!(value.valid_shape());
        assert_eq!(&crate::decode_strict::<FileSite>(&crate::canonical_json(value).unwrap()).unwrap(),value);
        let inside=value.witness(file).unwrap();
        assert_ne!(inside.registration,value.witness(other).unwrap().registration);
        assert_eq!(inside.generation,value.generation());
        let public=String::from_utf8(crate::canonical_json(&inside).unwrap()).unwrap();
        assert!(!public.contains("private.txt") && !public.contains("00126bh125abcde-") && !public.contains("disk_"));
    }
    assert_ne!(created.witness(file).unwrap(),restored.witness(file).unwrap());
    assert!(crate::decode_strict::<FileSite>(br#"{"state":"created","retained":"outside"}"#).is_err());
    assert!(crate::decode_strict::<FileSite>(br#"{}"#).is_err());
    let bogus=FileSite::Retained{generation:Generation::new(3),hide:op(4),allocation:allocation()};
    assert!(!bogus.valid_shape());
    assert!(bogus.restore(op(4),op(5)).is_err());
    let exhausted=FileSite::Restored{generation:Generation::new(u64::MAX),hide:op(4),restore:op(5)};
    assert!(exhausted.hide(op(6),allocation(),"private.txt").is_err());
}
