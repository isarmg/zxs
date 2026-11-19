use super::*;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
fn data()->(BirthRow,Vec<ClaimRow>){
    let paths:[&[u8];4]=[b"/srv/zxs-s",b"/mnt/zxs-r",b"/mnt/f-a",b"/mnt/f-\xff"];
    let rows=paths.iter().enumerate().map(|(i,p)|ClaimRow{ordinal:i as u8,role:role_at(i),
        volume:VolumeId([0x11+i as u8;16]),path:Path::new(OsStr::from_bytes(p)).to_owned(),
        limit:ComponentLimit::from_original(255).unwrap(),token:[0x21+i as u8;32]}).collect();
    let request=[0x5f,0x78,0x94,0x41,0x27,0x99,0xad,0x53,0xef,0x6a,0xb1,0xfe,0x00,0x98,0x16,0x78,
                 0x55,0x06,0xd3,0x89,0xde,0x2c,0x1e,0xdd,0x9b,0x02,0xe7,0x9b,0xe6,0xab,0xa1,0x1a];
    (BirthRow{pool:PoolId([4;16]),op:OperationId([1;16]),actor:[2;16],approval:[3;16],request,
              state:InitState::PendingIo,uid:1000,count:4},rows)
}
#[test]fn exact_original_request_distinguishes_logical_order_and_issued_results(){
    let (birth,rows)=data();let c=Catalog::checked(birth.pool,birth,rows.clone()).unwrap();
    assert_eq!(c.state(),InitState::PendingIo);assert_eq!(c.rows().len(),4);
    let mut mark=Vec::from(&b"ZXS-SQLITE-1/VOLUME\0"[..]);mark.push(2);
    for id in [[4;16],[0x14;16],[1;16]]{mark.extend_from_slice(&id);}
    mark.extend_from_slice(&[0x24;32]);assert!(c.marker_matches(3,&mark));
    assert!(!c.marker_matches(2,&mark));assert!(!c.marker_matches(4,&mark));
    mark[69]^=128;assert!(!c.marker_matches(3,&mark));
    // An issuer-generated ID does not change the caller's input; it can only be
    // obtained as an immutable result of this same pending/committed SQL op.
    let mut alternative=rows.clone();alternative[3].volume=VolumeId([9;16]);
    assert!(Catalog::checked(birth.pool,birth,alternative).is_ok());
    let mut dup=rows.clone();dup[3].token=dup[2].token;
    assert!(Catalog::checked(birth.pool,birth,dup).is_err());
    let mut swapped=rows.clone();swapped.swap(2,3);
    assert!(Catalog::checked(birth.pool,birth,swapped).is_err());
    let mut other=birth;other.actor[0]^=1;assert!(Catalog::checked(birth.pool,other,rows.clone()).is_err());
    assert!(Catalog::checked(PoolId([5;16]),birth,rows.clone()).is_err());
    assert!(Catalog::checked(birth.pool,birth,rows[..3].to_vec()).is_err());
    other=birth;other.state=InitState::Committed;
    assert_eq!(Catalog::checked(birth.pool,other,rows).unwrap().state(),InitState::Committed);
}
#[test]fn only_system_requires_utf8_and_roots_are_not_nested(){
    let (b,rows)=data();let mut paths:Vec<&Path>=rows.iter().map(|r|r.path.as_path()).collect();
    assert_eq!(init_request(b.op,b.actor,b.approval,b.uid,&paths).unwrap(),b.request);
    paths.swap(2,3);assert!(init_request(b.op,b.actor,b.approval,b.uid,&paths).is_err());
    paths.swap(0,2);assert!(init_request(b.op,b.actor,b.approval,b.uid,&paths).is_err());
    paths[0]=Path::new("/exact");paths[1]=Path::new("/exact/recycle");
    assert!(init_request(b.op,b.actor,b.approval,b.uid,&paths).is_err());
}
