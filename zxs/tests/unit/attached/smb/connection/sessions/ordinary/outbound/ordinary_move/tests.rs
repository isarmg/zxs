use super::*;
use crate::smb::wire::{direct::Frame,message::CANCEL};

#[test]
fn before_interim_cancel_matches_the_original_mid_and_verified_sid_not_the_tree_or_async_field(){
    // Routing only. The real receiver independently authenticates the
    // complete original CMAC; this cannot produce a cancellation proof.
    let sid=NonZeroU64::new(17).unwrap();
    let request=RequestSlot{sid,tree:71,mid:13};
    for (mid,flags,routing,pre,post) in [(13u64,8u8,0u64,true,true),(13,8,999u64<<32,true,true),
        (14,8,71u64<<32,false,false),(13,10,13,false,false),(0,10,14,false,true),(0,10,0,false,false)]
    {
        let mut p=[0u8;72];
        p[..4].copy_from_slice(&68u32.to_be_bytes());
        p[4..8].copy_from_slice(b"\xfeSMB");
        p[8]=64;p[16..18].copy_from_slice(&CANCEL.to_le_bytes());p[20]=flags;
        p[28..36].copy_from_slice(&mid.to_le_bytes());
        p[36..44].copy_from_slice(&routing.to_le_bytes());
        p[44..52].copy_from_slice(&sid.get().to_le_bytes());
        p[68..70].copy_from_slice(&4u16.to_le_bytes());
        p[70]=0x6a; // ignored reserved field
        let parsed=Compound::parse(Frame::split(&p).unwrap().unwrap().0).unwrap();
        let m=cancel_shape(&parsed).unwrap();
        assert_eq!(targets_original(m,sid,&request,None),pre);
        assert_eq!(targets_original(m,sid,&request,NonZeroU64::new(14)),post);
        assert!(!targets_original(m,NonZeroU64::new(18).unwrap(),&request,NonZeroU64::new(14)));
    }
}
