//! Policy and allocation examples only; no fake authority or storage OPEN.
use super::*;

#[test]
fn a_read_only_file_cannot_turn_a_requested_write_or_zero_mask_into_a_grant(){
    assert!(matches!(read_grant(0,false),Err(FileOpenError::AccessDenied)));
    for blocked in [0x2,0x4,0x1_0000,0x4000_0000,MAXIMUM_ALLOWED|0x2]{
        assert!(matches!(read_grant(blocked,false),Err(FileOpenError::AccessDenied)));
    }
    assert_eq!(read_grant(EXECUTE,false).unwrap().0,EXECUTE|READ_DATA);
    assert_eq!(read_grant(READ_ATTRIBUTES,false).unwrap().0,READ_ATTRIBUTES);
    assert_eq!(read_grant(MAXIMUM_ALLOWED,false).unwrap().0,FILE_READ_RIGHTS);
    assert_eq!(read_grant(DELETE_ACCESS,true).unwrap().0,DELETE_ACCESS);
    assert_eq!(read_grant(EXECUTE|DELETE_ACCESS,true).unwrap().0,EXECUTE|READ_DATA|DELETE_ACCESS);
}
#[test]
fn ordinary_file_maximal_policy_is_not_the_permission_granted_to_the_handle(){
    let just_delete=0x0011_0080;
    assert_eq!(read_grant(just_delete,true).unwrap().0,just_delete);
    assert_eq!(file_ceiling(true),0x0011_00a7);
    assert_eq!(file_ceiling(false),0x0010_00a1);
    let advertised=file_ceiling(true);
    let granted=read_grant(just_delete,true).unwrap().0;
    assert_eq!(granted&0x7,0);
    assert_eq!(advertised&0x7,0x7);
}
#[test]
fn logical_allocation_is_not_a_disk_size_measurement(){
    for (eof,bytes) in [(0,0),(1,4096),(4096,4096),(4097,8192),(i64::MAX as u64,(i64::MAX as u64)&!4095)]{
        assert_eq!(allocation(eof),bytes);
    }
}

#[test]
fn normalized_name_admission_preserves_explicit_standalone_placement_for_both_open_kinds(){
    use crate::smb::wire::{direct::Frame,message::Compound};
    let mut request=[0;104];
    request[..4].copy_from_slice(b"\xfeSMB");request[4]=64;request[12]=16;request[16]=8;
    request[36]=9;request[40]=3;request[64]=41;
    request[66]=1;request[67]=48;request[68]=19;
    request[88]=5;request[96]=7; // both halves explicit, still not an OPEN proof
    let framed=|messages:&[&[u8]]|{
        let length=messages.iter().map(|p|p.len()).sum::<usize>() as u32;
        let mut out=length.to_be_bytes().to_vec();for p in messages{out.extend_from_slice(p);}out
    };
    fn parse(raw:&[u8])->Compound<'_>{
        let (one,tail)=Frame::split(raw).ok().unwrap().unwrap();
        assert!(tail.is_empty());Compound::parse(one).ok().unwrap()
    }
    let wire=framed(&[&request]);let first=parse(&wire);let message=&first.messages()[0];
    assert_eq!(normalized_name_output(message,0,19).unwrap(),19);
    assert!(matches!(normalized_name_output(message,0,18),Err(FileStandardError::Fields(QueryError::ExceedsMaxTransact))));
    let mut compound=request;compound[20]=104;
    let wire=framed(&[&compound,&request]);let together=parse(&wire);
    for (ordinal,m) in together.messages().iter().enumerate(){
        assert!(matches!(normalized_name_output(m,ordinal,19),Err(FileStandardError::NotSupported)));
    }
    let mut missing=request;missing[88..104].fill(0xff);
    let wire=framed(&[&missing]);let one=parse(&wire);
    assert!(normalized_name_output(&one.messages()[0],0,19).is_err());
    let mut other=request;other[67]=35;
    let wire=framed(&[&other]);let one=parse(&wire);
    assert!(matches!(normalized_name_output(&one.messages()[0],0,19),Err(FileStandardError::NotSupported)));
}
