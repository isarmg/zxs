use super::*;
use super::super::low::tests::question;
#[test]
fn web_boot_requires_the_same_typed_original_and_distinct_exact_listener_context(){
    let origin=question();
    let (mut packet,mut bytes)=encode(&origin,[33;32],42,true,super::super::cohort::fixture()).unwrap();
    let selected=WebStart::decode(packet,&bytes,origin.subject()).unwrap();
    assert_eq!(selected.original.bytes(),origin.bytes());
    assert_eq!((selected.cookie,selected.read_only),(42,true));
    assert_eq!(&selected.prepared[..16],PREPARED);
    assert_eq!(&selected.prepared[16..],&packet[16..]);
    assert!(WebStart::decode(origin.start(),&bytes,origin.subject()).is_err());
    assert!(WebStart::decode(packet,&bytes,(origin.subject().0,0)).is_err());
    assert!(encode(&origin,[33;32],0,false,super::super::cohort::fixture()).is_err());
    assert!(WebStart::decode(packet,&bytes[..bytes.len()-1],origin.subject()).is_err());
    bytes[BODY.len()+40]=2;packet=bound([33;32],&bytes).unwrap();
    assert!(WebStart::decode(packet,&bytes,origin.subject()).is_err());
    bytes[BODY.len()+40]=0;bytes.push(0);packet=bound([33;32],&bytes).unwrap();
    assert!(WebStart::decode(packet,&bytes,origin.subject()).is_err());
}

#[test]
fn web_boot_has_no_mode_slot_or_earlier_envelope(){
    let original=question();let nonce=[33;32];
    let (frame,body)=encode(&original,nonce,42,true,super::super::cohort::fixture()).unwrap();
    let at=BODY.len()+FIXED;
    assert_eq!(&body[at..],original.bytes());
    let mut with_mode=body.clone();with_mode.insert(at,0);
    assert!(WebStart::decode(bound(nonce,&with_mode).unwrap(),&with_mode,original.subject()).is_err());
    let mut earlier=frame;earlier[..16].copy_from_slice(b"ZXS-WEB/START-3\0");
    assert!(WebStart::decode(earlier,&body,original.subject()).is_err());
}
