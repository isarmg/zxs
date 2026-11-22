use super::*;
pub(in crate::smb::launch) fn question()->PreflightQuestion{
    let mut w=b"ZXS-RUST-SMB-I1\0".to_vec();
    for id in 1..=5{w.extend([id;16]);}
    w.extend_from_slice(b"\x0bfixture.lan\x01\x04\x18\xc0\x00\x02\x11\x01");
    w.extend([0x31;16]);w.extend_from_slice(b"\x02\x00\x04Docs\x00\x07Recycle");
    PreflightQuestion::encode(QuestionSource{nonce:[66;32],release:[67;32],installed:hash(&w).unwrap(),
        init_actor:[68;16],uid:4012,gid:4013,entity:Entity{device:12,inode:32},
        system:Path::new("/original/S"),install:&w}).unwrap()
}
fn claim(q:&PreflightQuestion,ordinal:u8,id:u8,path:&[u8])->PreflightClaim{
    let mut mark=[0;101];mark[..20].copy_from_slice(b"ZXS-SQLITE-1/VOLUME\0");
    mark[20]=if ordinal==1{1}else{2};mark[21..37].copy_from_slice(&q.plan.origin[0]);
    mark[37..53].fill(id);mark[53..69].copy_from_slice(&q.plan.origin[1]);mark[69..].fill(id);
    PreflightClaim{ordinal,volume:VolumeId([id;16]),path:Path::new(OsStr::from_bytes(path)).to_owned(),component_limit:255,marker:mark}
}
#[test]
fn rust_low_preflight_reply_binds_exact_private_question_and_original_media(){
    let q=question();
    assert_eq!(q.hostname(),"fixture.lan");
    assert_eq!(q.endpoints(),&[("192.0.2.17".parse::<IpAddr>().unwrap(),24)]);
    assert_eq!(q.planned_folders().collect::<Vec<_>>(),vec![(VolumeId([0x31;16]),"Docs",true)]);
    assert_eq!(q.recycle_label(),"Recycle");
    assert!(PreflightQuestion::decode_request(&q.start(),q.bytes().to_vec(),q.subject()).is_ok());
    assert!(PreflightQuestion::decode_request(&q.start(),q.bytes().to_vec(),(q.uid,q.gid+1)).is_err());
    let mut wrong=q.bytes().to_vec();wrong[INPUT.len()+32]^=1;
    assert!(PreflightQuestion::decode_request(&q.start(),wrong,q.subject()).is_err());

    let mut sid=[1,5,0,0,0,0,0,5,21,0,0,0,1,2,3,4,5,6,7,8,9,10,11,12,0,0,0,0];
    sid[24..].copy_from_slice(&1000u32.to_le_bytes());
    let mut expected=PreflightSelection{origin:q.plan.origin,install_sha:q.installed,
        epoch:PreflightEpoch{account:[9;16],op:q.plan.origin[4],record:[10;16],sid,generation:1,login:"files".to_owned()},
        ordinary:vec![claim(&q,7,0x31,b"/ordinary/original-\xff")],recycle:claim(&q,1,0x32,b"/recycle/r")};
    let (packet,body)=q.build_reply(&expected).unwrap();
    assert_eq!(PublicReply::decode_verified(&q,&packet,&body).unwrap().facts(),&expected);
    assert!(PublicReply::decode_verified(&q,&packet,&body[..body.len()-1]).is_err());
    let mut same_frame_wrong_reply=body; same_frame_wrong_reply[OUTPUT.len()]^=1;
    let for_other=frame(DONE,q.nonce,hash(&same_frame_wrong_reply).unwrap());
    assert!(PublicReply::decode_verified(&q,&for_other,&same_frame_wrong_reply).is_err());
    expected.epoch.generation=2; assert!(q.build_reply(&expected).is_err()); expected.epoch.generation=1;
    expected.ordinary[0].marker[53]^=1;assert!(q.build_reply(&expected).is_err());expected.ordinary[0].marker[53]^=1;
    expected.recycle=claim(&q,1,0x32,b"/original/S/other");assert!(q.build_reply(&expected).is_err());
}
