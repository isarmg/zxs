//! Codec and size proof only. No source/target custody, live permission or
//! durable RESTORE is minted by successfully encoding or decoding these bytes.
use super::*;
use super::super::purpose::{Call,DeletedAt,Purpose};
use crate::fs::{Birth,Entity};
fn n(dev:u64,tag:u8)->NativeObject{
    let mut bytes=[tag;fs::MAX_BIRTH];bytes[..16].fill(dev as u8);
    NativeObject{entity:Entity{device:dev,inode:u64::from(tag)},birth:Birth::decode(&bytes).unwrap()}
}
fn max_origin(purpose:Purpose)->Origin{Origin{op:OperationId([1;16]),uid:9,file:FileId([2;16]),entry:EntryId([3;16]),
    history:HistoryId([4;16]),from:VolumeId([5;16]),to:VolumeId([6;16]),epoch:9,name:"n".repeat(wire::MAX_HUMAN),
    ticket:OperationId([7;16]),mode:Method::Transfer,purpose,src_root:n(1,1),dst_root:n(2,2),creation:[8;32],parent:[9;32],
    last:Some([10;32]),source_bytes:u64::MAX,source_material:[11;32]}}
fn sites()->Sites{Sites{directory:n(2,5),local:n(2,6),node:n(2,7),raw:n(2,8),incr:Some(n(2,9))}}
#[test]
fn any_accepted_return_fits_even_at_every_native_and_name_maximum(){
    let pool=PoolId([12;16]);let by=OperationId([15;16]);let previous=[16;32];
    let restore=Purpose::Restore{deleted_by:by,previous};
    let mut long=max_origin(restore);long.name.push('n');
    assert!(Moved::seal(pool,55,[17;32],long,[18;32],Some(7),sites()).is_err());
    let o=max_origin(restore);let mut b=Vec::new();o.put(&mut b).unwrap();assert_eq!(b.len(),MAX_ORIGIN);
    let m=Moved::seal(pool,55,[17;32],o,[18;32],Some(7),sites()).unwrap();
    assert_eq!(m.bytes.len(),MAX_CUSTODY_FRAME);assert!(m.bytes.len()<wire::MAX_FRAME);
    assert_eq!(Moved::read(pool,&m.bytes,wire::envelope(pool,&m.bytes).unwrap()).unwrap(),m);
    let del=Purpose::Delete{previous,at:DeletedAt::new(u64::MAX,999_999_999).unwrap()};
    let d=Moved::seal(pool,54,[19;32],max_origin(del),[20;32],Some(7),sites()).unwrap();
    assert_eq!(d.bytes.len()+4,m.bytes.len());
    assert_eq!(Moved::read(pool,&d.bytes,wire::envelope(pool,&d.bytes).unwrap()).unwrap(),d);
    assert!(DeletedAt::new(0,1_000_000_000).is_err());
    let mut altered=d.bytes.clone();let i=wire::ENVELOPE_BYTES-32+7*16+4+8;
    assert_eq!(altered[i],2);altered[i]=1;
    let end=altered.len()-32;let signed=crate::hash::digest(&altered[..end]).unwrap();altered[end..].copy_from_slice(&signed);
    assert!(Moved::read(pool,&altered,wire::envelope(pool,&altered).unwrap()).is_err());
}
#[test]
fn distinct_requests_addresses_and_physical_methods_cannot_be_relabelled(){
    let pool=PoolId([22;16]);let op=OperationId([1;16]);let by=OperationId([15;16]);let file=FileId([2;16]);
    let(a,b)=(VolumeId([5;16]),VolumeId([6;16]));let prev=[16;32];
    let req=|c:Call,e:u64|c.request(pool,9,op,file,a,b,e);
    assert_ne!(req(Call::Move,0).unwrap(),req(Call::Delete,0).unwrap());assert!(req(Call::Move,1).is_err());
    let r=Purpose::Restore{deleted_by:by,previous:prev};
    assert_ne!(req(r.call(),2).unwrap(),req(Call::Delete,2).unwrap());
    assert_ne!(req(r.call(),2).unwrap(),req(Call::Restore{deleted_by:by,decision:[17;32]},2).unwrap());
    assert!(req(r.call(),0).is_err());assert!(req(r.call(),u64::MAX).is_err());
    assert!(req(Call::Restore{deleted_by:op,decision:prev},2).is_err());
    let d=Purpose::Delete{previous:prev,at:DeletedAt::new(0,0).unwrap()};
    let(h,kept)=d.leaves(file,by,"same").unwrap();assert_eq!(r.leaves(file,op,"same").unwrap(),(kept.clone(),h));
    assert_ne!(kept,d.leaves(FileId([3;16]),by,"same").unwrap().1);
    assert_ne!(kept,d.leaves(file,op,"same").unwrap().1);assert!(wire::human_name(kept.to_bytes()).is_err());
    assert!(r.check_transfer(Method::Rename,&n(1,25),&n(2,26)).is_err());
    let x=n(1,25);let mut y=n(2,26);y.entity.device=1;assert!(r.check_transfer(Method::Transfer,&x,&y).is_err());
    let mut z=n(1,26);z.entity.device=2;assert!(r.check_transfer(Method::Transfer,&x,&z).is_err());
}
