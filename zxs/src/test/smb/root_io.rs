//! Fixed synthetic NTLM exchange and original signed send, no file powers
//! or fake Core. Positives require a launcher-issued original root.
use super::*;
use crate::smb::connection::{
    sessions::{challenge::tests::provider,finish::tests::publish_at,ordinary::Admission,tests::connected},
    setup::tests::{frame,parsed},transport::DirectStream,
};
use std::{io::{Read,Write},sync::Mutex};
use cmac::{KeyInit,Mac};
use crate::smb::wire::fixed_reply::{LOGOFF,TREE_DISCONNECT};
use crate::smb::wire::io_reply::{FLUSH,WRITE};
use crate::smb::wire::set_info::SET_INFO;
use crate::smb::wire::read_reply::READ;
use super::super::root_attributes::QUERY_INFO;

fn signed(packet:&mut [u8]){
    // Existing public raw-profile synthetic answer in final_keys.rs.
    let key=[0x90,0xda,0x95,0x77,0x49,0x4a,0x46,0x81,0x3d,0x02,0x54,0xb5,0xed,0x28,0x88,0x7f];
    packet[48..64].fill(0);
    let mut mac=cmac::Cmac::<aes::Aes128>::new_from_slice(&key).unwrap();mac.update(packet);
    packet[48..64].copy_from_slice(mac.finalize().as_bytes());
}

struct Sink(Arc<Mutex<Vec<u8>>>);
impl Read for Sink {
    fn read(&mut self,_:&mut [u8])->io::Result<usize>{Err(io::ErrorKind::Unsupported.into())}
}
impl Write for Sink {
    fn write(&mut self,p:&[u8])->io::Result<usize>{
        let n=p.len().min(11);self.0.lock().unwrap().extend_from_slice(&p[..n]);Ok(n)
    }
    fn flush(&mut self)->io::Result<()>{panic!("not storage FLUSH")}
}

fn input(cmd:u16,absent:bool,short:bool)->Vec<u8>{
    let mut p=vec![0;if cmd==CREATE{121}else if matches!(cmd,READ|WRITE){113}else if cmd==14{98}else if cmd==QUERY_INFO{105}else if cmd==SET_INFO{104}else if short{80}else{88}];
    p[..8].copy_from_slice(&[0xfe,b'S',b'M',b'B',64,0,1,0]);
    p[12..14].copy_from_slice(&cmd.to_le_bytes());p[14]=2;p[16]=8;p[24]=3;p[36]=7;p[40]=1;
    if cmd==CREATE {
        p[64]=57;p[68]=2;p[88]=1;p[96]=1;p[100]=1;p[104]=1;p[108]=120;
    } else if cmd==READ {
        p[64]=49;p[68..72].copy_from_slice(&(if short {65_537u32}else{5}).to_le_bytes());
        p[80]=3;p[88]=4;p[96]=5;
    } else if cmd==WRITE {
        p[64]=49;p[66]=112;p[68..72].copy_from_slice(&(if short {65_537u32}else{1}).to_le_bytes());
        p[80]=3;p[88]=4;p[112]=0x51;
    } else if cmd==SET_INFO {
        p[64]=33;p[66]=1;p[67]=20;p[72]=96;
        p[68..72].copy_from_slice(&(if short{65_537u32}else{8}).to_le_bytes());
        p[80]=3;p[88]=4;p[96..104].copy_from_slice(&1_048_576u64.to_le_bytes());
    } else if cmd==14 {
        p[64]=33;p[66]=12;p[72]=1;p[80]=1;
        p[88]=96;p[90]=2;p[92..96].copy_from_slice(&(if short{65_537u32}else{4096}).to_le_bytes());p[96]=b'*';
    } else if cmd==QUERY_INFO {
        p[64]=41;p[66]=2;p[67]=5;
        p[68..72].copy_from_slice(&(if short{65_537u32}else{4096}).to_le_bytes());
        p[88]=1;p[96]=1;
    } else {
        p[64]=24;
        if !short {
            if absent{p[72..88].fill(0xff);}else{p[72]=1;p[80]=1;}
        }
    }
    signed(&mut p);p
}

#[test]
fn exact_logoff_retires_its_own_session_and_never_signs_success_for_a_missing_tree_or_bad_request(){
    for (cmd,size,status) in [(LOGOFF,4,0u32),(LOGOFF,5,0xc000_000d),(TREE_DISCONNECT,4,0xc000_00c9)]{
        let instance=provider();let mut peer=connected(&instance);
        assert_eq!(publish_at(&mut peer,1),1);assert_eq!(publish_at(&mut peer,3),2);
        let seen=Arc::new(Mutex::new(Vec::new()));peer.stream=Some(DirectStream::Fixture(Box::new(Sink(Arc::clone(&seen)))));
        let mut request=vec![0;68];request[..8].copy_from_slice(&[0xfe,b'S',b'M',b'B',64,0,1,0]);
        request[12..14].copy_from_slice(&cmd.to_le_bytes());request[14]=2;request[16]=8;
        request[24]=5;request[36]=7;request[40]=1;request[64..68].copy_from_slice(&[size,0,0xa5,0xaa]);
        signed(&mut request);
        let bytes=frame(&request);let compound=parsed(&bytes);let paid=peer.receive(&compound).unwrap();
        let Admission::Ready(mut batch)=paid.preflight(&mut peer)else{panic!("original signed exit required");};
        assert!(batch.try_send_exit().unwrap());drop(batch);
        let mut expected=request[..64].to_vec();expected[8..12].copy_from_slice(&status.to_le_bytes());expected[16]=9;
        if cmd==LOGOFF{expected[36..40].fill(0);}
        if status==0 {expected.extend_from_slice(&[4,0,0,0]);}else{expected.extend_from_slice(&[9,0,0,0,0,0,0,0]);}
        signed(&mut expected);assert_eq!(&*seen.lock().unwrap(),&frame(&expected));
        assert!(peer.valid_slot(2u64.try_into().unwrap()).is_some());
        assert_eq!(peer.valid_slot(1u64.try_into().unwrap()).is_none(),status==0);
        assert_eq!(instance.run.index.lock().unwrap().live.len(),if status==0{1}else{2});
        assert_eq!(peer.in_flight,0);assert_eq!(peer.window.available(),5);
    }
}

#[test]
fn a_signed_directory_claim_cannot_bypass_its_missing_tree_even_with_paid_credit_or_spent_short_charge(){
    for short in [false,true]{
        let instance=provider();let mut peer=connected(&instance);assert_eq!(publish_at(&mut peer,1),1);
        let seen=Arc::new(Mutex::new(Vec::new()));peer.stream=Some(DirectStream::Fixture(Box::new(Sink(Arc::clone(&seen)))));
        let request=input(14,false,short);let bytes=frame(&request);let compound=parsed(&bytes);
        let paid=peer.receive(&compound).unwrap();
        assert_eq!(paid.payload(0),Some(if short{Payload::InvalidParameter}else{Payload::Covered}));
        let Admission::Ready(mut batch)=paid.preflight(&mut peer)else{panic!("original signed request required");};
        assert!(!batch.try_send_open_close().unwrap());assert!(batch.try_send_root_query().unwrap());drop(batch);
        let mut expected=request[..64].to_vec();expected[8..12].copy_from_slice(&0xc000_00c9u32.to_le_bytes());
        expected[16]=9;expected.extend_from_slice(&[9,0,0,0,0,0,0,0]);signed(&mut expected);
        assert_eq!(&*seen.lock().unwrap(),&frame(&expected));
        assert_eq!(peer.in_flight,0);assert_eq!(peer.window.available(),5);
    }
}

#[test]
fn signed_read_without_its_actual_tree_never_reads_or_returns_eof(){
    for short in [false,true]{
        let instance=provider();let mut peer=connected(&instance);assert_eq!(publish_at(&mut peer,1),1);
        let seen=Arc::new(Mutex::new(Vec::new()));peer.stream=Some(DirectStream::Fixture(Box::new(Sink(Arc::clone(&seen)))));
        let request=input(READ,false,short);let bytes=frame(&request);let fr=parsed(&bytes);
        let paid=peer.receive(&fr).unwrap();
        assert_eq!(paid.payload(0),Some(if short{Payload::InvalidParameter}else{Payload::Covered}));
        let Admission::Ready(mut batch)=paid.preflight(&mut peer)else{panic!("must verify actual frame");};
        assert!(batch.try_send_file_read().unwrap());drop(batch);
        let mut refusal=request[..64].to_vec();refusal[8..12].copy_from_slice(&0xc000_00c9u32.to_le_bytes());
        refusal[16]=9;refusal.extend_from_slice(&[9,0,0,0,0,0,0,0]);signed(&mut refusal);
        assert_eq!(&*seen.lock().unwrap(),&frame(&refusal));
        assert_eq!(peer.in_flight,0);assert_eq!(peer.window.available(),5);
    }
}

#[test]
fn a_paid_signed_file_mutation_cannot_bypass_the_original_tree_with_wire_ids(){
    for (command,short,payload) in [(WRITE,false,Payload::Covered),(WRITE,true,Payload::InvalidParameter),
        (FLUSH,false,Payload::NoVariableRule),(SET_INFO,false,Payload::Covered),(SET_INFO,true,Payload::InvalidParameter)]{
        let instance=provider();let mut peer=connected(&instance);assert_eq!(publish_at(&mut peer,1),1);
        let seen=Arc::new(Mutex::new(Vec::new()));
        peer.stream=Some(DirectStream::Fixture(Box::new(Sink(Arc::clone(&seen)))));
        let request=input(command,false,short);let bytes=frame(&request);let fr=parsed(&bytes);
        let paid=peer.receive(&fr).unwrap();assert_eq!(paid.payload(0),Some(payload));
        let Admission::Ready(mut batch)=paid.preflight(&mut peer)else{panic!("must verify actual frame");};
        assert!(batch.try_send_file_write().unwrap());drop(batch);
        let mut refusal=request[..64].to_vec();refusal[8..12].copy_from_slice(&0xc000_00c9u32.to_le_bytes());
        refusal[16]=9;refusal.extend_from_slice(&[9,0,0,0,0,0,0,0]);signed(&mut refusal);
        assert_eq!(&*seen.lock().unwrap(),&frame(&refusal));
        assert_eq!(peer.in_flight,0);assert_eq!(peer.window.available(),5);
        assert!(peer.open_table(NonZeroU64::new(1).unwrap()).unwrap().find(WireOpenId{persistent:3,volatile:4}).is_none());
    }
}

#[test]
fn metadata_query_cannot_substitute_a_file_id_or_a_short_charge_for_the_original_tree(){
    for (kind,class,short) in [(2,5,false),(2,5,true),(2,3,false),(2,3,true),(1,5,false),(1,5,true),
                              (1,6,false),(1,6,true),(1,48,false),(1,48,true)]{
        let instance=provider();let mut peer=connected(&instance);assert_eq!(publish_at(&mut peer,1),1);
        let seen=Arc::new(Mutex::new(Vec::new()));peer.stream=Some(DirectStream::Fixture(Box::new(Sink(Arc::clone(&seen)))));
        let mut request=input(QUERY_INFO,false,short);
        request[66]=kind;request[67]=class;signed(&mut request);
        let incoming=frame(&request);let fr=parsed(&incoming);
        let paid=peer.receive(&fr).unwrap();
        assert_eq!(paid.payload(0),Some(if short{Payload::InvalidParameter}else{Payload::Covered}));
        let Admission::Ready(mut batch)=paid.preflight(&mut peer)else{panic!("must verify actual frame");};
        assert!(batch.try_send_open_info().unwrap());drop(batch);
        let mut refusal=request[..64].to_vec();refusal[8..12].copy_from_slice(&0xc000_00c9u32.to_le_bytes());
        refusal[16]=9;refusal.extend_from_slice(&[9,0,0,0,0,0,0,0]);signed(&mut refusal);
        assert_eq!(&*seen.lock().unwrap(),&frame(&refusal));
        assert_eq!(peer.in_flight,0);assert_eq!(peer.window.available(),5);
    }
}

fn related_probe(class:u8,short:bool)->[Vec<u8>;3]{
    let mut parts=[input(CREATE,false,false),input(QUERY_INFO,false,short),input(CLOSE,true,false)];
    parts[0].resize(128,0);parts[1].resize(112,0);parts[1][67]=class;
    for (i,raw) in parts.iter_mut().enumerate(){
        raw[24]=3+i as u8;
        if i<2 {let n=raw.len() as u32;raw[20..24].copy_from_slice(&n.to_le_bytes());}
        if i>0 {
            raw[16]|=RELATED as u8;
            // These literal foreign addresses and non-sentinel FileIds
            // cannot be the origin of an OPEN in a related compound.
            raw[36..40].fill(0xff);raw[40..48].fill(0xff);
            let file=if i==1{88}else{72};raw[file..file+16].fill(0x32);
        }
        signed(raw);
    }
    parts
}

#[test]
fn related_root_probe_requires_actual_adjacent_open_and_preserves_all_response_members(){
    for class in [5,3]{
        let instance=provider();let mut peer=connected(&instance);assert_eq!(publish_at(&mut peer,1),1);
        assert_eq!(peer.window.available(),4);
        let seen=Arc::new(Mutex::new(Vec::new()));peer.stream=Some(DirectStream::Fixture(Box::new(Sink(Arc::clone(&seen)))));
        let requests=related_probe(class,false);let incoming=frame(&requests.concat());let fr=parsed(&incoming);
        let paid=peer.receive(&fr).unwrap();
        let Admission::Ready(mut batch)=paid.preflight(&mut peer)else{panic!("whole original compound did not verify");};
        assert!(batch.try_send_root_probe().unwrap());drop(batch);
        let mut expected=Vec::new();
        for (i,request) in requests.iter().enumerate(){
            let status=if i==0{0xc000_00c9u32}else{0xc000_0008u32};
            let mut part=request[..64].to_vec();part[8..12].copy_from_slice(&status.to_le_bytes());part[16]|=1;
            part[20..24].copy_from_slice(&(if i<2{72u32}else{0}).to_le_bytes());
            part[36..40].copy_from_slice(&7u32.to_le_bytes());part[40..48].copy_from_slice(&1u64.to_le_bytes());
            part.extend_from_slice(&[9,0,0,0,0,0,0,0]);signed(&mut part);expected.extend_from_slice(&part);
        }
        assert_eq!(&*seen.lock().unwrap(),&frame(&expected));
        assert_eq!(peer.in_flight,0);assert_eq!(peer.window.available(),7);
    }
}

#[test]
fn related_probe_is_not_dispatched_on_a_short_query_or_bad_last_mac(){
    for class in [5,3]{
        for bad_mac in [false,true]{
            let instance=provider();let mut peer=connected(&instance);assert_eq!(publish_at(&mut peer,1),1);
            let seen=Arc::new(Mutex::new(Vec::new()));peer.stream=Some(DirectStream::Fixture(Box::new(Sink(Arc::clone(&seen)))));
            let mut requests=related_probe(class,!bad_mac);
            if bad_mac{requests[2][48]^=1;}
            let incoming=frame(&requests.concat());let fr=parsed(&incoming);let paid=peer.receive(&fr).unwrap();
            match paid.preflight(&mut peer){
                Admission::Refused(_original) if bad_mac=>{},
                Admission::Ready(mut batch) if !bad_mac=>assert!(!batch.try_send_root_probe().unwrap()),
                _=>panic!("unexpected authorization for a rejected original frame"),
            }
            assert!(seen.lock().unwrap().is_empty());
        }
    }
}

#[test]
fn ordinary_directory_type_and_original_media_failures_do_not_get_the_same_status(){
    for (why,code) in [
        (DirectoryOpenError::FileIsDirectory,0xc000_00bau32),
        (DirectoryOpenError::NotADirectory,0xc000_0103),
        (DirectoryOpenError::Original(io::ErrorKind::NotConnected.into()),0xc000_026e),
        (DirectoryOpenError::Original(io::ErrorKind::WouldBlock.into()),0xc000_0467),
        (DirectoryOpenError::Original(io::ErrorKind::NotFound.into()),0xc000_0034),
    ]{
        assert_eq!(directory_open_failure(why).unwrap() as u32,code);
    }
}

#[test]
fn an_authenticated_request_without_the_original_tree_or_handle_gets_only_a_signed_failure(){
    for (cmd,absent,short,error) in [
        (CREATE,false,false,RootFailure::NetworkNameDeleted),
        (CLOSE,false,false,RootFailure::FileClosed),
        (CLOSE,true,false,RootFailure::FileClosed),
        (CLOSE,false,true,RootFailure::InvalidParameter),
    ]{
        let instance=provider();let mut peer=connected(&instance);assert_eq!(publish_at(&mut peer,1),1);
        let seen=Arc::new(Mutex::new(Vec::new()));peer.stream=Some(DirectStream::Fixture(Box::new(Sink(Arc::clone(&seen)))));
        let request=input(cmd,absent,short);let bytes=frame(&request);let compound=parsed(&bytes);
        let paid=peer.receive(&compound).unwrap();
        let Admission::Ready(mut batch)=paid.preflight(&mut peer)else{panic!("original signed request required");};
        assert!(batch.try_send_open_close().unwrap());drop(batch);
        let mut expected=request[..64].to_vec();expected[8..12].copy_from_slice(&(error as u32).to_le_bytes());
        expected[16]=9;expected.extend_from_slice(&[9,0,0,0,0,0,0,0]);signed(&mut expected);
        assert_eq!(&*seen.lock().unwrap(),&frame(&expected));
        assert_eq!(peer.in_flight,0);assert_eq!(peer.window.available(),5);
        let id=NonZeroU64::new(1).unwrap();
        assert!(peer.open_table(id).unwrap().find(WireOpenId{persistent:1,volatile:1}).is_none());
    }
}

#[test]
fn a_signed_named_open_alone_cannot_publish_a_retained_slot(){
    let instance=provider();let mut peer=connected(&instance);assert_eq!(publish_at(&mut peer,1),1);
    let seen=Arc::new(Mutex::new(Vec::new()));peer.stream=Some(DirectStream::Fixture(Box::new(Sink(Arc::clone(&seen)))));
    let mut request=input(CREATE,false,false);request.truncate(120);request[104]=0x40;
    let name:Vec<u8>=r"R\0123456789abcdef0123456789abcdef\draft-abcd.xlsx".encode_utf16().flat_map(u16::to_le_bytes).collect();
    request[110..112].copy_from_slice(&(name.len() as u16).to_le_bytes());
    request.extend_from_slice(&name);signed(&mut request);
    let bytes=frame(&request);let compound=parsed(&bytes);
    assert!(compound.messages()[0].create_name_fields().is_ok_and(|r|r.name==name.as_slice()));
    let paid=peer.receive(&compound).unwrap();
    let Admission::Ready(mut batch)=paid.preflight(&mut peer)else{panic!("must verify original signed CREATE");};
    assert!(batch.try_send_open_close().unwrap());drop(batch);
    let mut denied=request[..64].to_vec();
    denied[8..12].copy_from_slice(&(RootFailure::NetworkNameDeleted as u32).to_le_bytes());
    denied[16]=9;denied.extend_from_slice(&[9,0,0,0,0,0,0,0]);signed(&mut denied);
    assert_eq!(&*seen.lock().unwrap(),&frame(&denied));
    assert!(peer.open_table(NonZeroU64::new(1).unwrap()).unwrap().find(WireOpenId{persistent:1,volatile:1}).is_none());
    assert_eq!(peer.in_flight,0);
}

#[test]
fn paid_named_create_or_open_if_without_its_actual_tree_never_reserves_or_creates(){
    for (operation,kind,disk) in [(2u8,0x40,false),(3,0x40,false),(2,0x40,true),(3,0x40,true),
                                  (2,0x1,false),(3,0x1,false)]{
        let instance=provider();let mut peer=connected(&instance);assert_eq!(publish_at(&mut peer,1),1);
        let seen=Arc::new(Mutex::new(Vec::new()));peer.stream=Some(DirectStream::Fixture(Box::new(Sink(Arc::clone(&seen)))));
        let mut request=input(CREATE,false,false);request.truncate(120);
        request[88]=if kind==1{0x81}else{2};request[100]=operation;request[104]=kind;
        let name:Vec<u8>=r"F\Plans\new.xlsx".encode_utf16().flat_map(u16::to_le_bytes).collect();
        request[110..112].copy_from_slice(&(name.len() as u16).to_le_bytes());request.extend_from_slice(&name);
        if disk{
            let at=(request.len()+7)&!7;request.resize(at+24,0);
            request[112..116].copy_from_slice(&(at as u32).to_le_bytes());
            request[116..120].copy_from_slice(&24u32.to_le_bytes());
            request[at+4]=16;request[at+6]=4;request[at+16..at+20].copy_from_slice(b"QFid");
        }
        signed(&mut request);
        let bytes=frame(&request);let compound=parsed(&bytes);let paid=peer.receive(&compound).unwrap();
        assert_eq!(paid.payload(0),Some(Payload::NoVariableRule));
        let Admission::Ready(mut batch)=paid.preflight(&mut peer)else{panic!("must verify original CREATE");};
        assert!(batch.try_send_open_close().unwrap());drop(batch);
        let mut denied=request[..64].to_vec();
        denied[8..12].copy_from_slice(&0xc000_00c9u32.to_le_bytes());denied[16]=9;
        denied.extend_from_slice(&[9,0,0,0,0,0,0,0]);signed(&mut denied);
        assert_eq!(&*seen.lock().unwrap(),&frame(&denied));
        assert!(peer.open_table(NonZeroU64::new(1).unwrap()).unwrap().find(WireOpenId{persistent:1,volatile:1}).is_none());
        assert_eq!(peer.in_flight,0);
    }
    assert!(matches!(file_open_failure(FileOpenError::Original(io::ErrorKind::AlreadyExists.into())),
        Ok(RootFailure::ObjectNameCollision)));
    assert!(matches!(file_open_failure(FileOpenError::Original(io::ErrorKind::ConnectionAborted.into())),
        Err(Fault::WriteOutcomeUnknown)));
    assert!(matches!(directory_open_failure(DirectoryOpenError::Original(io::ErrorKind::AlreadyExists.into())),
        Ok(RootFailure::ObjectNameCollision)));
    assert!(matches!(directory_open_failure(DirectoryOpenError::Original(io::ErrorKind::ConnectionAborted.into())),
        Err(Fault::WriteOutcomeUnknown)));
}

#[test]
fn started_close_never_signs_a_definite_reply_for_an_inconclusive_save(){
    let unknown=OpenCloseError::Writer(FileCloseError::Original(io::ErrorKind::ConnectionAborted.into()));
    assert!(matches!(close_failure(unknown),Err(Fault::WriteOutcomeUnknown)));
    let delete_unknown=OpenCloseError::File(FileCloseError::Original(io::ErrorKind::ConnectionAborted.into()));
    assert!(matches!(close_failure(delete_unknown),Err(Fault::WriteOutcomeUnknown)));
    let full=OpenCloseError::Writer(FileCloseError::Original(io::ErrorKind::StorageFull.into()));
    assert!(matches!(close_failure(full),Ok(RootFailure::DiskFull)));
}

#[test]
fn mark_or_clear_without_an_original_open_never_acknowledges_disposition(){
    for (mark,short) in [(false,false),(true,false),(true,true)]{
        let instance=provider();let mut peer=connected(&instance);assert_eq!(publish_at(&mut peer,1),1);
        let seen=Arc::new(Mutex::new(Vec::new()));peer.stream=Some(DirectStream::Fixture(Box::new(Sink(Arc::clone(&seen)))));
        let mut request=input(SET_INFO,false,short);request.truncate(97);
        request[67]=13;request[96]=u8::from(mark);
        request[68..72].copy_from_slice(&(if short{65_537u32}else{1}).to_le_bytes());signed(&mut request);
        let incoming=frame(&request);let original=parsed(&incoming);let paid=peer.receive(&original).unwrap();
        assert_eq!(paid.payload(0),Some(if short{Payload::InvalidParameter}else{Payload::Covered}));
        let Admission::Ready(mut batch)=paid.preflight(&mut peer)else{panic!("must verify original mark or clear");};
        assert!(batch.try_send_file_write().unwrap());drop(batch);
        let mut refusal=request[..64].to_vec();refusal[8..12].copy_from_slice(&0xc000_00c9u32.to_le_bytes());
        refusal[16]=9;refusal.extend_from_slice(&[9,0,0,0,0,0,0,0]);signed(&mut refusal);
        assert_eq!(&*seen.lock().unwrap(),&frame(&refusal));
        assert!(peer.open_table(NonZeroU64::new(1).unwrap()).unwrap().find(WireOpenId{persistent:3,volatile:4}).is_none());
        assert_eq!(peer.in_flight,0);assert_eq!(peer.window.available(),5);
    }
}

#[test]
fn basic_rename_without_original_tree_never_moves_anything_even_if_paid(){
    // The spelling does not select a FILE vs. directory OPEN. Both
    // fully paid and short-charge forms need the same original table.
    for (destination,replace,short) in [
        (r"F\Plans\a.xlsx",true,false),("a-new.xlsx",true,false),("a-new.xlsx",true,true),
        (r"F\Plans\Archive",true,false),("Archive",true,false),(r"F\Plans\Archive",true,true),
        (r"F\Archive\2026\moved.xlsx",false,false),(r"F\Archive\2026\moved.xlsx",false,true),
    ]{
        let instance=provider();let mut peer=connected(&instance);assert_eq!(publish_at(&mut peer,1),1);
        let seen=Arc::new(Mutex::new(Vec::new()));
        peer.stream=Some(DirectStream::Fixture(Box::new(Sink(Arc::clone(&seen)))));
        let name:Vec<u8>=destination.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut request=input(SET_INFO,false,short);
        request.truncate(96);request.resize(116,0);
        request[67]=10;request[96]=u8::from(replace);
        request[112..116].copy_from_slice(&(name.len() as u32).to_le_bytes());
        request.extend_from_slice(&name);
        let input=if short{65_537}else{(request.len()-96) as u32};
        request[68..72].copy_from_slice(&input.to_le_bytes());signed(&mut request);
        let incoming=frame(&request);let fr=parsed(&incoming);let paid=peer.receive(&fr).unwrap();
        assert_eq!(paid.payload(0),Some(if short{Payload::InvalidParameter}else{Payload::Covered}));
        let Admission::Ready(mut batch)=paid.preflight(&mut peer)else{panic!("must verify actual frame");};
        assert!(!batch.try_send_related_rename().unwrap());
        assert!(batch.try_send_named_rename().unwrap());drop(batch);
        let mut refusal=request[..64].to_vec();refusal[8..12].copy_from_slice(&0xc000_00c9u32.to_le_bytes());
        refusal[16]=9;refusal.extend_from_slice(&[9,0,0,0,0,0,0,0]);signed(&mut refusal);
        assert_eq!(&*seen.lock().unwrap(),&frame(&refusal));
        assert!(peer.open_table(NonZeroU64::new(1).unwrap()).unwrap().find(WireOpenId{persistent:3,volatile:4}).is_none());
        assert_eq!(peer.in_flight,0);assert_eq!(peer.window.available(),5);
    }
}

fn related_file_rename(retained:bool,short:bool)->[Vec<u8>;3]{
    let mut start=input(CREATE,false,false);start.truncate(120);
    start[88..92].copy_from_slice(&0x0011_0080u32.to_le_bytes());start[96]=7;
    start[104]=if retained{0x40}else{0}; // Apple's ordinary VREG can omit NON_DIRECTORY
    let from=if retained{r"R\0123456789abcdef0123456789abcdef\a.xlsx"}else{r"F\Plans\a.xlsx"};
    let old:Vec<u8>=from.encode_utf16().flat_map(u16::to_le_bytes).collect();
    start[110..112].copy_from_slice(&(old.len() as u16).to_le_bytes());start.extend_from_slice(&old);
    let at=(start.len()+7)&!7;start.resize(at+24,0);
    start[112..116].copy_from_slice(&(at as u32).to_le_bytes());start[116..120].copy_from_slice(&24u32.to_le_bytes());
    start[at+4]=16;start[at+6]=4;start[at+16..at+20].copy_from_slice(b"MxAc");

    let mut set=input(SET_INFO,false,false);set.truncate(96);set.resize(116,0);
    let target=if retained{r"F\Plans\a.xlsx"}else{r"F\Plans\merged.xlsx"};
    let new:Vec<u8>=target.encode_utf16().flat_map(u16::to_le_bytes).collect();
    set[67]=10;set[112..116].copy_from_slice(&(new.len() as u32).to_le_bytes());set.extend_from_slice(&new);
    set.extend_from_slice(&[0;4]); // Apple adds this in BufferLength, not FileNameLength.
    let length=if short{65_537}else{(set.len()-96) as u32};
    set[68..72].copy_from_slice(&length.to_le_bytes());set.resize((set.len()+7)&!7,0);
    let mut parts=[start,set,input(CLOSE,false,false)];
    for (i,raw) in parts.iter_mut().enumerate(){
        raw[24]=3+i as u8;
        if i<2 {let n=raw.len() as u32;raw[20..24].copy_from_slice(&n.to_le_bytes());}
        if i>0{raw[16]|=RELATED as u8;raw[36..48].fill(0xff);}
        signed(raw);
    }
    parts
}

#[test]
fn signed_related_rename_without_real_create_produces_no_ordinary_or_retained_receipt(){
    for (retained,short,bad_mac) in [
        (true,false,false),(true,true,false),(true,false,true),
        (false,false,false),(false,true,false),(false,false,true),
    ]{
        let instance=provider();let mut peer=connected(&instance);assert_eq!(publish_at(&mut peer,1),1);
        let seen=Arc::new(Mutex::new(Vec::new()));
        peer.stream=Some(DirectStream::Fixture(Box::new(Sink(Arc::clone(&seen)))));
        let mut requests=related_file_rename(retained,short);if bad_mac{requests[2][48]^=1;}
        let framed=frame(&requests.concat());let original=parsed(&framed);let paid=peer.receive(&original).unwrap();
        match paid.preflight(&mut peer){
            Admission::Refused(_) if bad_mac=>{},
            Admission::Ready(mut batch) if !bad_mac=>{
                // The standalone sender must not bypass the compound's
                // adjacent original FILE/restore routing rules.
                assert!(!batch.try_send_named_rename().unwrap());
                assert!(seen.lock().unwrap().is_empty());
                assert_eq!(batch.try_send_related_rename().unwrap(),!short);drop(batch);
            },
            _=>panic!("incorrect original compound admission"),
        }
        if short||bad_mac{assert!(seen.lock().unwrap().is_empty());continue;}
        let mut expected=Vec::new();
        for (i,request) in requests.iter().enumerate(){
            let status=if i==0{0xc000_00c9u32}else{0xc000_0008u32};
            let mut bytes=request[..64].to_vec();bytes[8..12].copy_from_slice(&status.to_le_bytes());bytes[16]|=1;
            bytes[20..24].copy_from_slice(&(if i<2{72u32}else{0}).to_le_bytes());
            bytes[36..40].copy_from_slice(&7u32.to_le_bytes());bytes[40..48].copy_from_slice(&1u64.to_le_bytes());
            bytes.extend_from_slice(&[9,0,0,0,0,0,0,0]);signed(&mut bytes);expected.extend_from_slice(&bytes);
        }
        assert_eq!(&*seen.lock().unwrap(),&frame(&expected));
        assert_eq!(peer.in_flight,0);
    }
}

fn related_ordinary_unlink(short:bool)->[Vec<u8>;3]{
    let mut start=input(CREATE,false,false);start.truncate(120);
    start[88..92].copy_from_slice(&0x0011_0080u32.to_le_bytes());start[96]=0;start[104]=0x40;
    let name:Vec<u8>=r"F\Plans\draft-abcd.xlsx".encode_utf16().flat_map(u16::to_le_bytes).collect();
    start[110..112].copy_from_slice(&(name.len() as u16).to_le_bytes());start.extend_from_slice(&name);
    let at=(start.len()+7)&!7;start.resize(at+24,0);
    start[112..116].copy_from_slice(&(at as u32).to_le_bytes());start[116..120].copy_from_slice(&24u32.to_le_bytes());
    start[at+4]=16;start[at+6]=4;start[at+16..at+20].copy_from_slice(b"MxAc");

    let mut set=input(SET_INFO,false,short);set[67]=13;set[96..].fill(0);set[96]=1;
    set[68..72].copy_from_slice(&(if short{65_537u32}else{1}).to_le_bytes());
    let mut parts=[start,set,input(CLOSE,false,false)];
    for (i,raw) in parts.iter_mut().enumerate(){
        raw[24]=3+i as u8;
        if i<2 {let n=raw.len() as u32;raw[20..24].copy_from_slice(&n.to_le_bytes());}
        if i>0{raw[16]|=RELATED as u8;raw[36..48].fill(0xff);}
        // Literal FileIds differ; related operations inherit only an
        // actual preceding OPEN, not even numerically plausible values.
        signed(raw);
    }
    parts
}

#[test]
fn paid_ordinary_unlink_with_no_original_tree_cannot_mark_or_close(){
    for (short,bad_mac) in [(false,false),(true,false),(false,true)]{
        let instance=provider();let mut peer=connected(&instance);assert_eq!(publish_at(&mut peer,1),1);
        let seen=Arc::new(Mutex::new(Vec::new()));peer.stream=Some(DirectStream::Fixture(Box::new(Sink(Arc::clone(&seen)))));
        let mut query=related_ordinary_unlink(short);if bad_mac{query[2][48]^=1;}
        let framed=frame(&query.concat());let original=parsed(&framed);let paid=peer.receive(&original).unwrap();
        match paid.preflight(&mut peer){
            Admission::Refused(_) if bad_mac=>{},
            Admission::Ready(mut batch) if !bad_mac=>{
                assert!(!batch.try_send_related_rename().unwrap());
                assert_eq!(batch.try_send_related_disposition().unwrap(),!short);drop(batch);
            },
            _=>panic!("wrong original disposition chain admission"),
        }
        if short||bad_mac{assert!(seen.lock().unwrap().is_empty());continue;}
        let mut expected=Vec::new();
        for (i,request) in query.iter().enumerate(){
            let status=if i==0{0xc000_00c9u32}else{0xc000_0008u32};
            let mut bytes=request[..64].to_vec();bytes[8..12].copy_from_slice(&status.to_le_bytes());bytes[16]|=1;
            bytes[20..24].copy_from_slice(&(if i<2{72u32}else{0}).to_le_bytes());
            bytes[36..40].copy_from_slice(&7u32.to_le_bytes());bytes[40..48].copy_from_slice(&1u64.to_le_bytes());
            bytes.extend_from_slice(&[9,0,0,0,0,0,0,0]);signed(&mut bytes);expected.extend_from_slice(&bytes);
        }
        assert_eq!(&*seen.lock().unwrap(),&frame(&expected));
        assert_eq!(peer.in_flight,0);
    }
}

#[test]
fn finder_root_probe_and_fileall_fallback_layout_never_create_a_root_without_a_real_tree(){
    for file_all in [false,true]{
        let instance=provider();let mut peer=connected(&instance);assert_eq!(publish_at(&mut peer,1),1);
        let seen=Arc::new(Mutex::new(Vec::new()));peer.stream=Some(DirectStream::Fixture(Box::new(Sink(Arc::clone(&seen)))));
        let mut open=input(CREATE,false,false);open.resize(168,0);
        open[88..92].copy_from_slice(&0x0010_0080u32.to_le_bytes());open[96]=7;
        open[112..116].copy_from_slice(&120u32.to_le_bytes());open[116..120].copy_from_slice(&48u32.to_le_bytes());
        open[120]=24;
        for (at,tag) in [(120,b"MxAc"),(144,b"QFid")]{
            open[at+4]=16;open[at+6]=4;open[at+16..at+20].copy_from_slice(tag);
        }
        let mut parts=vec![open];
        if file_all{
            let mut all=input(QUERY_INFO,false,false);all.resize(112,0);
            all[66]=1;all[67]=18;all[68..72].copy_from_slice(&256u32.to_le_bytes());parts.push(all);
        }
        parts.push(input(CLOSE,false,false));let count=parts.len();
        for (i,r) in parts.iter_mut().enumerate(){
            r[24]=3+i as u8;
            if i+1<count{let n=r.len() as u32;r[20..24].copy_from_slice(&n.to_le_bytes());}
            if i>0{r[16]|=RELATED as u8;r[36..48].fill(255);}
            signed(r);
        }
        let bytes=frame(&parts.concat());let original=parsed(&bytes);let paid=peer.receive(&original).unwrap();
        let Admission::Ready(mut batch)=paid.preflight(&mut peer)else{panic!("same original signed members");};
        assert_eq!(batch.try_send_root_probe().unwrap(),file_all);
        if !file_all{assert!(batch.try_send_root_open_close_probe().unwrap());}
        drop(batch);
        let mut expected=Vec::new();
        for (i,r) in parts.iter().enumerate(){
            let status=if i==0{0xc000_00c9u32}else{0xc000_0008u32};
            let mut part=r[..64].to_vec();part[8..12].copy_from_slice(&status.to_le_bytes());part[16]|=1;
            part[20..24].copy_from_slice(&(if i+1<count{72u32}else{0}).to_le_bytes());
            part[36..40].copy_from_slice(&7u32.to_le_bytes());part[40..48].copy_from_slice(&1u64.to_le_bytes());
            part.extend_from_slice(&[9,0,0,0,0,0,0,0]);signed(&mut part);expected.extend_from_slice(&part);
        }
        assert_eq!(&*seen.lock().unwrap(),&frame(&expected));
        assert_eq!(peer.in_flight,0);
    }
}

#[test]
fn a_related_directory_pair_without_its_real_tree_cannot_create_or_disclose_a_page(){
    for (directory,short,bad_mac) in [(true,false,false),(true,true,false),(true,false,true),(false,false,false)]{
        let instance=provider();let mut peer=connected(&instance);assert_eq!(publish_at(&mut peer,1),1);
        let seen=Arc::new(Mutex::new(Vec::new()));peer.stream=Some(DirectStream::Fixture(Box::new(Sink(Arc::clone(&seen)))));
        let mut open=input(CREATE,false,false);
        open.truncate(120);open[88..92].copy_from_slice(&0x0010_0081u32.to_le_bytes());open[96]=7;
        open[104]=u8::from(directory);
        if !directory{
            // A legal FILE_OPEN that did not request a directory does
            // not become a definite CREATE failure on this limited route.
            let name:Vec<u8>=r"F\ordinary-abcd.bin".encode_utf16().flat_map(u16::to_le_bytes).collect();
            open[110..112].copy_from_slice(&(name.len() as u16).to_le_bytes());open.extend_from_slice(&name);
        }
        open.resize((open.len().max(121)+7)&!7,0);let n=open.len() as u32;
        open[20..24].copy_from_slice(&n.to_le_bytes());signed(&mut open);
        let mut query=input(14,false,short);query.resize(104,0);query[16]|=RELATED as u8;query[24]=4;
        query[36..48].fill(255);query[72]=91;query[80]=92;signed(&mut query);
        if bad_mac{query[48]^=1;}
        let parts=[open,query];let framed=frame(&parts.concat());let original=parsed(&framed);let paid=peer.receive(&original).unwrap();
        match paid.preflight(&mut peer){
            Admission::Refused(_) if bad_mac=>{},
            Admission::Ready(mut batch) if !bad_mac=>{
                assert_eq!(batch.try_send_related_browse().unwrap(),directory&&!short);
                drop(batch);
            },
            _=>panic!("wrong original paired directory admission"),
        }
        if !directory||short||bad_mac{assert!(seen.lock().unwrap().is_empty());continue;}
        let mut expected=Vec::new();
        for (i,r) in parts.iter().enumerate(){
            let status=if i==0{0xc000_00c9u32}else{0xc000_0008u32};
            let mut part=r[..64].to_vec();part[8..12].copy_from_slice(&status.to_le_bytes());part[16]|=1;
            part[20..24].copy_from_slice(&(if i==0{72u32}else{0}).to_le_bytes());
            part[36..40].copy_from_slice(&7u32.to_le_bytes());part[40..48].copy_from_slice(&1u64.to_le_bytes());
            part.extend_from_slice(&[9,0,0,0,0,0,0,0]);signed(&mut part);expected.extend_from_slice(&part);
        }
        assert_eq!(&*seen.lock().unwrap(),&frame(&expected));
        assert_eq!(peer.in_flight,0);
    }
}
