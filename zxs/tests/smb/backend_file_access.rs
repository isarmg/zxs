//! Admission and identity fixtures only; no real OPEN or session is minted.
use super::*;
use crate::{FileId,PoolId,smb::{backend::{OpenCoordinator,Vote},wire::{direct::Frame,message::Compound,
    query_fields::QueryError}}};
use std::num::NonZeroUsize;

#[test]
fn file_access_returns_the_exact_effective_read_or_write_mask_only_for_the_same_bound_ordinary_slot(){
    let owner=OpenCoordinator::new(NonZeroUsize::new(1).unwrap());
    let pool=PoolId([1;16]);
    let file=Object::File(pool,FileId([2;16]));
    let before=Arc::new(());
    let mut slot=Slot{owner,object:file,vote:Vote{needs:0,denies:0},bound:true,retained:false,released:false,
        identity:Arc::clone(&before)};
    let weak=Arc::downgrade(&before);
    for mask in [1,2,4,0x0011_0000,0x0011_00a7]{
        let grant=Grant::from_effective(mask).unwrap();
        let observed=Access::of(&slot,grant,file).unwrap();
        assert_eq!(observed.grant.0,mask); // neither added READ_ATTRIBUTES nor share ceiling
        assert!(observed.on(&weak,&slot,grant,file).is_ok());
        assert!(observed.on(&weak,&slot,Grant::from_effective(mask^0x80).unwrap(),file).is_err());
        assert!(observed.on(&weak,&slot,grant,Object::File(pool,FileId([3;16]))).is_err());
        slot.identity=Arc::new(());
        assert!(observed.on(&weak,&slot,grant,file).is_err());
        slot.identity=Arc::clone(&before);
    }
    let grant=Grant::from_effective(1).unwrap();
    slot.retained=true;
    assert!(Access::of(&slot,grant,file).is_err());
    slot.retained=false;slot.bound=false;
    assert!(Access::of(&slot,grant,file).is_err());
    slot.bound=true;slot.object=Object::Namespace(pool);
    assert!(Access::of(&slot,grant,slot.object).is_err());
    slot.object=file;slot.released=true;
    assert!(Access::of(&slot,grant,file).is_err());
}

fn compound(raw:&[u8])->Compound<'_>{
    let (frame,tail)=Frame::split(raw).ok().unwrap().unwrap();
    assert!(tail.is_empty());Compound::parse(frame).ok().unwrap()
}
fn framed(messages:&[&[u8]])->Vec<u8>{
    let size:usize=messages.iter().map(|p|p.len()).sum();
    let mut out=(size as u32).to_be_bytes().to_vec();
    for bytes in messages{out.extend_from_slice(bytes);}
    out
}
#[test]
fn file_access_shape_does_not_borrow_predecessor_open_or_another_information_class(){
    let mut request=[0;104];
    request[..4].copy_from_slice(b"\xfeSMB");request[4]=64;request[12]=16;request[16]=8;
    request[36]=9;request[40]=3;request[64]=41;request[66]=1;request[67]=8;request[68]=4;
    request[88]=5;request[96]=7;
    let frame=framed(&[&request]);let on=compound(&frame);
    assert_eq!(request_output(&on.messages()[0],0,65536).unwrap(),4);
    assert!(matches!(request_output(&on.messages()[0],0,3),Err(FileStandardError::Fields(QueryError::ExceedsMaxTransact))));
    let mut first=request;first[20]=104;let frame=framed(&[&first,&request]);let on=compound(&frame);
    for (ordinal,member) in on.messages().iter().enumerate(){
        assert!(matches!(request_output(member,ordinal,65536),Err(FileStandardError::NotSupported)));
    }
    let mut missing=request;missing[88..104].fill(0xff);
    let frame=framed(&[&missing]);let on=compound(&frame);
    assert!(matches!(request_output(&on.messages()[0],0,65536),Err(FileStandardError::NotSupported)));
    let mut other=request;other[67]=35;
    let frame=framed(&[&other]);let on=compound(&frame);
    assert!(matches!(request_output(&on.messages()[0],0,65536),Err(FileStandardError::NotSupported)));
}
