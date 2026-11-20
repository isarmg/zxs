//! Structural CREATE fixtures. None is an authenticated session or OPEN.
use super::*;
use crate::smb::wire::{direct::Frame,message::Compound};

fn tcp(raw:&[u8])->Vec<u8> {
    let mut wire=(raw.len() as u32).to_be_bytes().to_vec();wire.extend_from_slice(raw);wire
}
fn packet(name:&[u8],disposition:u32,options:u32,extra:&[u8])->Vec<u8> {
    let mut p=vec![0;120];
    p[..4].copy_from_slice(b"\xfeSMB");p[4]=64;p[12]=5;
    p[64]=57;p[66]=0xf7; // SecurityFlags are ignored, not an identity
    p[68]=2;p[72..88].fill(0xa5); // reserved in both fields
    p[88..92].copy_from_slice(&0x0200_0081u32.to_le_bytes());
    p[92..96].copy_from_slice(&0x80u32.to_le_bytes());p[96]=7;
    p[100..104].copy_from_slice(&disposition.to_le_bytes());p[104..108].copy_from_slice(&options.to_le_bytes());
    p[108..110].copy_from_slice(&120u16.to_le_bytes());p[110..112].copy_from_slice(&(name.len() as u16).to_le_bytes());
    if name.is_empty() {p.push(0);} else {p.extend_from_slice(name);}
    if !extra.is_empty() {
        let next=p.len().next_multiple_of(8);p.resize(next,0);
        p[112..116].copy_from_slice(&(next as u32).to_le_bytes());
        p[116..120].copy_from_slice(&(extra.len() as u32).to_le_bytes());
        p.extend_from_slice(extra);
    }
    p
}
fn shape(framed:&[u8])->Result<CreateFields<'_>> {
    let (f,rest)=Frame::split(framed).unwrap().unwrap();assert!(rest.is_empty());
    Compound::parse(f).unwrap().messages()[0].create_fields()
}
fn context(name:&[u8],data:&[u8],following:bool)->Vec<u8> {
    let data_at=(16+name.len()).next_multiple_of(8);
    let mut x=vec![0;(data_at+data.len()).next_multiple_of(8)];
    let next=if following {x.len() as u32} else {0};
    x[0..4].copy_from_slice(&next.to_le_bytes());x[4..6].copy_from_slice(&16u16.to_le_bytes());
    x[6..8].copy_from_slice(&(name.len() as u16).to_le_bytes());x[8]=0xb5; // ignored Reserved
    let offset=if data.is_empty() {u16::MAX} else {data_at as u16};
    x[10..12].copy_from_slice(&offset.to_le_bytes());x[12..16].copy_from_slice(&(data.len() as u32).to_le_bytes());
    x[16..16+name.len()].copy_from_slice(name);x[data_at..data_at+data.len()].copy_from_slice(data);
    x
}
fn only_context(kind:&[u8],data:&[u8])->Vec<u8> {tcp(&packet(&[],1,DIRECTORY,&context(kind,data,false)))}

#[test]
fn real_root_shape_preserves_requested_access_but_ignores_only_defined_hints() {
    let data=tcp(&packet(&[],1,DIRECTORY|NO_COMPRESSION|IGNORED_OPTIONS,&[]));
    let x=shape(&data).unwrap();
    assert_eq!(x.disposition(),Disposition::Open);
    assert_eq!(x.desired_access(),0x0200_0081); // can include MAXIMUM_ALLOWED; never a Grant
    assert_eq!(x.file_attributes(),0x80);assert_eq!(x.share_access(),7);
    assert_eq!(x.impersonation_level(),2);assert_eq!(x.oplock(),OplockRequest::None);
    assert_eq!(x.requested_options(),DIRECTORY|NO_COMPRESSION|IGNORED_OPTIONS);
    assert_eq!(x.effective_options(),DIRECTORY);
    assert_eq!(x.current_intent().unwrap().name(),CurrentName::Root);
    for (disp,opts,kind) in [(2,DIRECTORY,SpecialOpen::RootOpenOnly),
        (3,DIRECTORY,SpecialOpen::RootOpenOnly),(1,DIRECTORY|DELETE_ON_CLOSE,SpecialOpen::RootOpenOnly),
        (1,NON_DIRECTORY,SpecialOpen::RootIsDirectory),(1,UNBUFFERED,SpecialOpen::Unbuffered)] {
        let bytes=tcp(&packet(&[],disp,opts,&[]));
        assert_eq!(shape(&bytes).unwrap().current_intent().err(),Some(kind));
    }
    // A normal relative open still belongs to path/authority validation.
    let data=tcp(&packet(&[b'A',0,b'\\',0,b'B',0],2,SEQUENTIAL|RANDOM,&[]));
    let x=shape(&data).unwrap();assert_eq!(x.effective_options(),RANDOM);
    assert_eq!(x.current_intent().unwrap().name(),CurrentName::Relative(&[b'A',0,b'\\',0,b'B',0]));
}

#[test]
fn native_probe_keeps_the_required_reply_choices_even_when_context_order_changes() {
    let mut apple=[0u8;24];apple[0]=1;apple[8]=7;apple[16]=1;
    let tick=0xfedc_ba98_7654_3210u64.to_le_bytes();
    let lease=[0u8;52];let durable=[0u8;32];
    let contexts:[(&[u8],&[u8]);5]=[(b"AAPL",&apple),(b"MxAc",&tick),(b"RqLs",&lease),
        (b"DH2Q",&durable),(b"QFid",&[])];
    for reverse in [false,true] {
        let iter=if reverse {[4,3,2,1,0]} else {[0,1,2,3,4]};
        let list:Vec<u8>=iter.into_iter().enumerate().flat_map(|(at,i)|context(contexts[i].0,contexts[i].1,at!=4)).collect();
        let mut root=packet(&[],1,DIRECTORY,&list);root[67]=0xff;
        let bytes=tcp(&root);let x=shape(&bytes).unwrap();
        assert_eq!(x.oplock(),OplockRequest::Lease);
        let parsed:Vec<_>=x.contexts().map(|c|(c.name(),c.data())).collect();
        let want:Vec<_>=iter.into_iter().map(|i|contexts[i]).collect();assert_eq!(parsed,want);
        let intent=x.current_intent().unwrap();assert_eq!(intent.name(),CurrentName::Root);
        assert!(intent.disk_id());
        assert_eq!(intent.maximal_access(),Some(MaximumAccessQuery::CompareChangeTime(0xfedc_ba98_7654_3210)));
    }
    let unconditional=only_context(b"MxAc",&[]);
    let zero_time=only_context(b"MxAc",&[0;8]);
    assert_eq!(shape(&unconditional).unwrap().current_intent().unwrap().maximal_access(),Some(MaximumAccessQuery::Unconditional));
    assert_eq!(shape(&zero_time).unwrap().current_intent().unwrap().maximal_access(),Some(MaximumAccessQuery::CompareChangeTime(0)));
}

#[test]
fn empty_path_on_reconnect_snapshot_replay_or_unknown_is_not_the_root() {
    let mut apple_id=[0u8;24];apple_id[0]=2;
    for (name,data,want) in [(b"DHnC",&[0;16][..],SpecialOpen::Reconnect),
        (b"DH2C",&[0;36],SpecialOpen::Reconnect),(b"TWrp",&[1;8],SpecialOpen::Historical),
        (b"AAPL",&apple_id,SpecialOpen::Context),(b"ZZZZ",&[],SpecialOpen::Context)] {
        let wire=only_context(name,data);
        assert_eq!(shape(&wire).unwrap().current_intent().err(),Some(want));
    }
    let mut raw=packet(&[],1,DIRECTORY,&[]);raw[19]=0x20;
    assert_eq!(shape(&tcp(&raw)).unwrap().current_intent().err(),Some(SpecialOpen::Replay));
    for first in [true,false] {
        let (a,b)=if first {((b"ZZZZ",&[][..]),(b"DHnC",&[0;16][..]))} else {
            ((b"DHnC",&[0;16][..]),(b"ZZZZ",&[][..]))};
        let mut pair=context(a.0,a.1,true);pair.extend(context(b.0,b.1,false));
        assert_eq!(shape(&tcp(&packet(&[],1,DIRECTORY,&pair))).unwrap().current_intent().err(),Some(SpecialOpen::Reconnect));
    }
}

#[test]
fn invalid_fixed_choices_and_invalid_known_contexts_never_match_plain_open() {
    for (disp,opt,want) in [
        (6,0,Error::Malformed),(4,DIRECTORY,Error::Malformed),(1,DIRECTORY|NON_DIRECTORY,Error::Malformed),
        (1,0x80,Error::Malformed),(1,RESERVE_OPFILTER,Error::Unsupported),
        (1,0x2000,Error::Field(FieldError::Unsupported)),
    ] {
        assert_eq!(shape(&tcp(&packet(&[],disp,opt,&[]))).err(),Some(want));
    }
    assert_eq!(shape(&tcp(&packet(&[1],1,0,&[]))).err(),Some(Error::Malformed));
    for (at,bad) in [(67,16),(68,4),(96,8)] {
        let mut raw=packet(&[],1,DIRECTORY,&[]);raw[at]=bad;
        assert_eq!(shape(&tcp(&raw)).err(),Some(Error::Malformed));
    }
    for (kind,body) in [(b"MxAc",&[1][..]),(b"QFid",&[0;8]),(b"RqLs",&[0;31]),(b"DH2Q",&[0;31]),
                       (b"DH2C",&[0;35]),(b"TWrp",&[])] {
        assert_eq!(shape(&only_context(kind,body)).err(),Some(Error::Malformed));
    }
    for (end,data) in [(b"MxAc",&[][..]),(b"DHnQ",&[0;16][..]),(b"DH2C",&[0;36][..])] {
        let (name,body)=if end==b"MxAc" {(b"MxAc",&[][..])} else {(b"DH2Q",&[0;32][..])};
        let mut list=context(name,body,true);list.extend(context(end,data,false));
        assert_eq!(shape(&tcp(&packet(&[],1,0,&list))).err(),Some(Error::Malformed));
    }
}

#[test]
fn reserved_context_is_ignored_but_empty_name_needs_its_own_valid_offset() {
    let reserved=0x93ad25509cb411e7b42383de968bcd7cu128.to_be_bytes();
    let request=only_context(&reserved,&[0xa5,0x99]);
    let created=shape(&request).unwrap();
    assert_eq!(created.contexts().next().unwrap().kind(),Kind::Reserved);
    assert_eq!(created.current_intent().unwrap().name(),CurrentName::Root);
    let mut near=reserved;near[15]^=1;
    assert_eq!(shape(&only_context(&near,&[])).unwrap().current_intent().err(),Some(SpecialOpen::Context));
    for offset in [0u16,64,119,121,128] {
        let mut root=packet(&[],1,DIRECTORY,&[]);
        root[108..110].copy_from_slice(&offset.to_le_bytes());
        assert_eq!(shape(&tcp(&root)).err(),Some(Error::Malformed));
    }
}

#[test]
fn offsets_and_lengths_are_confined_to_this_context_and_this_command() {
    let mut list=context(b"MxAc",&[0;8],true);list.extend(context(b"QFid",&[],false));
    for (at,bad) in [(0,33u16),(0,56),(4,17),(6,3),(10,16),(10,32),(10,17),(12,u16::MAX)] {
        let mut raw=list.clone();raw[at..at+2].copy_from_slice(&bad.to_le_bytes());
        assert_eq!(shape(&tcp(&packet(&[],1,0,&raw))).err(),Some(Error::Malformed));
    }
    let chain:Vec<_>=(0..MAX_CONTEXTS+1).flat_map(|i|context(b"ZZZZ",&[],i!=MAX_CONTEXTS)).collect();
    assert_eq!(shape(&tcp(&packet(&[],1,0,&chain))).err(),Some(Error::ContextLimit));
    let mut raw=packet(&[],1,0,&context(b"MxAc",&[0;8],false));let next=raw.len().next_multiple_of(8);
    raw.resize(next,0);raw[20..24].copy_from_slice(&(next as u32).to_le_bytes());
    raw[116..120].copy_from_slice(&34u32.to_le_bytes()); // two bytes of the following SMB2 header
    raw.extend(packet(&[],1,0,&[]));
    assert_eq!(shape(&tcp(&raw)).err(),Some(Error::Field(FieldError::Malformed)));
}
