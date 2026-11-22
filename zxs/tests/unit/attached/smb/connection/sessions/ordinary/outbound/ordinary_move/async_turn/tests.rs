use super::*;
use crate::smb::{connection::CAP,wire::{direct::Frame,message::Compound,set_info::SET_INFO}};

#[test]
fn interim_bits_and_window_reserve_the_one_unfinished_member(){
    let mut raw=[0;70];
    raw[..4].copy_from_slice(&66u32.to_be_bytes());
    raw[4..8].copy_from_slice(b"\xfeSMB");
    raw[8]=64;
    raw[16..18].copy_from_slice(&SET_INFO.to_le_bytes());
    raw[18..20].copy_from_slice(&CAP.to_le_bytes());
    raw[20..24].copy_from_slice(&SIGNED.to_le_bytes());
    raw[28..36].copy_from_slice(&11u64.to_le_bytes());
    raw[44..52].copy_from_slice(&7u64.to_le_bytes());
    raw[52..68].fill(0x8f);
    let parsed=Compound::parse(Frame::split(&raw).unwrap().unwrap().0).unwrap();
    let m=&parsed.messages()[0];
    let before=Window{low:12,next:12,bits:0};
    let plan=Interim::plan(before,1,m).unwrap();
    assert_eq!(before.available(),0);
    assert_eq!(plan.window.available(),CAP-1);
    let bytes=plan.bytes(m);
    assert_eq!(bytes[8..12],0x103u32.to_le_bytes());
    assert_eq!(bytes[12..14],SET_INFO.to_le_bytes());
    assert_eq!(bytes[14..16],(CAP-1).to_le_bytes());
    assert_eq!(bytes[16..20],3u32.to_le_bytes());
    assert_eq!(bytes[20..24],[0;4]);
    assert_eq!(bytes[24..32],11u64.to_le_bytes());
    assert_eq!(bytes[32..40],12u64.to_le_bytes());
    assert_eq!(bytes[40..48],7u64.to_le_bytes());
    assert_eq!(bytes[48..64],[0;16]);
    assert_eq!(bytes[64..],[9,0,0,0,0,0,0,0]);
}
