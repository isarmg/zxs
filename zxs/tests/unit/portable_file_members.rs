use super::*;
use crate::{FileId,OperationId};
use std::os::unix::fs::PermissionsExt;

fn new_member(creator:u8,credential:[u8;32],share:u8,published:[u8;32],grant:[u8;32],vol:VolumeId,name:&str,
        ids:[u8;5],slot:u8)->Vec<u8>{
    let mut b=[[creator;16].as_slice(),&credential,&[share;16],&published,&grant,&[0],&vol.0,&[name.len() as u8],name.as_bytes()].concat();
    for id in ids{b.extend([id;16]);}b.push(slot);b
}
fn entry_header(pool:crate::PoolId,op:u8,name:&str,ids:[u8;4])->Vec<u8>{
    let mut h=b"ZXS-PORTABLE-1/FILE\0".to_vec();h.extend(pool.0);h.extend([op;16]);for id in ids{h.extend([id;16]);}
    h.push(name.len() as u8);h.extend(name.as_bytes());h
}
#[test]
fn one_logical_membership_across_shares_and_exact_prior_create_right(){
    let g=Genesis::only(&field(GEN,"genesis")).unwrap();let a=field(FIRST,"write_frame");let da=frame_sha(&a);let vol=g.rows[2].volume;
    let share=action(&g,2,da,0x61,10,&terms(da,0x60,vol));let ds=frame_sha(&share);
    let acl=action(&g,3,ds,0x62,11,&rights(terms(da,0x60,vol),ds,0x55,None,1|8));let dg=frame_sha(&acl);
    let valid=|frames:Vec<Vec<u8>>|{let(tail,floor)=certify(&g,&frames);decision::Decisions::read(&g,&tail,&floor)};
    let defaults=[0x80,0x81,0x82,0x83,0x84];
    let mut b=new_member(0x55,da,0x60,ds,dg,vol,"X",defaults,0);
    let f=action(&g,4,dg,0x63,13,&b);
    assert!(valid(vec![a.clone(),share.clone(),acl.clone(),f.clone()]).unwrap().files.get(vol,"X").is_some());
    // Mutations are rehashed complete envelopes. Digest success cannot
    // supply an unauthorized account, grant, directory/physical role or Op.
    for i in [0,16,48,64,96,128,129,145]{
        let mut wrong=b.clone();wrong[i]^=0x13;
        assert!(valid(vec![a.clone(),share.clone(),acl.clone(),action(&g,4,dg,0x63,13,&wrong)]).is_err());
    }
    let df=frame_sha(&f);
    for (op,ids,name,slot) in [(0x84,[1,2,3,4,5],"another",0),(0x65,[1,2,3,4,0x61],"another",0),
        (0x65,[1,2,3,4,0x65],"another",0),(0x65,[0x80,2,3,4,5],"another",0),(0x65,[1,0x81,3,4,5],"another",0),
        (0x65,[1,2,0x82,4,5],"another",0),(0x65,[1,2,3,0x83,5],"another",0),
        (0x65,[1,2,3,4,5],"X",0),(0x65,[1,2,3,4,0xff],"another",0),(0x65,[1,2,3,4,5],"another",8)]{
        let bad=new_member(0x55,da,0x60,ds,dg,vol,name,ids,slot);
        assert!(valid(vec![a.clone(),share.clone(),acl.clone(),f.clone(),action(&g,5,df,op,13,&bad)]).is_err());
    }
    // Upper/lower are distinct and all-ones is not banned as a general
    // operation. It never makes the permanent root marker into stage zero.
    b=new_member(0x55,da,0x60,ds,dg,vol,"x",[1,2,3,4,5],0);
    assert!(valid(vec![a,share,acl,f,action(&g,5,df,0xff,13,&b)]).is_ok());
}
#[test]
#[ignore="controlled ordinary files and two public synthetic principals; no services or disks"]
fn same_volume_through_two_shares_uses_issued_file_not_the_original_creators_continuing_rights(){
    let t=PrivateTemp::under(&std::env::temp_dir());let s=t.child("s");let r=t.0.join("recycle-offline");let f=t.child("f");
    let(g,base,_,mark)=in_roots(&s,&r,&f);let root_mark=crate::pool::portable_claim::ROOT_NAME.to_str().unwrap();
    let a=changed(&field(FIRST,"write_frame"),1,g.digest,9);let da=frame_sha(&a);let vol=g.rows[2].volume;
    let mut fmark=mark.clone();fmark[22]=2;fmark[39..55].copy_from_slice(&vol.0);create(&f.join(root_mark),&fmark);
    let mut frames=vec![a.clone()];create(&s.join("REGISTRATION"),&[base,a].concat());
    create(&s.join("CONFIRMED"),&certify(&g,&frames).1);create(&s.join(root_mark),&mark);
    let original=opened::Recorded::open(&s,g.pool).unwrap();let manager=original.authenticate("first.admin","correct horse battery staple").unwrap();
    let(salt,tag)=second_public_verifier();let add=action(&g,2,da,0x70,12,&ordinary(0x55,da,0xa2,b"reader.b",salt,tag));let db=frame_sha(&add);
    append(&s,&g,&mut frames,add);let reader=original.authenticate("reader.b","second public synthetic example").unwrap();
    let p=terms(da,0x71,vol);let p2=terms(da,0x72,vol);
    let share=action(&g,3,db,0x73,10,&p);let ds=frame_sha(&share);append(&s,&g,&mut frames,share);
    let share=action(&g,4,ds,0x74,10,&p2);let ds2=frame_sha(&share);append(&s,&g,&mut frames,share);
    let primary=ShareId([0x71;16]);let alternate=ShareId([0x72;16]);
    assert!(manager.select(primary,ShareUse::Create).is_err());
    let grant=action(&g,5,ds2,0x75,11,&rights(p.clone(),ds,0x55,None,9));let dg=frame_sha(&grant);append(&s,&g,&mut frames,grant);
    let grant=action(&g,6,dg,0x76,11,&rights(p2,ds2,0xa2,None,17));let dr=frame_sha(&grant);append(&s,&g,&mut frames,grant);
    let list=reader.select(alternate,ShareUse::List).unwrap();
    assert!(list.list_files().unwrap().is_empty());assert!(reader.select(primary,ShareUse::List).is_err());
    let name="方案.xlsx";let ids=[0x80,0x81,0x82,0x83,0x84];let file=FileId([0x81;16]);let op=OperationId([0x77;16]);
    let terms=new_member(0x55,da,0x71,ds,dg,vol,name,ids,1);let created=action(&g,7,dr,0x77,13,&terms);let dc=frame_sha(&created);
    append(&s,&g,&mut frames,created);assert!(list.list_files().is_err()); // pending is not empty
    let dir=f.join(format!("方案-{file}.xlsx"));osfs::create_dir(&dir).unwrap();osfs::set_permissions(&dir,osfs::Permissions::from_mode(0o700)).unwrap();
    let bytes=entry_header(g.pool,0x77,name,[ids[0],ids[1],ids[2],ids[3]]);let header=dir.join("FILE");create(&header,&bytes);
    let external=f.join(format!(".zxs-new-{op}-7"));assert!(list.list_files().is_err());
    create(&external,&bytes);assert!(list.list_files().is_err()); // same bytes, wrong actual file
    osfs::remove_file(&external).unwrap();osfs::hard_link(&header,&external).unwrap();
    let attrs=list.list_files().unwrap();assert_eq!(attrs.len(),1);assert_eq!((attrs[0].file,attrs[0].name.as_str()),(file,name));
    assert_eq!(attrs[0].created_by,op);
    let can_save=reader.select(alternate,ShareUse::Save).unwrap();assert_eq!(can_save.file(name,file).unwrap(),attrs[0]);
    assert!(can_save.file("方案.XLSX",file).is_err());assert!(can_save.file(name,FileId([1;16])).is_err());
    assert!(manager.select(primary,ShareUse::Save).is_err());assert!(reader.select(alternate,ShareUse::Create).is_err());
    let withdraw=action(&g,8,dc,0x78,11,&rights(p,ds,0x55,Some(dg),0));append(&s,&g,&mut frames,withdraw);
    assert!(manager.select(primary,ShareUse::Create).is_err());assert_eq!(list.list_files().unwrap(),attrs);
    let old=t.0.join("original-container");osfs::rename(&dir,&old).unwrap();osfs::create_dir(&dir).unwrap();
    osfs::set_permissions(&dir,osfs::Permissions::from_mode(0o700)).unwrap();create(&dir.join("FILE"),&bytes);
    assert!(can_save.file(name,file).is_err());assert!(list.list_files().is_err());
    let wrong=t.0.join("unrelated-container");osfs::rename(&dir,&wrong).unwrap();osfs::rename(old,&dir).unwrap();
    assert_eq!(can_save.file(name,file).unwrap().entry,attrs[0].entry);
    let lower=f.join(format!(".zxs-new-{}-0",OperationId([0x84;16])));osfs::create_dir(&lower).unwrap();
    osfs::set_permissions(&lower,osfs::Permissions::from_mode(0o700)).unwrap();assert_eq!(list.list_files().unwrap(),attrs);
    create(&lower.join("unknown"),b"leave me");assert!(list.list_files().is_err());
    assert_eq!(osfs::read(lower.join("unknown")).unwrap(),b"leave me");assert!(!r.exists());
}

// This is a real complete authorized frame stream in memory, including its
// 8 MiB/8192 original floor. The root-local inventory budget is different.
#[test]
fn cold_birth_admission_counts_exact_native_names_and_bytes_per_original_root(){
    let g=Genesis::only(&field(GEN,"genesis")).unwrap();let a=field(FIRST,"write_frame");let da=frame_sha(&a);
    let vol=g.rows[2].volume;let other=g.rows[3].volume;
    let p=action(&g,2,da,0x61,10,&terms(da,0x60,vol));let ds=frame_sha(&p);
    let q=action(&g,3,ds,0x62,11,&rights(terms(da,0x60,vol),ds,0x55,None,9));let dg=frame_sha(&q);
    let p2=action(&g,4,dg,0x64,10,&terms(da,0x63,other));let ds2=frame_sha(&p2);
    let q2=action(&g,5,ds2,0x65,11,&rights(terms(da,0x63,other),ds2,0x55,None,9));let dg2=frame_sha(&q2);
    let prefixes=vec![a,p,q,p2,q2];let mut src=vec![];
    let put=|frames:&mut Vec<Vec<u8>>,n:usize,display:String,slot:u8,second:bool|{
        let(sh,ds,dg,vol)=if second{(0x63,ds2,dg2,other)}else{(0x60,ds,dg,vol)};
        let mut b=new_member(0x55,da,sh,ds,dg,vol,&display,[1;5],slot);
        let start=b.len()-81;
        let identity=|j:u8|((n as u128+1)*16+u128::from(j)).to_be_bytes();
        for j in 0..5{b[start+16*j..start+16*(j+1)].copy_from_slice(&identity(1+j as u8));}
        let next=action_for(&g,1+frames.len() as u64,frame_sha(frames.last().unwrap()),identity(0),13,&b);
        frames.push(next);
    };
    let accepts=|frames:&Vec<Vec<u8>>|{let (tail,lower)=certify(&g,frames);
        assert!(frames.len()<8192 && tail.len()<8*1024*1024);
        decision::Decisions::read(&g,&tail,&lower).is_ok()};
    src.extend(prefixes.clone());for n in 0..4095{put(&mut src,n,format!("f{n}"),0,false);}
    assert!(accepts(&src)); // 8191: marker + 2*4095. Next would be 8193.
    put(&mut src,5000,"w".into(),0,false);assert!(!accepts(&src));src.pop();
    put(&mut src,5000,"w".into(),0,true);assert!(accepts(&src));
    let mut bytes=prefixes;for n in 0..1885{put(&mut bytes,n,format!("{n:0200}"),0,false);}
    // Marker=44, each previous FILE=278, remaining=214 for a 136-byte
    // name +33-byte suffix/framing and a 43-byte witness plus its framing.
    put(&mut bytes,5000,"x".repeat(136),0,false);assert!(accepts(&bytes));
    bytes.pop();put(&mut bytes,5000,"x".repeat(137),0,false);assert!(!accepts(&bytes));
    bytes.pop();put(&mut bytes,5000,"x".repeat(136),1,false);assert!(!accepts(&bytes)); // one real lower slot
}
#[test]
fn issued_birth_can_fit_its_fixed_inner_roles_in_the_longest_transfer_enclosure(){
    use crate::ordinary::portable_birth::Birth;
    fn long(n:usize)->std::path::PathBuf{
        let mut p=String::new();while n-p.len()>205{p.push('/');p.push_str(&"p".repeat(199));}
        let remain=n-p.len();p.push('/');p.push_str(&"q".repeat(remain-1));p.into()
    }
    let b=Birth{pool:crate::PoolId([1;16]),volume:VolumeId([2;16]),operation:OperationId([3;16]),
        entry:crate::EntryId([4;16]),file:crate::FileId([5;16]),history:crate::HistoryId([6;16]),branch:crate::BranchId([7;16]),
        ticket:OperationId([8;16]),slot:0,name:"a".into()};
    let limit=ComponentLimit::from_original(255).unwrap();let good=long(3989);let bad=long(3990);let short=Path::new("/role");
    let check=|f:&Path,r:&Path|b.validate_geometry((f,limit),(r,limit),crate::pool::portable_claim::ROOT_NAME);
    assert!(check(&good,short).is_ok());assert!(check(short,&good).is_ok());
    assert!(check(&bad,short).is_err());assert!(check(short,&bad).is_err());
}
