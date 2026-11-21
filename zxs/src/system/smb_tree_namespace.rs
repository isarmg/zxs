//! One authenticated SMB business root. Root listing uses only the original
//! plan and current SQL; no selected drive has to be online merely to list it.
use super::{InitialReader,Site,SmbSession,Tree,TreeOrigin,denied,live,os,serve_folder};
use super::super::reference_capacity::{Reference,pick_reference};
use super::super::super::share_mode::{Mode,Setting};
use crate::{display,fs::directory_birth::DirectoryParent,system::reader::{Catalog,volume::{Capacity,CapacityOrigin}}};
use std::{io,os::fd::{AsRawFd,FromRawFd,OwnedFd},sync::atomic::{AtomicBool,AtomicUsize,Ordering},thread};

const NAME:usize=255;
const SLOT:usize=1+NAME+16;
const ROW:usize=16;
const OPEN_READ:u8=253;
const OPEN_WRITE:u8=252;
const OPEN_MAX:u8=247;
pub(super) const CAPACITY_RW:u8=242;
pub(super) const CAPACITY_RO:u8=241;
const CAP_READY:u8=0;
const CAP_NO_MEDIA:u8=1;
const CAP_BUSY:u8=2;
static CAPACITY_JOBS:AtomicUsize=AtomicUsize::new(0);
static READS:AtomicUsize=AtomicUsize::new(0);
static WRITES:AtomicUsize=AtomicUsize::new(0);

unsafe extern "C"{
    fn zxs_smb_tree_reply(fd:i32,bytes:*const u8)->i32;
    fn zxs_smb_root_wait(tree:i32,session:i32,timeout:i32)->i32;
    fn zxs_dir_take(tree:i32,worker:i32,special:*mut u8,name:*mut u8)->i32;
    fn zxs_ns_dir_reply(fd:i32,body:*const u8)->i32;
    fn zxs_ns_capacity_reply(fd:i32,status:u8,total:u64,available:u64,free:u64)->i32;
}
struct Limit{used:&'static AtomicUsize}
impl Limit{
    fn acquire(write:bool)->Option<Self>{
        let used=if write{&WRITES}else{&READS};
        let ceiling=if write{64}else{192};
        used.fetch_update(Ordering::AcqRel,Ordering::Acquire,|n|(n<ceiling).then_some(n+1)).ok()?;
        Some(Self{used})
    }
}
impl Drop for Limit{fn drop(&mut self){self.used.fetch_sub(1,Ordering::AcqRel);}}

fn page(site:Site<'_>,session:&SmbSession<'_>,limit:u8,left:&[u8])->io::Result<[u8;1+ROW*SLOT]>{
    if limit as usize>ROW{return Err(denied());}
    if !left.is_empty(){display::component(left,NAME)?;}
    session.source.at(|db,c|{
        session.require_current(db,&c)?;
        let matches=if limit==0{Vec::new()}else{site.plan.project_visible_after(db,&c,left)?};
        let count=matches.len().min(limit as usize);let mut out=[0;1+ROW*SLOT];out[0]=count as u8;
        for ((name,ino),buf) in matches.into_iter().take(count).zip(out[1..].chunks_exact_mut(SLOT)){
            buf[0]=name.len() as u8;buf[1..1+name.len()].copy_from_slice(name);
            buf[1+NAME..1+NAME+8].copy_from_slice(&ino.to_be_bytes());
        }
        Ok(out)
    })
}
fn writer_class(role:u8,mode:Mode)->Option<bool>{
    match (role,mode){
        (_,Mode::Disabled)|(OPEN_WRITE,Mode::ReadOnly)=>None,
        (OPEN_WRITE|OPEN_MAX,Mode::ReadWrite)=>Some(true),
        (OPEN_READ|OPEN_MAX,_)=>Some(false),
        _=>None,
    }
}
fn selected(site:Site<'_>,session:&SmbSession<'_>,name:&[u8])->io::Result<Setting>{
    let name=display::component(name,NAME)?;
    session.source.at(|db,c|{
        session.require_current(db,&c)?;
        site.plan.current(db,&c,name)?.filter(|x|x.mode!=Mode::Disabled).ok_or_else(denied)
    })
}

struct CapacityPermit<'a>(&'a AtomicBool);
impl<'a> CapacityPermit<'a>{
    fn take(tcon:&'a AtomicBool)->Option<Self>{
        tcon.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).ok()?;
        if CAPACITY_JOBS.fetch_update(Ordering::AcqRel,Ordering::Acquire,|n|(n<2).then_some(n+1)).is_err(){
            tcon.store(false,Ordering::Release);return None;
        }
        Some(Self(tcon))
    }
}
impl Drop for CapacityPermit<'_>{fn drop(&mut self){
    CAPACITY_JOBS.fetch_sub(1,Ordering::AcqRel);self.0.store(false,Ordering::Release);
}}
fn no_capacity(fd:i32,status:u8){
    let _=unsafe{zxs_ns_capacity_reply(fd,status,0,0,0)};
}

/// The same single slot covers the original root and all folders of a
/// TCON. None of their read/write or enumeration workers waits on disk here.
pub(super) fn start_capacity<'scope,'env:'scope>(
    scope:&'scope thread::Scope<'scope,'env>,slot:&mut Option<thread::ScopedJoinHandle<'scope,()>>,
    site:Site<'env>,origin:&TreeOrigin,answer:OwnedFd,task:impl FnOnce(TreeOrigin,OwnedFd)+Send+'scope,
){
    let fd=answer.as_raw_fd();
    if slot.as_ref().is_some_and(|job|!job.is_finished()){no_capacity(fd,CAP_BUSY);return;}
    let Some(permit)=site.capacity_slot.and_then(CapacityPermit::take)else{no_capacity(fd,CAP_BUSY);return;};
    let Ok(peer)=origin.duplicate()else{no_capacity(fd,CAP_BUSY);return;};
    if let Ok(job)=thread::Builder::new().name("zxs-smb-capacity".into()).spawn_scoped(scope,move||{
        let _permit=permit;task(peer,answer);
    }){*slot=Some(job);}
}

impl Reference<'_>{
    fn confirm(&self,session:&SmbSession<'_>,catalog:&Catalog,validate:impl FnOnce()->io::Result<()>,
        live:impl FnOnce()->io::Result<()>)->io::Result<()>{
        if !self.root.for_tree(session.source,self.grant.volume){return Err(denied());}
        self.root.require_streaming()?;
        validate()?;
        let original=catalog.rows().iter().find(|r|r.volume==self.grant.volume).ok_or_else(denied)?;
        let exact=Tree{session,setting:self.grant,parent:DirectoryParent::Root};
        session.source.at(|db,c|{
            exact.require(db,&c,false)?;
            if c.rows().iter().find(|r|r.volume==self.grant.volume)!=Some(original){return Err(denied());}
            live()
        })
    }
}

fn reference_capacity(site:Site<'_>,origin:&TreeOrigin,answer:&OwnedFd,can_write:bool)->io::Result<()>{
    let fd=answer.as_raw_fd();
    let current=||->io::Result<()>{
        if (site.stopping)()||!live(origin,answer,0){return Err(denied());}
        Ok(())
    };
    current()?;site.validate()?;
    let src=InitialReader::alongside(site.dir,site.mark,site.pool,site.shape)?;
    let session=SmbSession{source:&src,original:origin.slot.original.clone()};
    let (catalog,choices)=src.at(|db,c|{
        session.require_current(db,&c)?;
        let selected=site.plan.folders().iter().map(|f|site.plan.current(db,&c,f.name())?.ok_or_else(denied))
            .collect::<io::Result<Vec<_>>>()?;
        Ok((c,selected))
    })?;
    let Some(chosen)=pick_reference(&src,&catalog,choices,can_write,&current)? else{
        site.validate()?;
        src.at(|db,c|{session.require_current(db,&c)?;current()})?;
        current()?;no_capacity(fd,CAP_NO_MEDIA);return Ok(());
    };
    chosen.confirm(&session,&catalog,||site.validate(),&current)?;
    current()?;
    // Keep the original F and marker through the private terminal reply.
    let b=&chosen.bytes;
    let result=os(unsafe{zxs_ns_capacity_reply(fd,CAP_READY,b.total,if chosen.writable{b.available}else{0},b.free)});
    drop(chosen);result
}

fn finish_ordinary_capacity(held:CapacityOrigin<'_>,sample:Option<Capacity>,
    before_cut:impl FnOnce()->io::Result<()>,cut:impl FnOnce()->io::Result<()>,
    send:impl FnOnce(Option<Capacity>)->io::Result<()>)->io::Result<()>{
    if sample.is_some(){held.require_held()?;}
    before_cut()?;
    // Original F/marker lookup can block. Never put it after the last
    // current-grant cut. Only a nonblocking liveness check and reply follow.
    cut()?;
    send(sample)
}
pub(super) fn ordinary_capacity(site:Site<'_>,origin:&TreeOrigin,answer:&OwnedFd,parent:&OwnedFd,
    active:&AtomicBool,held:CapacityOrigin<'_>,grant:Setting,can_write:bool)->io::Result<()>{
    let current=||->io::Result<()>{
        if !active.load(Ordering::Acquire)||(site.stopping)()||!live(origin,parent,0)||!live(origin,answer,0){
            return Err(denied());
        }
        Ok(())
    };
    current()?;site.validate()?;
    let src=InitialReader::alongside(site.dir,site.mark,site.pool,site.shape)?;
    let session=SmbSession{source:&src,original:origin.slot.original.clone()};
    let exact=Tree{session:&session,setting:grant,parent:DirectoryParent::Root};
    let cut=||src.at(|db,c|{
        exact.require(db,&c,false)?;held.require_catalog(&c,grant.volume)?;current()
    });
    cut()?;
    // No path supplied by the client or a fresh volume selected from SQL.
    // This uses the open F itself and the actual ordinary ancestry of the FSP.
    let sample=held.sample().ok().filter(|s|s.total>=4096);
    let fd=answer.as_raw_fd();
    finish_ordinary_capacity(held,sample,||site.validate(),cut,|found|{
        current()?;
        if let Some(b)=found{
            let writable=can_write&&grant.mode==Mode::ReadWrite&&!b.read_only;
            os(unsafe{zxs_ns_capacity_reply(fd,CAP_READY,b.total,if writable{b.available}else{0},b.free)})
        }else{
            no_capacity(fd,CAP_NO_MEDIA);Ok(())
        }
    })
}

pub(super) fn serve(site:Site<'_>,origin:TreeOrigin,root:OwnedFd)->io::Result<()>{
    if origin.slot.pool!=site.pool || (site.stopping)() || !live(&origin,&root,0){return Err(denied());}
    site.validate()?;
    let src=InitialReader::alongside(site.dir,site.mark,site.pool,site.shape)?;
    let session=SmbSession{source:&src,original:origin.slot.original.clone()};
    src.at(|db,c|session.require_current(db,&c))?;
    let mut start=[0;74];start[57..73].copy_from_slice(&site.pool.0);
    if (site.stopping)() || !live(&origin,&root,0){return Err(denied());}
    site.validate()?;os(unsafe{zxs_smb_tree_reply(root.as_raw_fd(),start.as_ptr())})?;
    // This local stop ends only the selected children of this one TCON.
    // It is set before joining any slow storage task.
    let running=AtomicBool::new(true);
    struct Stop<'a>(&'a AtomicBool);
    impl Drop for Stop<'_>{fn drop(&mut self){self.0.store(false,Ordering::Release);}}
    let alive=||{
        !running.load(Ordering::Acquire)||(site.stopping)()||!live(&origin,&root,0)
    };
    let capacity_slot=AtomicBool::new(false);
    let child=Site{stopping:&alive,capacity_slot:Some(&capacity_slot),..site};
    thread::scope(|scope|{
        let _stop=Stop(&running);
        let mut jobs=Vec::new();
        let mut capacity:Option<thread::ScopedJoinHandle<'_,()>>=None;
        loop{
            if alive(){break;}
            let status=unsafe{zxs_smb_root_wait(root.as_raw_fd(),origin.session.as_raw_fd(),500)};
            if status<0{break;}
            if status==0{continue;}
            let mut role=0;let mut name=[0;NAME+1];
            let fd=unsafe{zxs_dir_take(root.as_raw_fd(),origin.worker.as_raw_fd(),&mut role,name.as_mut_ptr())};
            if fd<0{break;}
            let answer=unsafe{OwnedFd::from_raw_fd(fd)};
            let label=&name[1..1+usize::from(name[0])];
            if matches!(role,CAPACITY_RO|CAPACITY_RW){
                if !label.is_empty(){continue;}
                start_capacity(scope,&mut capacity,child,&origin,answer,move|peer,answer|{
                    let _=reference_capacity(child,&peer,&answer,role==CAPACITY_RW);
                });
            }else if matches!(role,OPEN_READ|OPEN_WRITE|OPEN_MAX){
                let (grant,writer)=if label==site.plan.retained_label().as_bytes(){
                    if role==OPEN_WRITE{continue;}
                    (None,false)
                }else{
                    let Ok(grant)=selected(site,&session,label)else{continue;};
                    let Some(writer)=writer_class(role,grant.mode)else{continue;};
                    (Some(grant),writer)
                };
                jobs.retain(|(_,h):&(bool,thread::ScopedJoinHandle<'_,()>)|!h.is_finished());
                if jobs.iter().filter(|(w,_)|*w==writer).count()>=(if writer{16}else{64}){continue;}
                let Some(permit)=Limit::acquire(writer)else{continue;};
                let peer=origin.duplicate()?;
                if let Ok(job)=thread::Builder::new().name("zxs-planned-folder".into()).spawn_scoped(scope,move||{
                    let _permit=permit;
                    if let Some(grant)=grant{let _=serve_folder(child,peer,grant,writer,answer);}
                    else{let _=super::retained::serve(child,peer,answer);}
                }){jobs.push((writer,job));}
            }else{
                let data=page(site,&session,role,label);
                if alive() || site.validate().is_err(){break;}
                if let Ok(bytes)=data{let _=unsafe{zxs_ns_dir_reply(answer.as_raw_fd(),bytes.as_ptr())};}
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests{
    use super::*;
    use crate::system::reader::volume::OrdinaryRoot;
    #[test]
    #[ignore="source-only: fresh SQLite fixture and unprivileged isolated ordinary F; no listener"]
    fn real_reference_uses_one_confirmed_volume_and_rejects_a_late_change_of_grant(){
        use crate::{OperationId,fs::HeldDir,system::reader::{ordinary_pool,sqlite3_get_autocommit},test_support::PrivateTemp};
        let tmp=PrivateTemp::under(&std::env::temp_dir());
        let(path,pool,f,rw,ro)=ordinary_pool(&tmp);
        let src=InitialReader::open(&path,pool).unwrap();
        let original=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();
        let (_,c)=original.select_with_catalog(rw).unwrap();
        // Two known grants address the same F in this fixture. It must never
        // be counted twice. Reference sampling does not take its parent lock.
        let rw=original.connect(rw).unwrap().setting;
        let ro=original.connect(ro).unwrap().setting;
        let peer=HeldDir::open(&f).unwrap();peer.exclusive().unwrap();
        let ready=||{assert_eq!(unsafe{sqlite3_get_autocommit(src.conn.0)},1);Ok(())};
        let writable=pick_reference(&src,&c,vec![ro,rw],true,&ready).unwrap().unwrap();
        assert_eq!(writable.grant,rw);assert!(writable.writable);
        assert!(writable.bytes.available<=writable.bytes.free&&writable.bytes.free<=writable.bytes.total);
        let locked=pick_reference(&src,&c,vec![ro,rw],false,&ready).unwrap().unwrap();
        assert_eq!(locked.grant,ro);assert!(!locked.writable);
        assert_eq!(writable.bytes.total,locked.bytes.total);
        writable.confirm(&original,&c,&ready,||Ok(())).unwrap();
        let web=src.authenticate_web("first.admin","correct horse battery staple").unwrap();
        web.set_share_mode(OperationId([0xaf;16]),rw.share,rw.op,Mode::Disabled).unwrap();
        assert!(writable.confirm(&original,&c,&ready,||Ok(())).is_err());
        drop(peer);
        let gone=tmp.0.join("F-offline");std::fs::rename(&f,&gone).unwrap();
        assert!(pick_reference(&src,&c,vec![ro],true,&ready).unwrap().is_none());
        std::fs::rename(&gone,&f).unwrap();
    }

    #[test]
    #[ignore="source only: original nested F and real name replacement; no SMB listener"]
    fn ordinary_capacity_reuses_held_ancestry_and_never_a_fresh_reference(){
        use crate::{OperationId,system::reader::ordinary_pool,test_support::PrivateTemp};
        let tmp=PrivateTemp::under(&std::env::temp_dir());
        let (path,pool,f,rw,_)=ordinary_pool(&tmp);
        let src=InitialReader::open(&path,pool).unwrap();
        let session=SmbSession::fixture_authenticated(&src,"files.smb").unwrap();
        let tree=session.connect(rw).unwrap();
        let (_,base)=tree.ordinary_route(&[],true).unwrap();
        tree.make_ordinary_directory(&base,OperationId([0xd3;16]),"Reports",|_|Ok(()),&||Ok(()),|_|Ok(())).unwrap();
        let (catalog,route)=tree.ordinary_route(&["Reports".into()],false).unwrap();
        let root=OrdinaryRoot::at_parent(&src,&catalog,&route,None).unwrap();
        let holder=root.capacity_origin();holder.require_catalog(&catalog,tree.setting.volume).unwrap();
        assert!(holder.sample().unwrap().total>=4096);
        let away=f.join("Reports-away");
        std::fs::rename(f.join("Reports"),&away).unwrap();
        assert!(holder.sample().is_err());
        assert!(pick_reference(&src,&catalog,vec![tree.setting],true,||Ok(())).unwrap().is_some());
        std::fs::rename(&away,f.join("Reports")).unwrap();
        let cut=||src.at(|db,c|{tree.require(db,&c,false)?;holder.require_catalog(&c,tree.setting.volume)});
        let sample=holder.sample().unwrap();
        let replied=std::cell::Cell::new(false);
        finish_ordinary_capacity(holder,Some(sample),||Ok(()),&cut,|result|{
            assert!(result.is_some());replied.set(true);Ok(())
        }).unwrap();
        assert!(replied.replace(false));
        let observer=InitialReader::alongside(&src.dir,&src.marker,pool,src.expected_schema).unwrap();
        let admin=observer.authenticate_web("first.admin","correct horse battery staple").unwrap();
        let sample=holder.sample().unwrap();
        let changed=std::cell::Cell::new(false);
        assert!(finish_ordinary_capacity(holder,Some(sample),||{
            admin.set_share_mode(OperationId([0xd4;16]),rw,tree.setting.op,Mode::ReadOnly)?;
            changed.set(true);Ok(())
        },cut,|_|{replied.set(true);Ok(())}).is_err());
        assert!(changed.get()&&!replied.get(),"never invoke the private reply after the original grant changed");
    }

    #[test]
    fn maximum_on_read_only_folder_uses_separate_capacity(){
        let writes:Vec<_>=(0..64).map(|_|Limit::acquire(true).unwrap()).collect();
        assert!(Limit::acquire(true).is_none());
        let ro=writer_class(OPEN_MAX,Mode::ReadOnly).unwrap();
        let read=Limit::acquire(ro).unwrap();
        assert!(!ro);
        assert_eq!(writer_class(OPEN_WRITE,Mode::ReadOnly),None);
        assert_eq!(writer_class(OPEN_MAX,Mode::ReadWrite),Some(true));
        drop(read);drop(writes);
    }
}
