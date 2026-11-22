//! In-memory fixtures only; no listener, installed account, named root or
//! actual interface. Do not substitute this provider for a live ZXS Tree.
use super::*;
use crate::smb::{
    connection::{sessions::{challenge::tests::provider,finish::tests::publish_at,
        ordinary::{Admission,Step},tests::connected},setup::tests::{frame,parsed},transport::DirectStream},
    session::SigningMacForSessionFixture,
};
use crate::smb::fixed_output::{KeyInit,Mac};
use std::{io::{self,Read,Write},sync::{Arc,Mutex}};

fn request(extra_zero:bool,maximum:u32)->Vec<u8>{
    let mut p=vec![0;120+usize::from(extra_zero)];
    p[..8].copy_from_slice(&[0xfe,b'S',b'M',b'B',64,0,1,0]);
    p[12]=11;p[14]=2;p[16]=8;p[24]=3;p[36]=7;p[40]=1;
    p[64]=57;
    p[68..72].copy_from_slice(&0x0014_01fcu32.to_le_bytes());
    p[72..88].fill(0xff);
    p[108..112].copy_from_slice(&maximum.to_le_bytes());
    p[112]=1;
    p
}
fn sign(p:&mut[u8]){
    // Existing public synthetic transcript, not an installed credential.
    let key=[0x90,0xda,0x95,0x77,0x49,0x4a,0x46,0x81,0x3d,0x02,0x54,0xb5,0xed,0x28,0x88,0x7f];
    p[48..64].fill(0);
    let mut mac=SigningMacForSessionFixture::new_from_slice(&key).unwrap();mac.update(p);
    p[48..64].copy_from_slice(mac.finalize().as_bytes());
}

#[test]
fn interface_probe_matches_the_signed_standalone_zero_input_shape_and_classifies_same_code_errors(){
    for extra in [false,true]{
        for maximum in [0,16_384,65_536,u32::MAX]{
            let mut p=request(extra,maximum);
            // Neither zero-count offset nor reserved words are addresses.
            p[66..68].fill(0xd0);p[88..92].fill(0xff);
            p[100..104].fill(0xf1);p[116..120].fill(0x87);
            let body=frame(&p);let f=parsed(&body);
            assert_eq!(f.messages()[0].network_interface_probe(),Some(NetworkProbe::NoHandle{max_output:maximum}));
        }
    }
    for (at,value) in [(12,16),(16,0),(16,0x88),(36,0),(64,56),(68,4),(92,1),(96,1),(104,1),(120,1)]{
        let mut p=request(true,16_384);p[at]=value;
        let body=frame(&p);let f=parsed(&body);
        assert_eq!(f.messages()[0].network_interface_probe(),None,"byte {at}");
    }
    for (at,value,want) in [(72,0x7f,NetworkProbe::InvalidFileId),(80,0x7f,NetworkProbe::InvalidFileId),
        (112,0,NetworkProbe::NotFsctl),(112,3,NetworkProbe::NotFsctl)]{
        let mut p=request(true,16_384);p[at]=value;
        let body=frame(&p);let f=parsed(&body);
        assert_eq!(f.messages()[0].network_interface_probe(),Some(want));
    }
    let mut wrong=request(true,16_384);wrong[72]=0;wrong[112]=0;
    let body=frame(&wrong);let f=parsed(&body);
    assert_eq!(f.messages()[0].network_interface_probe(),Some(NetworkProbe::NotFsctl));
    let mut p=request(false,16_384);p[20]=120;
    let mut another=request(false,16_384);another[24]=4;p.extend_from_slice(&another);
    let body=frame(&p);let f=parsed(&body);
    assert_eq!(f.messages().len(),2);
    assert_eq!(f.messages()[0].network_interface_probe(),None);
}

#[test]
fn interface_probe_uses_a_standard_error_with_no_success_buffer_and_caps_output_budget(){
    let sid=1u64.try_into().unwrap();
    for (charge,maximum,change,want) in [(1,16_384,None,NetworkProbeFailure::NotSupported),
        (2,65_537,None,NetworkProbeFailure::InvalidParameter),
        (1,16_384,Some((80,0x7f)),NetworkProbeFailure::InvalidParameter),
        (1,16_384,Some((112,0)),NetworkProbeFailure::NotSupported)]{
        let run=provider();let mut c=connected(&run);assert_eq!(publish_at(&mut c,1),1);
        let mut p=request(true,maximum);p[6]=charge;
        if let Some((at,value))=change{p[at]=value;}
        sign(&mut p);
        let body=frame(&p);let f=parsed(&body);
        let paid=c.receive(&f).unwrap();
        assert_eq!(paid.payload(0),Some(Payload::Covered));
        let Admission::Ready(mut batch)=paid.preflight(&mut c)else{panic!("expected a signed original");};
        // Codec only. The real dispatcher still requires that TID actually
        // holds this original site's RustRoot, which this fixture lacks.
        let Some(Step::Verified(member))=batch.next().unwrap()else{panic!("expected a covered original");};
        if maximum>member.max_transact()||p[80]!=0xff{
            assert!(UnsignedSmall::network_probe_failure(&member,sid,NetworkProbeFailure::NotSupported).is_err());
        }
        if p[112]!=1{
            assert!(UnsignedSmall::network_probe_failure(&member,sid,NetworkProbeFailure::InvalidParameter).is_err());
        }
        let draft=UnsignedSmall::network_probe_failure(&member,sid,want).unwrap().credit(2);
        let bytes=draft.for_mac();
        assert_eq!(bytes.len(),72);
        assert_eq!(&bytes[8..12],&(want as u32).to_le_bytes());
        assert_eq!(&bytes[12..16],&[11,0,2,0]);
        assert_eq!(&bytes[16..20],&9u32.to_le_bytes());
        assert_eq!(&bytes[20..24],&[0;4]);
        assert_eq!(&bytes[24..48],&p[24..48]);
        assert_eq!(&bytes[48..64],&[0;16]);
        assert_eq!(&bytes[64..],&[9,0,0,0,0,0,0,0]);
    }
}

struct Sink(Arc<Mutex<Vec<u8>>>);
impl Read for Sink {
    fn read(&mut self,_:&mut[u8])->io::Result<usize>{Err(io::ErrorKind::Unsupported.into())}
}
impl Write for Sink{
    fn write(&mut self,p:&[u8])->io::Result<usize>{
        let n=p.len().min(13);self.0.lock().unwrap().extend_from_slice(&p[..n]);Ok(n)
    }
    fn flush(&mut self)->io::Result<()>{panic!("not buffered")}
}

#[test]
fn interface_probe_cannot_infer_a_tree_and_the_signed_error_settles_only_its_original_debt(){
    for (maximum,change) in [(16_384,None),(65_537,None),(16_384,Some((72,0x7f))),(16_384,Some((112,0)))]{
        let run=provider();let mut c=connected(&run);assert_eq!(publish_at(&mut c,1),1);
        let trace=Arc::new(Mutex::new(Vec::new()));
        c.stream=Some(DirectStream::Fixture(Box::new(Sink(Arc::clone(&trace)))));
        let before=c.window.next;
        let mut q=request(true,maximum);
        if let Some((at,value))=change{q[at]=value;}
        sign(&mut q);
        let body=frame(&q);let f=parsed(&body);let paid=c.receive(&f).unwrap();
        assert_eq!(paid.payload(0),Some(if maximum>65_536{Payload::InvalidParameter}else{Payload::Covered}));
        let Admission::Ready(mut batch)=paid.preflight(&mut c)else{panic!("expected a signed original");};
        assert!(batch.try_send_network_interface_refusal().unwrap());drop(batch);

        let mut expected=q[..64].to_vec();expected[8..12].copy_from_slice(&0xc000_00c9u32.to_le_bytes());
        expected[16]=9;expected.extend_from_slice(&[9,0,0,0,0,0,0,0]);sign(&mut expected);
        assert_eq!(&*trace.lock().unwrap(),&frame(&expected));
        assert_eq!(c.in_flight,0);
        assert_eq!(c.window.next,before+2); // exactly the chosen new credits, not old MID reuse

        trace.lock().unwrap().clear();
        let mut echo=q[..64].to_vec();echo[12]=13;echo[14]=1;echo[24]=4;echo[36]=0;
        echo.extend_from_slice(&[4,0,0,0]);sign(&mut echo);
        let bytes=frame(&echo);let next=parsed(&bytes);
        let Admission::Ready(mut b)=c.receive(&next).unwrap().preflight(&mut c) else{panic!("same session");};
        b.echoes().unwrap().unwrap().send().unwrap();drop(b);
        let mut expect_echo=echo;expect_echo[16]=9;sign(&mut expect_echo);
        assert_eq!(&*trace.lock().unwrap(),&frame(&expect_echo));
        assert_eq!(c.in_flight,0);
    }
}

#[test]
fn the_closed_probe_route_does_not_relabel_other_controls_or_unverified_input(){
    for mode in 0..3{
        let run=provider();let mut c=connected(&run);assert_eq!(publish_at(&mut c,1),1);
        let mut q=request(true,16_384);
        if mode==0{q[68..72].copy_from_slice(&0x0014_0204u32.to_le_bytes());} // validate-negotiate is not this control
        if mode==1{q[96]=1;} // this is not the bounded zero-input/response profile
        if mode==2{q[16]=0;}
        sign(&mut q);
        let body=frame(&q);let f=parsed(&body);let paid=c.receive(&f).unwrap();
        match paid.preflight(&mut c){
            Admission::Ready(mut b) if mode<2=>{
                assert!(!b.try_send_network_interface_refusal().unwrap());
                b.before_reply().unwrap(); // no member consumed
            },
            Admission::Refused(_) if mode==2=>{},
            _=>panic!("unexpected probe admission"),
        }
    }
}
