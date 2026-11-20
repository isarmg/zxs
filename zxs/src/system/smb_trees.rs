//! One bounded job owns an actual F directory and its original marker, its
//! own read-only SQLite connection, and the private endpoint of one TCON.
//! No host path, physical inode, block device or FILE descriptor crosses to smbd.
use super::{InitialReader,Stamp,attempts::TreeOrigin,denied,os};
use super::super::{SmbSession,Tree,directory,share_mode::Setting};
use crate::{PoolId,fs::ControlRead,identity::ShareId,system::{lease::SystemDir,reader::volume::OrdinaryRoot}};
use std::{fs::File,io,os::fd::{AsRawFd,FromRawFd,OwnedFd},sync::{Arc,atomic::{AtomicBool,Ordering}},thread};

const GLOBAL:usize=32;
const PER_SESSION:usize=8;
unsafe extern "C"{
    fn zxs_smb_tree_reply(fd:i32,result:*const u8)->i32;
    fn zxs_smb_root_wait(tree:i32,session:i32,timeout:i32)->i32;
    fn zxs_smb_process_live(pidfd:i32)->i32;
    fn zxs_dir_take(tree:i32,worker:i32,limit:*mut u8,cursor:*mut u8)->i32;
    fn zxs_dir_reply(fd:i32,page:*const u8)->i32;
    fn zxs_member_reply(fd:i32,info:*const u8)->i32;
    fn zxs_smb_ctl_live(fd:i32)->i32;
}
#[derive(Clone,Copy)]
pub(super) struct Site<'a>{dir:&'a SystemDir,mark:&'a ControlRead,pool:PoolId,shape:[u8;32],binding:&'a File,stamp:&'a Stamp}
impl<'a> Site<'a>{
    pub(super) fn from_reader(src:&'a InitialReader,binding:&'a File,stamp:&'a Stamp)->Self{
        Self{dir:&src.dir,mark:&src.marker,pool:src.pool,shape:src.expected_schema,binding,stamp}
    }
    fn validate(&self)->io::Result<()>{
        if Stamp::read(self.binding)?!=*self.stamp{return Err(denied());}Ok(())
    }
}
pub(super) struct Workers<'scope,'env:'scope>{
    scope:&'scope thread::Scope<'scope,'env>,site:Site<'env>,
    jobs:Vec<(Arc<AtomicBool>,thread::ScopedJoinHandle<'scope,()>)>,
}
impl<'scope,'env:'scope> Workers<'scope,'env>{
    pub(super) fn new(scope:&'scope thread::Scope<'scope,'env>,site:Site<'env>)->Self{
        Self{scope,site,jobs:Vec::with_capacity(GLOBAL)}
    }
    pub(super) fn submit(&mut self,origin:TreeOrigin,share:ShareId,reply:OwnedFd){
        self.jobs.retain(|(_,h)|!h.is_finished());
        if self.jobs.len()>=GLOBAL || self.jobs.iter().filter(|(owner,_)|Arc::ptr_eq(owner,&origin.alive)).count()>=PER_SESSION{
            return; // fail just this tree; no SQL, F, or per-session slot
        }
        let owner=origin.alive.clone();let site=self.site;
        let job=thread::Builder::new().name("zxs-f-root".into()).spawn_scoped(self.scope,move||{
            let _=serve(site,origin,share,reply);
        });
        if let Ok(h)=job{self.jobs.push((owner,h));}
    }
}
fn live(origin:&TreeOrigin,tree:&OwnedFd,timeout:i32)->bool{
    origin.alive.load(Ordering::Acquire) &&
        unsafe{zxs_smb_process_live(origin.worker.as_raw_fd())}==0 &&
        unsafe{zxs_smb_root_wait(tree.as_raw_fd(),origin.session.as_raw_fd(),timeout)}>=0
}
fn choice(setting:Setting,pool:PoolId,ordinal:u8)->io::Result<[u8;74]>{
    if !(2..64).contains(&ordinal)||setting.revision<=0||!matches!(setting.mode as u8,1|2){return Err(denied());}
    let mut b=[0;74];b[..16].copy_from_slice(&setting.share.0);b[16..32].copy_from_slice(&setting.volume.0);
    b[32..48].copy_from_slice(&setting.op.0);b[48..56].copy_from_slice(&setting.revision.to_be_bytes());
    b[56]=setting.mode as u8;b[57..73].copy_from_slice(&pool.0);b[73]=ordinal;Ok(b)
}
/// One SQL cut chooses the same grant and its actual F row. Take the
/// original directory and marker outside SQL, then reselect the same grant
/// before there can be an answer. The returned root still holds both FDs.
fn attach<'r>(session:&SmbSession<'r>,share:ShareId,after:impl FnOnce()->io::Result<()>)->io::Result<(OrdinaryRoot<'r>,[u8;74],Setting)>{
    let (tree,c)=session.select_with_catalog(share)?;
    let root=OrdinaryRoot::for_body(session.source,&c,tree.setting.volume)?;
    let bytes=choice(tree.setting,session.source.pool,root.ordinal)?;
    root.require_streaming_at(after)?;
    session.source.at(|db,c|tree.require(db,&c,false))?; // SQL authority only; no F path or marker I/O
    Ok((root,bytes,tree.setting))
}
fn serve(site:Site<'_>,origin:TreeOrigin,share:ShareId,reply:OwnedFd)->io::Result<()>{
    if origin.slot.pool!=site.pool||!live(&origin,&reply,0){return Err(denied());}
    site.validate()?;
    let src=InitialReader::alongside(site.dir,site.mark,site.pool,site.shape)?;
    let session=SmbSession{source:&src,original:origin.slot.original.clone()};
    let (root,bytes,setting)=attach(&session,share,||site.validate())?;
    if !live(&origin,&reply,0){return Err(denied());}
    os(unsafe{zxs_smb_tree_reply(reply.as_raw_fd(),bytes.as_ptr())})?;
    let tree=Tree{session:&session,setting};
    // Exact member/header/working materials survive on this tree's one
    // worker. Closing the FSP closes its private peer. No thread, writable
    // permit, SQLite connection or data transfer is created per open FILE.
    let mut members=Vec::<(OwnedFd,super::super::first_work::Current<'_,'_,'_>)>::new();
    const MAX_MEMBERS:usize=16;
    while live(&origin,&reply,0){
        members.retain(|(endpoint,_)|unsafe{zxs_smb_ctl_live(endpoint.as_raw_fd())}==0);
        match unsafe{zxs_smb_root_wait(reply.as_raw_fd(),origin.session.as_raw_fd(),500)}{
            0=>continue,1=>{},_=>break,
        }
        let mut limit=0;let mut cursor=[0;223];
        let fd=unsafe{zxs_dir_take(reply.as_raw_fd(),origin.worker.as_raw_fd(),&mut limit,cursor.as_mut_ptr())};
        if fd<0{break;} // original session remains usable on its other trees
        let answer=unsafe{OwnedFd::from_raw_fd(fd)};
        if limit==255{
            if members.len()>=MAX_MEMBERS || site.validate().is_err() || !live(&origin,&reply,0){continue;}
            let Ok(name)=crate::display::component(&cursor[1..1+usize::from(cursor[0])],222) else{continue;};
            let Ok((handle,key))=tree.open_current_root(&root,name) else{continue;};
            let (p,v,e,f)=handle.original_ids();
            let mut data=[0u8;88];
            for(i,id)in [p.0,v.0,e.0,f.0].iter().enumerate(){data[16*i..16*i+16].copy_from_slice(id);}
            for(i,num)in [key.words().1,handle.working_revision(),handle.size()].iter().enumerate(){
                data[64+8*i..72+8*i].copy_from_slice(&num.to_be_bytes());}
            if !live(&origin,&reply,0)||site.validate().is_err(){break;}
            if unsafe{zxs_member_reply(answer.as_raw_fd(),data.as_ptr())}==0{members.push((answer,handle));}
            continue;
        }
        let result=(||->io::Result<_>{
            site.validate()?;
            if !live(&origin,&reply,0){return Err(denied());}
            if limit==0{
                root.require_streaming()?;
                src.at(|db,c|tree.require(db,&c,false))?;
                return Ok(Vec::new());
            }
            directory::page(&tree,&root,&cursor[1..1+usize::from(cursor[0])],limit,||Ok(()))
        })();
        let Ok(rows)=result else{continue;}; // close only this answer, never send a prefix
        if !live(&origin,&reply,0) || site.validate().is_err(){break;}
        let mut out=[0u8;1+16*(1+222+8+8)];out[0]=rows.len() as u8;
        for (row,slot) in rows.iter().zip(out[1..].chunks_exact_mut(239)){
            slot[0]=row.name.len() as u8;slot[1..1+row.name.len()].copy_from_slice(row.name.as_bytes());
            slot[223..231].copy_from_slice(&row.number.to_be_bytes());slot[231..239].copy_from_slice(&row.size.to_be_bytes());
        }
        let _=unsafe{zxs_dir_reply(answer.as_raw_fd(),out.as_ptr())}; // cancel lost no cursor, grant or data
    }
    drop(members);drop(root);Ok(())
}

#[cfg(test)]
mod tests{
    use super::*;
    use crate::{OperationId,system::reader::{ordinary_pool,account::share_mode::Mode},test_support::PrivateTemp};
    #[test]
    #[ignore="source only: actual private S, F and SQL; successful SMB login supplied by fixture"]
    fn successful_root_requires_original_volume_and_unchanged_authority_after_hold(){
        let tmp=PrivateTemp::under(&std::env::temp_dir());
        let(path,p,f,rw,ro)=ordinary_pool(&tmp);
        let src=InitialReader::open(&path,p).unwrap();
        let other=InitialReader::alongside(&src.dir,&src.marker,p,src.expected_schema).unwrap();
        let s=SmbSession::fixture_authenticated(&other,"files.smb").unwrap();
        let a=attach(&s,rw,||Ok(())).unwrap();let b=attach(&s,ro,||Ok(())).unwrap();
        assert_ne!(&a.1[..16],&b.1[..16]);assert_eq!(&a.1[16..32],&b.1[16..32]);
        assert_eq!(a.1[56],2);assert_eq!(b.1[56],1);assert_eq!(&a.1[57..73],&p.0);
        assert_eq!(a.1[73],2);assert!(a.0.root.same_mount(&b.0.root).unwrap());
        let away=tmp.0.join("f-detached");std::fs::rename(&f,&away).unwrap();
        assert!(a.0.require_streaming().is_err());assert!(attach(&s,rw,||Ok(())).is_err());
        std::fs::rename(&away,&f).unwrap();
        let web=src.authenticate_web("first.admin","correct horse battery staple").unwrap();
        let prior_other=OperationId(b.1[32..48].try_into().unwrap());
        let while_checking_f=attach(&s,rw,||{
            // The observer itself is outside a transaction at the real F
            // pathname/stamp check. A separate writer can actually COMMIT.
            assert_ne!(unsafe{crate::system::reader::sqlite3_get_autocommit(other.conn.0)},0);
            let off=web.set_share_mode(OperationId([0xb6;16]),ro,prior_other,Mode::Disabled)?;
            web.set_share_mode(OperationId([0xb7;16]),ro,off.op,Mode::ReadOnly)?;Ok(())
        }).unwrap();
        assert_eq!(a.1,while_checking_f.1,"unrelated share changed, selected grant still current");
        let previous=OperationId(a.1[32..48].try_into().unwrap());let mut reached=false;
        assert!(attach(&s,rw,||{
            let off=web.set_share_mode(OperationId([0xb8;16]),rw,previous,Mode::Disabled)?;
            web.set_share_mode(OperationId([0xb9;16]),rw,off.op,Mode::ReadWrite)?;
            reached=true;Ok(())
        }).is_err());assert!(reached,"same share regranted after opening actual F, before reply");
        let new=attach(&s,rw,||Ok(())).unwrap();assert_ne!(&a.1[32..48],&new.1[32..48]);
        assert!(attach(&s,ShareId([0x91;16]),||Ok(())).is_err());
        assert!(!other.snapshot().unwrap().rows()[1].path.exists()); // no R probe required
    }
    #[test]
    #[ignore="source only: original ordinary FILE and separate SQL connections; not a network authentication fixture"]
    fn independent_read_opens_hold_the_same_member_but_not_a_replacement_volume_or_later_grant(){
        use std::{fs::OpenOptions,os::unix::fs::{FileExt,OpenOptionsExt}};
        let t=PrivateTemp::under(&std::env::temp_dir());let(path,p,f,rw,ro)=ordinary_pool(&t);
        let src=InitialReader::open(&path,p).unwrap();
        let observer=InitialReader::alongside(&src.dir,&src.marker,p,src.expected_schema).unwrap();
        let s=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();
        let o=SmbSession::fixture_authenticated(&observer,"files.smb").unwrap();
        let(r1,_,_)=attach(&s,rw,||Ok(())).unwrap();let(r2,_,_)=attach(&o,ro,||Ok(())).unwrap();
        let w=s.connect(rw).unwrap();let r=o.connect(ro).unwrap();
        let born=w.create_file(OperationId([0xa6;16]),"文档").unwrap();
        let(empty,k1)=r.open_current_root(&r2,"文档").unwrap();assert_eq!(empty.working_revision(),0);
        let data=OpenOptions::new().create_new(true).read(true).write(true).mode(0o600).open(t.0.join("input")).unwrap();
        data.write_all_at(b"new",0).unwrap();data.sync_all().unwrap();
        w.save_first(OperationId([0xa7;16]),"文档",born.file,&data).unwrap();
        let(file,k2)=w.open_current_root(&r1,"文档").unwrap();assert_eq!(k1,k2);
        assert_eq!((file.file(),file.size(),file.working_revision()),(born.file,3,1));
        assert_eq!(empty.smb_read_window(0,3,|b|Ok(b.len())).unwrap(),0);
        assert!(r.open_current_root(&r1,"文档").is_err(),"cannot borrow even the same path from another observer");
        let web=src.authenticate_web("first.admin","correct horse battery staple").unwrap();
        empty.check_at_file(||{
            assert_ne!(unsafe{crate::system::reader::sqlite3_get_autocommit(observer.conn.0)},0);
            let off=web.set_share_mode(OperationId([0xa8;16]),rw,w.setting.op,Mode::Disabled)?;
            web.set_share_mode(OperationId([0xa9;16]),rw,off.op,Mode::ReadWrite)?;Ok(())
        }).unwrap(); // unrelated share did not stall or cancel this original read
        assert!(file.native_key().is_err(),"stale original RW never becomes active again");
        let dst=t.0.join("root-held-away");std::fs::rename(&f,&dst).unwrap();
        assert!(r.open_current_root(&r2,"文档").is_err());assert!(empty.native_key().is_err());
        std::fs::rename(&dst,&f).unwrap();let(later,k3)=r.open_current_root(&r2,"文档").unwrap();
        assert_eq!((later.size(),k1),(3,k3));
    }

}
