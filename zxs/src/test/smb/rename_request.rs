//! Original bytes only. No authentication, transfer or synthetic OPEN.
use super::*;
use crate::smb::wire::{
    credit_fields::Cost,direct::Frame,io_fields::WireOpenId,message::{Compound,RELATED},
};

fn request(name:&str,replace:u8)->Vec<u8>{
    let utf:Vec<u8>=name.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut p=vec![0;116];
    p[..8].copy_from_slice(&[0xfe,b'S',b'M',b'B',64,0,1,0]);
    p[12..14].copy_from_slice(&SET_INFO.to_le_bytes());
    p[64]=33;p[66]=1;p[67]=10;p[72]=96;p[80]=1;p[88]=2;
    p[96]=replace;p[97..104].fill(0x88);
    p[112..116].copy_from_slice(&(utf.len() as u32).to_le_bytes());
    p.extend_from_slice(&utf);p.resize(p.len().max(120),0x88);
    let input=(p.len()-96) as u32;p[68..72].copy_from_slice(&input.to_le_bytes());
    p
}
fn with<T>(raw:&[u8],f:impl FnOnce(&Compound<'_>)->T)->T{
    let mut tcp=(raw.len() as u32).to_be_bytes().to_vec();tcp.extend_from_slice(raw);
    let (record,tail)=Frame::split(&tcp).unwrap().unwrap();assert!(tail.is_empty());
    let compound=Compound::parse(record).unwrap();f(&compound)
}
fn data(raw:&[u8],max:u32)->Result<Fields,RenameError>{with(raw,|c|decode(&c.messages()[0],max))}

#[test]
fn basic_rename_keeps_exact_spelling_and_the_requested_not_granted_flag(){
    for (path,flag) in [(r"F\Reports\a.xlsx",0),(r"\F\Reports\a.xlsx",1),(r"F\Reports\a.xlsx",255)]{
        let raw=request(path,flag);
        let checked=data(&raw,(raw.len()-96) as u32).unwrap();
        assert_eq!(checked.destination.components().collect::<Vec<_>>(),["F","Reports","a.xlsx"]);
        assert_eq!(checked.requested_replace,flag!=0);
        assert_eq!(checked.had_root_separator,path.starts_with('\\'));
        assert_eq!(checked.open,OpenReference::Explicit(WireOpenId{persistent:1,volatile:2}));
        with(&raw,|c|assert!(matches!(c.messages()[0].credit_cost(),Cost::Bytes(n) if n as usize==raw.len()-96)));
    }
}
#[test]
fn a_leading_tree_separator_is_evidence_not_an_ordinary_basename(){
    let rooted=data(&request(r"\x.txt",0),65_536).unwrap();
    let relative=data(&request("x.txt",0),65_536).unwrap();
    assert_eq!(rooted.destination.components().next(),relative.destination.components().next());
    assert!(rooted.had_root_separator);
    assert!(!relative.had_root_separator);
}
#[test]
fn basic_rename_refuses_escape_and_never_uses_the_next_member_or_raw_id(){
    for path in [r"\\host\share\x",r"F\..\x",r"F/x",r"F\file::$DATA","F\0x"]{
        assert_eq!(data(&request(path,0),65_536).err(),Some(RenameError::Name(NameError::InvalidName)));
    }
    let raw=request(r"F\a.xlsx",1);
    assert_eq!(data(&raw,1).err(),Some(RenameError::Fields(Error::ExceedsMaxTransact)));
    let mut short=raw.clone();short[68..72].copy_from_slice(&23u32.to_le_bytes());
    assert_eq!(data(&short,65_536).err(),Some(RenameError::Fields(Error::InformationLength)));
    short[68..72].fill(0);
    assert_eq!(data(&short,65_536).err(),Some(RenameError::Fields(Error::Malformed)));
    let mut other=raw.clone();other[104]=1;
    assert_eq!(data(&other,65_536).err(),Some(RenameError::Fields(Error::Malformed)));
    other[104]=0;other[67]=65;
    assert_eq!(data(&other,65_536).err(),Some(RenameError::Fields(Error::NotInProfile)));

    let mut compound=raw.clone();let len=(raw.len()+7)&!7;compound.resize(len,0);
    compound[20..24].copy_from_slice(&(len as u32).to_le_bytes());
    compound[72..74].copy_from_slice(&(len as u16).to_le_bytes());
    let mut next=raw;next[16..20].copy_from_slice(&RELATED.to_le_bytes());compound.extend_from_slice(&next);
    with(&compound,|c|{
        assert_eq!(decode(&c.messages()[0],65_536).err(),Some(RenameError::Fields(Error::Malformed)));
        assert_eq!(decode(&c.messages()[1],65_536).unwrap().open,OpenReference::PreviousInCompound);
    });
}
