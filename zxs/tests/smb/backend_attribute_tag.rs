//! Tests of slot classification only, not a session, filesystem, or reply.
use super::*;
use crate::{DirectoryId,FileId,OperationId,PoolId,VolumeId,smb::backend::{OpenCoordinator,Vote}};
use std::num::NonZeroUsize;

#[test]
fn class35_uses_only_live_eligible_objects_with_granted_read_attributes(){
    let owner=OpenCoordinator::new(NonZeroUsize::new(1).unwrap());
    let pool=PoolId([1;16]);
    let attrs=Grant::from_effective(READ_ATTRIBUTES).unwrap();
    for (object,expected) in [
        (Object::Namespace(pool),Some(Kind::Directory)),
        (Object::OrdinaryRoot(pool,VolumeId([2;16])),Some(Kind::Directory)),
        (Object::Directory(pool,DirectoryId([3;16])),Some(Kind::Directory)),
        (Object::File(pool,FileId([4;16])),Some(Kind::File)),
        (Object::RecycleRoot(pool),None),
        (Object::DeleteRound(pool,OperationId([5;16])),None),
    ]{
        // This isolated classifier never obtains a Core object or a real
        // share vote. Its test slot has no entry in the coordinator.
        let mut slot=Slot{owner:Arc::clone(&owner),object,vote:Vote{needs:0,denies:0},
            bound:true,retained:false,released:false,identity:Arc::new(())};
        let held=Kind::of(&slot,attrs).ok();
        assert_eq!(held,expected);
        assert_eq!(held.map(Kind::attributes),expected.map(|kind|match kind{
            Kind::File=>0x20,Kind::Directory=>0x10,
        }));
        for other in [1,2,0x20,0x10000,0x0010_0000]{
            assert_eq!(Kind::of(&slot,Grant::from_effective(other).unwrap()).unwrap_err().kind(),
                io::ErrorKind::PermissionDenied);
        }
        slot.bound=false;
        assert_eq!(Kind::of(&slot,attrs).unwrap_err().kind(),io::ErrorKind::PermissionDenied);
        slot.bound=true;slot.retained=true;
        assert_eq!(Kind::of(&slot,attrs).unwrap_err().kind(),io::ErrorKind::PermissionDenied);
        slot.retained=false;slot.released=true;
        assert_eq!(Kind::of(&slot,attrs).unwrap_err().kind(),io::ErrorKind::PermissionDenied);
    }
}
