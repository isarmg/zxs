use super::*;
use crate::identity::ShareId;
use super::super::shares::ShareUse;

fn action(g:&Genesis,seq:u64,previous:[u8;32],op:u8,kind:u8,body:&[u8])->Vec<u8>{
    action_for(g,seq,previous,[op;16],kind,body)
}
fn action_for(g:&Genesis,seq:u64,previous:[u8;32],op:[u8;16],kind:u8,body:&[u8])->Vec<u8>{
    let(base,sealed)=match kind{
        9=>(b"ZXS-PORTABLE-1/FIRST-ADMIN\0".as_slice(),48),
        10=>(b"ZXS-PORTABLE-1/PUBLISH-ROOT-SHARE\0".as_slice(),0),
        11=>(b"ZXS-PORTABLE-1/ROOT-ACCESS-GRANT\0".as_slice(),0),
        12=>(b"ZXS-PORTABLE-1/ADD-ACCOUNT\0".as_slice(),48),
        13=>(b"ZXS-PORTABLE-1/CREATE-ROOT-FILE\0".as_slice(),5*16+1),_=>unreachable!(),
    };
    let request=hash::digest(&[base,&g.pool.0,&op,&body[..body.len()-sealed]].concat()).unwrap();
    let mut h=ENC.to_vec();h.extend(((165+body.len()) as u32).to_be_bytes());h.extend(g.pool.0);h.extend(seq.to_be_bytes());
    h.extend(previous);h.push(kind);h.extend(op);h.extend(request);h.extend(body);digest(h)
}
fn terms(credential:[u8;32],share:u8,volume:VolumeId)->Vec<u8>{
    [[0x55;16].as_slice(),&credential,&[share;16],&[0],&volume.0].concat()
}
fn rights(mut b:Vec<u8>,published:[u8;32],target:u8,previous:Option<[u8;32]>,bits:u8)->Vec<u8>{
    b.extend(published);b.extend([target;16]);if let Some(d)=previous{b.push(1);b.extend(d);}else{b.push(0);}
    b.push(bits);b
}
fn certify(g:&Genesis,frames:&[Vec<u8>])->(Vec<u8>,Vec<u8>){
    let tail=frames.concat();let d=frames.last().map(|f|frame_sha(f)).unwrap_or(g.digest);
    let f=lower(g.digest,frames.len() as u64,tail.len(),d);(tail,f)
}
#[test]
fn claims_outside_original_manager_share_root_account_and_predecessor_are_not_admitted(){
    let g=Genesis::only(&field(GEN,"genesis")).unwrap();let a=field(FIRST,"frame");let da=frame_sha(&a);
    let vol=g.rows[2].volume;let t=terms(da,0x71,vol);let s=action(&g,2,da,0x72,10,&t);let ds=frame_sha(&s);
    let right=rights(t.clone(),ds,0x55,None,5);let grant=action(&g,3,ds,0x73,11,&right);
    let valid=|frames:&[Vec<u8>]|{let(tail,floor)=certify(&g,frames);decision::Decisions::read(&g,&tail,&floor).is_ok()};
    assert!(valid(&[a.clone(),s.clone(),grant.clone()]));
    for mut bad in [terms([8;32],0x71,vol),terms(da,0x71,g.rows[0].volume),terms(da,0x71,g.rows[1].volume)]{
        assert!(!valid(&[a.clone(),action(&g,2,da,0x72,10,&bad)]));
        bad[0]^=1;assert!(!valid(&[a.clone(),action(&g,2,da,0x72,10,&bad)]));
    }
    let mut liable=t.clone();liable[0]^=1;
    assert!(!valid(&[a.clone(),action(&g,2,da,0x72,10,&liable)]));
    for bad in [rights(t.clone(),[9;32],0x55,None,5),rights(t.clone(),ds,0x77,None,5),
            rights(terms(da,0x71,g.rows[3].volume),ds,0x55,None,5),rights(terms(da,0x99,vol),ds,0x55,None,5),
            rights(t.clone(),ds,0x55,Some([0;32]),5),rights(t.clone(),ds,0x55,None,8)]{
        assert!(!valid(&[a.clone(),s.clone(),action(&g,3,ds,0x73,11,&bad)]));
    }
    // Correctly checksummed frames, but no publication or duplicate original Op.
    assert!(!valid(&[a.clone(),action(&g,2,da,0x72,11,&right)]));
    assert!(!valid(&[a.clone(),s.clone(),action(&g,3,ds,0x72,11,&right)]));
    let wrong=action(&g,4,frame_sha(&grant),0x74,11,&rights(t,ds,0x55,None,0));
    assert!(!valid(&[a,s,grant,wrong]));
}

fn append(s:&Path,g:&Genesis,frames:&mut Vec<Vec<u8>>,frame:Vec<u8>){
    append_observed(s,g,frames,frame,|_|{});
}
fn append_observed(s:&Path,g:&Genesis,frames:&mut Vec<Vec<u8>>,frame:Vec<u8>,before_replace:impl FnOnce(&[u8])){
    // Test-only construction of a committed cut; this is NOT a service or
    // maintenance writer. Sync the original append and real replacement floor.
    let mut reg=osfs::OpenOptions::new().append(true).open(s.join("REGISTRATION")).unwrap();
    reg.write_all(&frame).unwrap();reg.sync_all().unwrap();frames.push(frame);
    let (_,floor)=certify(g,frames);let pending=s.join("fixture-floor");
    before_replace(&floor);
    create(&pending,&floor);osfs::rename(pending,s.join("CONFIRMED")).unwrap();osfs::File::open(s).unwrap().sync_all().unwrap();
}
#[test]
#[ignore="controlled non-root ordinary files, public Argon2 fixture; no network or real data volume"]
fn same_open_original_reads_only_the_exact_grant_and_never_upgrades_an_old_handle(){
    let t=PrivateTemp::under(&std::env::temp_dir());let s=t.child("sys");let r=t.0.join("no-r");let f=t.0.join("no-files-needed");
    let(g,base,a,m)=in_roots(&s,&r,&f);let da=frame_sha(&a);let mut frames=vec![a.clone()];
    create(&s.join("REGISTRATION"),&[base,a].concat());create(&s.join("CONFIRMED"),&certify(&g,&frames).1);
    create(&s.join(crate::pool::portable_claim::ROOT_NAME.to_str().unwrap()),&m);
    let original=opened::Recorded::open(&s,g.pool).unwrap();let ex=osfs::File::open(s.join("REGISTRATION")).unwrap();
    assert!(ex.try_lock().is_err());assert!(original.authenticate("first.admin","wrong public fixture").is_err());
    // The original REG SH survives the slow verifier but not a stale
    // authorization: the short S lock is free; a sidecar blocks issuance.
    let unknown=s.join("unexpected");
    assert!(original.authenticate_at("first.admin","correct horse battery staple",||{
        let cut=HeldDir::open(&s)?;cut.exclusive()?;assert!(ex.try_lock().is_err());osfs::write(&unknown,[])
    }).is_err());
    osfs::remove_file(&unknown).unwrap();
    let who=original.authenticate("first.admin","correct horse battery staple").unwrap();
    let id=ShareId([0x71;16]);let vol=g.rows[2].volume;let persona=terms(da,0x71,vol);
    assert!(who.select(id,ShareUse::List).is_err());
    let share=action(&g,2,da,0x72,10,&persona);let ds=frame_sha(&share);let first_floor=t.0.join("held-first-floor");
    let in_place=|future:&[u8]|{osfs::write(s.join("CONFIRMED"),future).unwrap();assert!(original.first_result(OperationId([0x44;16])).is_err());};
    append_observed(&s,&g,&mut frames,share,|future|{
        in_place(future);
        // Keep that actual old inode addressable outside S AFTER the real
        // replacement. This is only to exercise its later attempted reuse.
        osfs::hard_link(s.join("CONFIRMED"),&first_floor).unwrap();
    });
    assert!(who.select(id,ShareUse::Published).is_err()); // publish alone has no grant
    let grant=action(&g,3,ds,0x73,11,&rights(persona.clone(),ds,0x55,None,5));let dg=frame_sha(&grant);
    append_observed(&s,&g,&mut frames,grant,|future|{
        in_place(future); // the current cached floor, not necessarily the first
        osfs::write(&first_floor,future).unwrap();osfs::rename(&first_floor,s.join("CONFIRMED")).unwrap();
        assert!(original.first_result(OperationId([0x44;16])).is_err()); // original floor reused, not fresh
    });
    let file_read=who.select(id,ShareUse::Published).unwrap();
    assert_eq!(file_read.root().unwrap(),vol);assert!(who.select(id,ShareUse::Current).is_err());
    let other=action(&g,4,dg,0x74,10,&terms(da,0x80,vol));let dx=frame_sha(&other);append(&s,&g,&mut frames,other);
    assert_eq!(file_read.root().unwrap(),vol);assert!(who.select(ShareId([0x80;16]),ShareUse::List).is_err());
    let revoke=action(&g,5,dx,0x75,11,&rights(persona.clone(),ds,0x55,Some(dg),0));let dv=frame_sha(&revoke);
    append(&s,&g,&mut frames,revoke);assert!(file_read.root().is_err());
    let regrant=action(&g,6,dv,0x76,11,&rights(persona,ds,0x55,Some(dv),5));append(&s,&g,&mut frames,regrant);
    assert!(file_read.root().is_err());assert_eq!(who.select(id,ShareUse::Published).unwrap().root().unwrap(),vol);
    assert!(ex.try_lock().is_err());
    // A previously accepted but now obsolete exact prefix is not current
    // again on this open register. A new apparently valid fork at seq7
    // which omitted the real seq6 likewise cannot extend this observer.
    let held=s.join("REGISTRATION");let lower_path=s.join("CONFIRMED");
    let plen=osfs::metadata(&held).unwrap().len() as usize-frames.iter().map(Vec::len).sum::<usize>();
    let image=osfs::read(&held).unwrap();
    let replace=|which:&[Vec<u8>]|{
        let(tail,floor)=certify(&g,which);
        osfs::write(&held,[&image[..plen],&tail].concat()).unwrap();
        osfs::File::open(&held).unwrap().sync_all().unwrap();
        let new_floor=s.join("fixture-floor");
        create(&new_floor,&floor);osfs::rename(&new_floor,&lower_path).unwrap();osfs::File::open(&s).unwrap().sync_all().unwrap();
    };
    let original_op=OperationId([0x44;16]);
    replace(&frames[..5]);
    assert!(original.first_result(original_op).is_err()); // still present in either history
    let x=action(&g,6,dv,0x84,10,&terms(da,0x83,vol));let y=action(&g,7,frame_sha(&x),0x86,10,&terms(da,0x85,vol));
    let mut fork=frames[..5].to_vec();fork.extend([x,y]);replace(&fork);
    assert!(original.first_result(original_op).is_err()); // higher but lacks original seq6
    let right_next=action(&g,7,frame_sha(&frames[5]),0x87,10,&terms(da,0x88,vol));
    frames.push(right_next);replace(&frames);
    assert_eq!(original.first_result(original_op).unwrap(),Some((crate::identity::AccountId([0x55;16]),da)));
    assert_eq!(who.select(id,ShareUse::Published).unwrap().root().unwrap(),vol);
    assert!(file_read.root().is_err());assert!(!r.exists()&&!f.exists());
    drop(file_read);drop(who);drop(original);ex.try_lock().unwrap();ex.unlock().unwrap();
}

fn ordinary(manager:u8,credential:[u8;32],target:u8,login:&[u8],salt:[u8;16],verifier:[u8;32])->Vec<u8>{
    [[manager;16].as_slice(),&credential,&[target;16],&[login.len() as u8],login,&salt,&verifier].concat()
}
#[test]
fn portable_bootstrap_requires_one_exact_profile_and_grants_only_to_confirmed_accounts(){
    let g=Genesis::only(&field(GEN,"genesis")).unwrap();
    let vol=g.rows[2].volume;
    let valid=|frames:&[Vec<u8>]|{let(tail,floor)=certify(&g,frames);decision::Decisions::read(&g,&tail,&floor).is_ok()};
    let a=field(FIRST,"frame");let aw=field(FIRST,"write_frame");
    let terms1=field(FIRST,"terms");let mut other=terms1.clone();
    assert_eq!(action(&g,1,g.digest,0x44,9,&terms1),a);other[0]=2;
    assert_eq!(action(&g,1,g.digest,0x44,9,&other),aw);assert_ne!(frame_sha(&a),frame_sha(&aw));
    // No missing-mode decoder; no in-band upgrade by rehashing the frame
    // while retaining an approval/request for the narrow management scope.
    for bad in [0,3,255]{other[0]=bad;assert!(!valid(&[action(&g,1,g.digest,0x44,9,&other)]));}
    assert!(!valid(&[action(&g,1,g.digest,0x44,9,&terms1[1..])]));
    let mut altered=a[..a.len()-32].to_vec();altered[165-32]=2;assert!(!valid(&[digest(altered)]));
    let mut upper=terms1.clone();upper[34]=b'F';assert!(!valid(&[action(&g,1,g.digest,0x44,9,&upper)]));
    let mut noid=terms1.clone();noid[1..17].fill(0);assert!(!valid(&[action(&g,1,g.digest,0x44,9,&noid)]));
    let mut no_opref=terms1.clone();no_opref[17..33].fill(0);assert!(!valid(&[action(&g,1,g.digest,0x44,9,&no_opref)]));
    for (first,can_write) in [(a,false),(aw,true)]{
        let da=frame_sha(&first);let b=ordinary(0x55,da,0xa2,b"reader.b",[2;16],[3;32]);
        let account=action(&g,2,da,0x70,12,&b);let db=frame_sha(&account);let persona=terms(da,0x71,vol);
        let publication=action(&g,3,db,0x72,10,&persona);let ds=frame_sha(&publication);
        for (mask,accept) in [(1,true),(5,true),(9,can_write),(17,can_write),(31,can_write),(16,false),(33,false)]{
            let grant=action(&g,4,ds,0x73,11,&rights(persona.clone(),ds,0xa2,None,mask));
            assert_eq!(valid(&[first.clone(),account.clone(),publication.clone(),grant]),accept);
        }
        for bad in [ordinary(0xa2,db,0xc3,b"new.c",[2;16],[3;32]),ordinary(0x55,da,0xa2,b"other",[4;16],[5;32]),
                ordinary(0x55,da,0xc3,b"reader.b",[4;16],[5;32]),ordinary(0x55,da,0xc3,b"first.admin",[4;16],[5;32])]{
            assert!(!valid(&[first.clone(),account.clone(),action(&g,3,db,0x93,12,&bad)]));
        }
        let mut not_manager=terms(db,0x71,vol);not_manager[..16].fill(0xa2);
        let own=action(&g,4,ds,0x73,11,&rights(not_manager.clone(),ds,0xa2,None,31));
        assert!(!valid(&[first.clone(),account.clone(),publication,own]));
        assert!(!valid(&[first,account,action(&g,3,db,0x72,10,&not_manager)]));
    }
}

// Fixed public fixture from the authenticated previous Debian trixie Argon2
// library, t=3, m=65536, p=4, v=19, password "second public synthetic example".
// Keep the verifier independent of the implementation under test.
fn second_public_verifier()->([u8;16],[u8;32]){
    ([0x9b;16],[0x55,0x96,0xe5,0x08,0x4a,0x64,0x6a,0x3e,0x7d,0x03,0xc1,0xa6,0x25,0xfa,0xf5,0x9a,
        0xca,0x7e,0xf0,0xaf,0x78,0xfd,0xa2,0xa4,0x26,0x38,0x5d,0xa1,0xcb,0x62,0x87,0xf2])
}

#[test]
#[ignore="two synthetic subjects, controlled ordinary named SYSTEM; no network or external data disks"]
fn separately_authenticated_users_have_only_their_own_current_share_rights(){
    let t=PrivateTemp::under(&std::env::temp_dir());let s=t.child("sys");let r=t.0.join("no-r");let f=t.0.join("no-f");
    let(g,base,_,m)=in_roots(&s,&r,&f);let a=changed(&field(FIRST,"write_frame"),1,g.digest,9);
    let da=frame_sha(&a);let mut frames=vec![a.clone()];create(&s.join("REGISTRATION"),&[base,a].concat());
    create(&s.join("CONFIRMED"),&certify(&g,&frames).1);create(&s.join(crate::pool::portable_claim::ROOT_NAME.to_str().unwrap()),&m);
    let authority=opened::Recorded::open(&s,g.pool).unwrap();
    let manager=authority.authenticate("first.admin","correct horse battery staple").unwrap();
    let (salt,tag)=second_public_verifier();let b=action(&g,2,da,0x70,12,&ordinary(0x55,da,0xa2,b"reader.b",salt,tag));let db=frame_sha(&b);
    append(&s,&g,&mut frames,b);
    assert!(authority.authenticate("reader.b","correct horse battery staple").is_err());
    assert!(authority.authenticate("unknown","correct horse battery staple").is_err());
    let reader=authority.authenticate("reader.b","second public synthetic example").unwrap();let vol=g.rows[2].volume;
    let id=ShareId([0x71;16]);let p=terms(da,0x71,vol);
    let share=action(&g,3,db,0x72,10,&p);let ds=frame_sha(&share);append(&s,&g,&mut frames,share);
    assert!(reader.select(id,ShareUse::List).is_err());assert!(manager.select(id,ShareUse::List).is_err());
    let set=action(&g,4,ds,0x73,11,&rights(p.clone(),ds,0xa2,None,1|16));let dg=frame_sha(&set);
    append(&s,&g,&mut frames,set);
    let original=reader.select(id,ShareUse::Save).unwrap();assert_eq!(original.root().unwrap(),vol);
    for right in [ShareUse::Published,ShareUse::Current,ShareUse::Create]{assert!(reader.select(id,right).is_err());}
    assert!(manager.select(id,ShareUse::Save).is_err());
    let withdraw=action(&g,5,dg,0x74,11,&rights(p,ds,0xa2,Some(dg),1));append(&s,&g,&mut frames,withdraw);
    assert!(original.root().is_err());assert!(reader.select(id,ShareUse::Save).is_err());
    assert_eq!(reader.select(id,ShareUse::List).unwrap().root().unwrap(),vol);assert!(!r.exists()&&!f.exists());
}

#[path="portable_file_members.rs"]
mod file_members;
