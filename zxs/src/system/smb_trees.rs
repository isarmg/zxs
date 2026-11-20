//! One bounded job owns an actual F directory and its original marker, its
//! own read-only SQLite connection, and the private endpoint of one TCON.
//! No host path, physical inode, block device or FILE descriptor crosses to smbd.
use super::{InitialReader,Stamp,attempts::TreeOrigin,denied,os};
use super::super::{SmbSession,share_mode::Setting};
use crate::{PoolId,fs::ControlRead,identity::ShareId,system::{lease::SystemDir,reader::volume::OrdinaryRoot}};
use std::{fs::File,io,os::fd::{AsRawFd,OwnedFd},sync::{Arc,atomic::{AtomicBool,Ordering}},thread};

const GLOBAL:usize=32;
const PER_SESSION:usize=8;
unsafe extern "C"{
    fn zxs_smb_tree_reply(fd:i32,result:*const u8)->i32;
    fn zxs_smb_root_wait(tree:i32,session:i32,timeout:i32)->i32;
    fn zxs_smb_process_live(pidfd:i32)->i32;
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
        unsafe{zxs_smb_root_wait(tree.as_raw_fd(),origin.session.as_raw_fd(),timeout)}==0
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
fn attach<'r>(session:&SmbSession<'r>,share:ShareId,after:impl FnOnce()->io::Result<()>)->io::Result<(OrdinaryRoot<'r>,[u8;74])>{
    let (tree,c)=session.select_with_catalog(share)?;
    let root=OrdinaryRoot::for_body(session.source,&c,tree.setting.volume)?;
    let bytes=choice(tree.setting,session.source.pool,root.ordinal)?;
    root.require_streaming_at(after)?;
    session.source.at(|db,c|tree.require(db,&c,false))?; // SQL authority only; no F path or marker I/O
    Ok((root,bytes))
}
fn serve(site:Site<'_>,origin:TreeOrigin,share:ShareId,reply:OwnedFd)->io::Result<()>{
    if origin.slot.pool!=site.pool||!live(&origin,&reply,0){return Err(denied());}
    site.validate()?;
    let src=InitialReader::alongside(site.dir,site.mark,site.pool,site.shape)?;
    let session=SmbSession{source:&src,original:origin.slot.original.clone()};
    let (root,bytes)=attach(&session,share,||site.validate())?;
    if !live(&origin,&reply,0){return Err(denied());}
    os(unsafe{zxs_smb_tree_reply(reply.as_raw_fd(),bytes.as_ptr())})?;
    // Idle is NOT a continuous positive permission. Keep this exact root
    // until DISCONNECT, rejected data, native session loss or controlled stop.
    // No OPEN/LIST/READ packet is implemented through the tree channel yet.
    while live(&origin,&reply,500){}
    drop(root);Ok(())
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
}
