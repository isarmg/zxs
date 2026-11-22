use super::*;
use crate::{fs::HeldDir,test_support::PrivateTemp};
use std::{fs as osfs,io::Write,os::unix::fs::OpenOptionsExt};

fn field(src:&str,key:&str)->Vec<u8>{
    let s=src.lines().filter_map(|l|l.split_once('=')).find(|(k,_)|*k==key).unwrap().1;
    (0..s.len()).step_by(2).map(|n|u8::from_str_radix(&s[n..n+2],16).unwrap()).collect()
}
const GEN:&str=include_str!("../vectors/portable-genesis-v1.txt");
const FIRST:&str=include_str!("../vectors/portable-first-decision-v1.txt");
const ENC:&[u8]=b"ZXS-PORTABLE-1/DECISION\0";
fn digest(mut b:Vec<u8>)->Vec<u8>{let d=hash::digest(&b).unwrap();b.extend(d);b}
fn lower(g:[u8;32],count:u64,n:usize,d:[u8;32])->Vec<u8>{
    [FLOOR.as_slice(),&g,&count.to_be_bytes(),&(n as u64).to_be_bytes(),&d].concat()
}
fn changed(frame:&[u8],seq:u64,prev:[u8;32],kind:u8)->Vec<u8>{
    let mut b=frame[..frame.len()-32].to_vec();let pos=ENC.len()+4+16;
    b[pos..pos+8].copy_from_slice(&seq.to_be_bytes());b[pos+8..pos+40].copy_from_slice(&prev);b[pos+40]=kind;digest(b)
}
fn frame_sha(bytes:&[u8])->[u8;32]{bytes[bytes.len()-32..].try_into().unwrap()}
#[test]
fn first_decision_uses_the_confirmed_original_frame_not_a_higher_complete_suffix(){
    let base=field(GEN,"genesis");let f=field(FIRST,"frame");let floor=field(FIRST,"floor");
    let mut both=base.clone();both.extend(&f);let(g,tail)=Genesis::prefix(&both).unwrap();
    let id=OperationId([0x44;16]);
    assert_eq!(tail,f.as_slice());assert_eq!((f.len(),floor.len()),(258,105));assert!(Genesis::only(&both).is_err());
    let d=decision::Decisions::read(&g,tail,&floor).unwrap();let a=d.accounts.first().unwrap();
    assert_eq!(a.account.0,[0x55;16]);assert_eq!(a.operation,id);assert_eq!(a.decision,frame_sha(&f));
    assert_eq!(a.decision.as_slice(),field(FIRST,"decision"));assert_eq!(lower(g.digest,1,f.len(),a.decision),floor);
    let zero=field(GEN,"zero_floor");assert!(decision::Decisions::read(&g,&[],&zero).unwrap().accounts.first().is_none());
    assert!(decision::Decisions::read(&g,tail,&zero).is_err());
    for len in 0..f.len(){assert!(decision::Decisions::read(&g,&f[..len],&floor).is_err());}
    let two=changed(&f,2,a.decision,9);
    let tail2=[tail,two.as_slice()].concat();
    assert!(decision::Decisions::read(&g,&tail2,&floor).is_err());
    assert!(decision::Decisions::read(&g,&tail2,&lower(g.digest,2,tail2.len(),frame_sha(&two))).is_err());
    for (seq,prev,kind) in [(2,g.digest,9),(1,[0;32],9),(1,g.digest,99)]{
        let bad=changed(&f,seq,prev,kind);assert!(decision::Decisions::read(&g,&bad,&lower(g.digest,1,bad.len(),frame_sha(&bad))).is_err());
    }
    let mut altered=f[..f.len()-32].to_vec();let request=ENC.len()+4+16+8+32+1+16;
    altered[request]^=1;let bad=digest(altered);
    assert!(decision::Decisions::read(&g,&bad,&lower(g.digest,1,bad.len(),frame_sha(&bad))).is_err());
    assert!(decision::Decisions::read(&g,&f,&lower(g.digest,1,f.len()+base.len(),a.decision)).is_err());
    assert!(decision::Decisions::read(&g,&f,&lower([7;32],1,f.len(),a.decision)).is_err());
}

#[test]
fn upstream_password_and_exact_login_match_the_confirmed_portable_credential_only(){
    let g=Genesis::only(&field(GEN,"genesis")).unwrap();
    let frame=field(FIRST,"frame");let floor=field(FIRST,"floor");
    let cut=decision::Decisions::read(&g,&frame,&floor).unwrap();let a=cut.accounts.first().unwrap();
    assert!(a.password_matches("first.admin","correct horse battery staple").unwrap());
    assert!(!a.password_matches("First.Admin","correct horse battery staple").unwrap());
    assert!(!a.password_matches("first.admin","wrong public fixture").unwrap());
}

fn create(p:&Path,bytes:&[u8]){let mut fd=osfs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(p).unwrap();fd.write_all(bytes).unwrap();fd.sync_all().unwrap();}
#[test]
#[ignore="protected ordinary temporary directory with non-root storage owner; no data or recycler device needed"]
fn original_named_system_observes_only_the_precise_confirmed_nonsecret_result(){
    let t=PrivateTemp::under(&std::env::temp_dir());let s=t.child("s");let r=t.0.join("missing-r");let f=t.0.join("missing-f");
    let(g,base,f1,m)=in_roots(&s,&r,&f);
    let result=frame_sha(&f1);let floor=lower(g.digest,1,f1.len(),result);
    let markname=super::super::portable_claim::ROOT_NAME.to_str().unwrap();
    let state=s.join("REGISTRATION");let bound=s.join("CONFIRMED");create(&s.join(markname),&m);
    let image=[base,f1].concat();create(&state,&image);create(&bound,&lower(g.digest,0,0,g.digest));
    assert!(opened::Recorded::open(&s,g.pool).is_err());osfs::write(&bound,&floor).unwrap();
    assert!(opened::Empty::open(&s).is_err());assert!(opened::Recorded::open(&s,PoolId([255;16])).is_err());
    let old=opened::Recorded::open(&s,g.pool).unwrap();let lease=osfs::File::open(&state).unwrap();assert!(lease.try_lock().is_err());
    assert_eq!(old.first_result(OperationId([0x44;16])).unwrap(),Some((crate::identity::AccountId([0x55;16]),result)));
    assert_eq!(old.first_result(OperationId([0x77;16])).unwrap(),None);
    // Adjacent unknown sidecar, or the same floor under a replacement
    // file: the previously held receipt observation cannot open that cut.
    create(&s.join("unexpected"),b"");assert!(old.first_result(OperationId([0x44;16])).is_err());osfs::remove_file(s.join("unexpected")).unwrap();
    osfs::rename(&bound,t.0.join("old-lower")).unwrap();create(&bound,&floor);
    assert!(old.first_result(OperationId([0x44;16])).is_err());drop(old);lease.try_lock().unwrap();lease.unlock().unwrap();
    assert_eq!(osfs::read(state).unwrap(),image);assert!(!r.exists()&&!f.exists());
}

fn in_roots(s:&Path,r:&Path,f:&Path)->(Genesis,Vec<u8>,Vec<u8>,Vec<u8>){
    let held=HeldDir::open(s).unwrap();let limit=ComponentLimit::held(&held).unwrap().encoded();let uid=fs::local_uid();assert_ne!(uid,0);
    let mut b=field(GEN,"body")[..75].to_vec();b[71..75].copy_from_slice(&uid.to_be_bytes());b.extend(3u16.to_be_bytes());
    for (i,path) in [s,r,f].into_iter().enumerate(){let raw=path.as_os_str().as_bytes();b.push(i as u8);
        b.extend([1+i as u8;16]);b.push(limit);b.extend((raw.len() as u16).to_be_bytes());b.extend(raw);}
    let base=digest(b);let g=Genesis::only(&base).unwrap();let f1=changed(&field(FIRST,"frame"),1,g.digest,9);
    let mut m=b"ZXS-PORTABLE-1/VOLUME\0".to_vec();m.push(0);for raw in [g.pool.0,g.rows[0].volume.0,g.op.0]{m.extend(raw);}m.extend(g.digest);
    assert!(Declaration::decode(&m).unwrap()==g.mark(&g.rows[0]));
    (g,base,f1,m)
}

#[path="portable_share_access.rs"]
mod access_tests;
