//! One bounded job owns an actual F directory and its original marker, its
//! own read-only SQLite connection, and the private endpoint of one TCON.
//! No host path, physical inode, block device or FILE descriptor crosses to smbd.
use super::{InitialReader,Stamp,attempts::TreeOrigin,denied,os};
use super::super::{SmbSession,Tree,directory,share_mode::Setting};
use crate::{OperationId,PoolId,fs::{ControlRead,directory_birth::DirectoryParent},identity::ShareId,system::{lease::SystemDir,reader::volume::OrdinaryRoot}};
use std::{cell::Cell,fs::File,io,os::fd::{AsRawFd,FromRawFd,OwnedFd},sync::{Arc,atomic::{AtomicBool,AtomicUsize,Ordering}},thread};

#[path="smb_tree_writer.rs"]
mod writer;
#[path="smb_birth_worker.rs"]
mod birth;
#[path="smb_move_job.rs"]
mod mover;
#[path="smb_retained_tree.rs"]
mod retained;
#[path="smb_tree_namespace.rs"]
mod namespace;

const GLOBAL:usize=32;
const PER_SESSION:usize=8;
unsafe extern "C"{
    fn zxs_smb_tree_reply(fd:i32,result:*const u8)->i32;
    fn zxs_smb_root_wait(tree:i32,session:i32,timeout:i32)->i32;
    fn zxs_smb_process_live(pidfd:i32)->i32;
    fn zxs_dir_take(tree:i32,worker:i32,limit:*mut u8,cursor:*mut u8)->i32;
    fn zxs_odir_rows(fd:i32,page:*const u8)->i32;
    fn zxs_odir_take(fd:i32,mode:*mut u8,len:*mut u16,path:*mut u8)->i32;
    fn zxs_odir_reply(fd:i32,code:u8,basics:*const u8,id:*const u8,op:*const u8)->i32;
    fn zxs_odir_take_ack(fd:i32,id:*const u8,op:*const u8)->i32;
    fn zxs_odir_done(fd:i32,id:*const u8,op:*const u8)->i32;
    fn zxs_member_reply(fd:i32,info:*const u8)->i32;
    fn zxs_tree_events(tree:i32,session:i32,members:*const i32,n:usize,events:*mut u8)->i32;
    fn zxs_member_held(member:i32)->i32;
    fn zxs_read_take(member:i32,worker:i32,offset:*mut u64,count:*mut u32)->i32;
    fn zxs_read_fill(fd:i32,data:*const u8,len:usize)->i32;
    fn zxs_read_accept(fd:i32)->i32;
}
#[derive(Clone,Copy)]
pub(super) struct Site<'a>{dir:&'a SystemDir,mark:&'a ControlRead,pool:PoolId,shape:[u8;32],binding:&'a File,stamp:&'a Stamp,stopping:&'a(dyn Fn()->bool+Sync),plan:&'a super::first::InstalledPlan}
impl<'a> Site<'a>{
    pub(super) fn from_reader(src:&'a InitialReader,binding:&'a File,stamp:&'a Stamp,stopping:&'a(dyn Fn()->bool+Sync),plan:&'a super::first::InstalledPlan)->Self{
        Self{dir:&src.dir,mark:&src.marker,pool:src.pool,shape:src.expected_schema,binding,stamp,stopping,plan}
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
        if share.0!=[0;16]{return;} // old per-share TREE_CONNECT has no product route
        self.jobs.retain(|(_,h)|!h.is_finished());
        if self.jobs.len()>=GLOBAL || self.jobs.iter().filter(|(owner,_)|Arc::ptr_eq(owner,&origin.alive)).count()>=PER_SESSION{
            return; // fail just this tree; no SQL, F, or per-session slot
        }
        let owner=origin.alive.clone();let site=self.site;
        let job=thread::Builder::new().name("zxs-f-root".into()).spawn_scoped(self.scope,move||{
            let _=namespace::serve(site,origin,reply);
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
fn member_packet(h:&super::super::first_work::Current<'_,'_,'_>,key:super::super::file_number::NativeKey)->[u8;88]{
    let (p,v,e,f)=h.original_ids();let mut bytes=[0u8;88];
    for(i,id)in [p.0,v.0,e.0,f.0].iter().enumerate(){bytes[16*i..16*i+16].copy_from_slice(id);}
    for(i,n)in [key.words().1,h.working_revision(),h.size()].iter().enumerate(){
        bytes[64+8*i..72+8*i].copy_from_slice(&n.to_be_bytes());
    }
    bytes
}
fn serve_folder(site:Site<'_>,origin:TreeOrigin,expected:Setting,writer_class:bool,reply:OwnedFd)->io::Result<()>{
    if origin.slot.pool!=site.pool||(site.stopping)()||!live(&origin,&reply,0){return Err(denied());}
    site.validate()?;
    let src=InitialReader::alongside(site.dir,site.mark,site.pool,site.shape)?;
    let session=SmbSession{source:&src,original:origin.slot.original.clone()};
    let (root,bytes,setting)=attach(&session,expected.share,||site.validate())?;
    if expected!=setting||(site.stopping)(){return Err(denied());}
    if !live(&origin,&reply,0){return Err(denied());}
    os(unsafe{zxs_smb_tree_reply(reply.as_raw_fd(),bytes.as_ptr())})?;
    let tree=Tree{session:&session,setting,parent:DirectoryParent::Root};
    let tree_live=AtomicBool::new(true);
    struct Clear<'a>(&'a AtomicBool);
    impl Drop for Clear<'_>{fn drop(&mut self){self.0.store(false,Ordering::Release);}}
    thread::scope(|scope|drive(scope,site,&origin,&reply,&root,&tree,writer_class,&tree_live,Clear(&tree_live)))
}

const ORDINARY_PATH:u8=243;
const SELECT_EXISTING_KIND:u8=4;
const ORDINARY_FILE_KIND:u8=7;
const PATH_MAX:usize=4096;
static FOLDER_READS:AtomicUsize=AtomicUsize::new(0);
static FOLDER_WRITES:AtomicUsize=AtomicUsize::new(0);
struct PathPermit(&'static AtomicUsize);
impl PathPermit{
    fn obtain(write:bool)->Option<Self>{
        let (used,cap)=if write{(&FOLDER_WRITES,32)}else{(&FOLDER_READS,64)};
        used.fetch_update(Ordering::AcqRel,Ordering::Acquire,|n|(n<cap).then_some(n+1)).ok()?;
        Some(Self(used))
    }
}
impl Drop for PathPermit{fn drop(&mut self){self.0.fetch_sub(1,Ordering::AcqRel);}}
fn serve_nested(site:Site<'_>,origin:&TreeOrigin,top:&OwnedFd,top_live:&AtomicBool,expected:Setting,
    writer_class:bool,reply:OwnedFd,_permit:PathPermit)->io::Result<()>{
    let fd=reply.as_raw_fd();
    let live_parent=||!top_live.load(Ordering::Acquire)||(site.stopping)()||!live(origin,top,0);
    let alive=||->io::Result<()>{
        if live_parent() || !live(origin,&reply,0){return Err(denied());}
        Ok(())
    };
    let offered=Cell::new(false);
    let prepare=||->io::Result<_>{
        alive()?;site.validate()?;
        let (mut mode,mut len)=(0,0);
        let mut raw=[0;PATH_MAX];
        os(unsafe{zxs_odir_take(fd,&mut mode,&mut len,raw.as_mut_ptr())})?;
        let raw=raw.get(..usize::from(len)).ok_or_else(denied)?;
        let names=super::super::ordinary_directory::components(raw)?;
        Ok((mode,names))
    };
    let result=(||->io::Result<()>{
        let (mode,names)=prepare()?;
        let src=InitialReader::alongside(site.dir,site.mark,site.pool,site.shape)?;
        let auth=SmbSession{source:&src,original:origin.slot.original.clone()};
        let tree=auth.connect_id(expected.share)?;
        if tree.setting!=expected{return Err(denied());}
        let (catalog,route)=if mode==0{
            tree.ordinary_route(&names,writer_class)?
        }else if mode==SELECT_EXISTING_KIND{
            use super::super::ordinary_directory::NamedKind;
            match tree.ordinary_kind(&names)?{
                (catalog,NamedKind::Directory(route))=>(catalog,route),
                (_,NamedKind::File)=>{
                    site.validate()?;alive()?;
                    return os(unsafe{zxs_odir_reply(fd,ORDINARY_FILE_KIND,std::ptr::null(),std::ptr::null(),std::ptr::null())});
                }
            }
        }else{
            let (leaf,parents)=names.split_last().ok_or_else(denied)?;
            let (c,parent)=tree.ordinary_route(parents,mode==2)?;
            let existing=tree.ordinary_route(&names,false);
            let may_make=mode==2||mode==3;
            match existing{
                Ok(found) if mode==1||mode==3=>found,
                Ok(_)=>return Err(io::ErrorKind::AlreadyExists.into()),
                Err(e) if may_make&&matches!(e.kind(),io::ErrorKind::NotFound|io::ErrorKind::WouldBlock)=>(||{
                    let ordinary=OrdinaryRoot::at_parent(&src,&c,&parent,None)?;
                    let info=choice(expected,src.pool,ordinary.ordinal)?;
                    let scoped=tree.scoped(parent.parent());
                    let op=match scoped.resumable_directory(&parent,leaf)?{Some(original)=>original,None=>OperationId::new()?};
                    let (born,_)=scoped.make_ordinary_directory(&parent,op,leaf,|x|{
                        alive()?;site.validate()?;
                        os(unsafe{zxs_odir_reply(fd,1,info.as_ptr(),x.id.0.as_ptr(),x.original.0.as_ptr())})?;
                        offered.set(true);
                        os(unsafe{zxs_odir_take_ack(fd,x.id.0.as_ptr(),x.original.0.as_ptr())})
                    },&alive,|_|Ok(()))?;
                    if !offered.get()&&mode==2{return Err(io::ErrorKind::AlreadyExists.into());}
                    let (cur,actual)=tree.ordinary_route(&names,false)?;
                    if actual.selected()!=Some(&born){return Err(denied());}
                    Ok((cur,actual))
                })().or_else(|e:io::Error|{
                    // Another request may have confirmed between the two
                    // original observations. OPEN_IF may now open that exact
                    // directory, but cannot return CREATED a second time.
                    if mode==3&&!offered.get()&&e.kind()==io::ErrorKind::AlreadyExists{
                        tree.ordinary_route(&names,false).or(Err(e))
                    }else{Err(e)}
                })?,
                Err(e)=>return Err(e),
            }
        };
        let root=OrdinaryRoot::at_parent(&src,&catalog,&route,None)?;
        let registered=route.selected().ok_or_else(denied)?;
        let brief=choice(expected,src.pool,root.ordinal)?;
        let selected=tree.scoped(route.parent());
        root.require_streaming()?;
        src.at(|db,c|selected.require(db,&c,false))?;
        site.validate()?;alive()?;
        if offered.get(){
            os(unsafe{zxs_odir_done(fd,registered.id.0.as_ptr(),registered.original.0.as_ptr())})?;
        }else{
            os(unsafe{zxs_odir_reply(fd,0,brief.as_ptr(),registered.id.0.as_ptr(),registered.original.0.as_ptr())})?;
        }
        let active=AtomicBool::new(true);
        struct Clear<'a>(&'a AtomicBool);
        impl Drop for Clear<'_>{fn drop(&mut self){self.0.store(false,Ordering::Release);}}
        let path_site=Site{stopping:&live_parent,..site};
        thread::scope(|scope|drive(scope,path_site,origin,&reply,&root,&selected,writer_class,&active,Clear(&active)))
    })();
    if let Err(e)=&result{
        if !offered.get()&&alive().is_ok()&&site.validate().is_ok(){
            let reason=match e.kind(){
                io::ErrorKind::AlreadyExists=>2,io::ErrorKind::WouldBlock=>3,
                io::ErrorKind::StorageFull=>5,io::ErrorKind::NotFound=>6,_=>4,
            };
            let _=unsafe{zxs_odir_reply(fd,reason,std::ptr::null(),std::ptr::null(),std::ptr::null())};
        }
    }
    result
}

fn drive<'scope,'env:'scope>(scope:&'scope thread::Scope<'scope,'env>,site:Site<'env>,
    origin:&'env TreeOrigin,reply:&'env OwnedFd,root:&'env OrdinaryRoot<'_>,tree:&Tree<'_,'_>,writer_class:bool,
    tree_live:&'env AtomicBool,_clear:impl Drop)->io::Result<()>{
    // The drop guard ends writer admission before scoped threads are joined.
    // Compatible readers keep their original bounded dispatcher.
    let mut members=Vec::<(OwnedFd,super::super::first_work::Current<'_,'_,'_>)>::new();
    const MAX_MEMBERS:usize=16;
    let mut writers=Vec::new();let mut births=Vec::new();let mut moves=Vec::new();let mut paths=Vec::new();
    while !(site.stopping)()&&live(origin,reply,0){
        let mut channels=[-1;MAX_MEMBERS];let mut events=[0u8;MAX_MEMBERS+1];
        for(i,(m,_))in members.iter().enumerate(){channels[i]=m.as_raw_fd();}
        if unsafe{zxs_tree_events(reply.as_raw_fd(),origin.session.as_raw_fd(),channels.as_ptr(),members.len(),events.as_mut_ptr())}<0{break;}
        // One native FSP owns one exact Current. Replies have independent stream
        // endpoints: one incomplete/error response cannot become another READ.
        for i in (0..members.len()).rev(){
            if !live(origin,reply,0)||(site.stopping)(){return Err(denied());}
            match events[i+1]{
                0=>continue,2=>{members.remove(i);continue;},1=>{},_=>return Err(denied())
            }
            let member=members[i].0.as_raw_fd();
            let action=mover::pending(member);
            if action<0{members.remove(i);continue;}
            if action==1{
                let Ok((request,answer))=mover::take(member,origin)else{members.remove(i);continue;};
                if !matches!(&request,mover::Request::Delete(_)|mover::Request::Mark){mover::refuse(answer,5);continue;}
                moves.retain(|job:&thread::ScopedJoinHandle<'_,()>|!job.is_finished());
                let Some(permit)=mover::Permit::take()else{mover::refuse(answer,1);continue;};
                let source=members[i].1.native_placement().clone();let authority=tree.setting;
                if let Ok(job)=thread::Builder::new().name("zxs-delete-to-recycle".into()).spawn_scoped(scope,move||{
                    mover::run(site,origin,reply,answer,request,source,Some(authority),permit);
                }){moves.push(job);}
                continue;
            }
            let (mut offset,mut count)=(0,0);
            let fd=unsafe{zxs_read_take(member,origin.worker.as_raw_fd(),&mut offset,&mut count)};
            if fd<0{members.remove(i);continue;} // malformed or dead FSP: retire only this member
            let answer=unsafe{OwnedFd::from_raw_fd(fd)};
            if site.validate().is_err()||unsafe{zxs_member_held(member)}!=0{continue;}
            let checked=members[i].1.smb_read_window(offset,count,|bytes|
                os(unsafe{zxs_read_fill(answer.as_raw_fd(),bytes.as_ptr(),bytes.len())}));
            // The private stream may contain tentative bytes, but its positive
            // trailer is sent only after the *post*-copy original F/SQL checks.
            if checked.is_ok() && !(site.stopping)() && site.validate().is_ok() && live(&origin,&reply,0) && unsafe{zxs_member_held(member)}==0{
                let _=unsafe{zxs_read_accept(answer.as_raw_fd())};
            }
        }
        if events[0]==0{continue;}
        let mut limit=0;let mut cursor=[0;256];
        let fd=unsafe{zxs_dir_take(reply.as_raw_fd(),origin.worker.as_raw_fd(),&mut limit,cursor.as_mut_ptr())};
        if fd<0{break;} // original session remains usable on its other trees
        let answer=unsafe{OwnedFd::from_raw_fd(fd)};
        if limit==ORDINARY_PATH{
            if tree.parent!=DirectoryParent::Root{continue;}
            paths.retain(|h:&thread::ScopedJoinHandle<'_,()>|!h.is_finished());
            if paths.len()>=16{continue;}
            let Some(permit)=PathPermit::obtain(writer_class)else{continue;};
            let expected=tree.setting;
            if let Ok(job)=thread::Builder::new().name("zxs-ordinary-directory".into()).spawn_scoped(scope,move||{
                let _=serve_nested(site,origin,reply,tree_live,expected,writer_class,answer,permit);
            }){paths.push(job);}
            continue;
        }
        if (birth::BEGIN..=birth::END).contains(&limit){
            let flags=limit-birth::BEGIN;let writer=flags&2!=0;
            if writer&&!writer_class{continue;}
            births.retain(|(_,h):&(bool,thread::ScopedJoinHandle<'_,()>)|!h.is_finished());
            if births.iter().filter(|(w,_)|*w==writer).count()>=(if writer{8}else{16}){continue;}
            let Some(permit)=birth::Permit::acquire(writer)else{continue;};
            let d=root.folder();let expected=tree.setting;let parent=tree.parent;
            if let Ok(job)=thread::Builder::new().name("zxs-file-birth".into()).spawn_scoped(scope,move||{
                let _=birth::serve(site,origin,reply,tree_live,expected,parent,d,answer,cursor,flags,permit);
            }){births.push((writer,job));}
            continue;
        }
        if limit==writer::OPEN{
            writers.retain(|h:&thread::ScopedJoinHandle<'_,()>|!h.is_finished());
            if !writer_class||writers.len()>=8||tree.setting.mode!=super::super::share_mode::Mode::ReadWrite||
               (site.stopping)()||!live(origin,reply,0){continue;}
            let Some(permit)=writer::Permit::acquire()else{continue;};
            let d=root.folder();let expected=tree.setting;let parent=tree.parent;
            if let Ok(job)=thread::Builder::new().name("zxs-file-writer".into()).spawn_scoped(scope,move||{
                let _=writer::serve(site,origin,reply,tree_live,expected,parent,d,answer,cursor,permit);
            }){writers.push(job);}
            continue;
        }
        if limit==255{
            if members.len()>=MAX_MEMBERS || site.validate().is_err() || !live(&origin,&reply,0){continue;}
            let Ok(name)=crate::display::component(&cursor[1..1+usize::from(cursor[0])],222) else{continue;};
            let Ok((handle,key))=tree.open_current_root(&root,name) else{continue;};
            let data=member_packet(&handle,key);
            if (site.stopping)()||!live(&origin,&reply,0)||site.validate().is_err(){break;}
            if unsafe{zxs_member_reply(answer.as_raw_fd(),data.as_ptr())}==0{members.push((answer,handle));}
            continue;
        }
        let result=(||->io::Result<_>{
            site.validate()?;
            if !live(&origin,&reply,0){return Err(denied());}
            if limit==0{
                root.require_streaming()?;
                tree.session.source.at(|db,c|tree.require(db,&c,false))?;
                return Ok(Vec::new());
            }
            directory::mixed_page(tree,root,&cursor[1..1+usize::from(cursor[0])],limit,||Ok(()))
        })();
        let Ok(rows)=result else{continue;}; // close only this answer, never send a prefix
        if (site.stopping)()||!live(&origin,&reply,0) || site.validate().is_err(){break;}
        let mut out=[0u8;1+16*(1+255+8+8)];out[0]=rows.len() as u8;
        for (row,slot) in rows.iter().zip(out[1..].chunks_exact_mut(272)){
            let name=row.name();slot[0]=name.len() as u8;slot[1..1+name.len()].copy_from_slice(name.as_bytes());
            match row{
                directory::Child::Directory(_)=>{slot[256..264].fill(0xff);slot[264]=0x80;},
                directory::Child::File(f)=>{slot[256..264].copy_from_slice(&f.number.to_be_bytes());slot[264..272].copy_from_slice(&f.size.to_be_bytes());}
            }
        }
        let _=unsafe{zxs_odir_rows(answer.as_raw_fd(),out.as_ptr())}; // cancel lost no cursor, grant or data
    }
    Ok(())
}

#[cfg(test)]
mod tests{
    use super::*;
    use crate::{OperationId,system::reader::{ordinary_pool,account::share_mode::Mode},test_support::PrivateTemp};
    #[test]
    fn ordinary_path_write_budget_cannot_consume_compatible_reader_budget(){
        let writers:Vec<_>=(0..32).map(|_|PathPermit::obtain(true).unwrap()).collect();
        assert!(PathPermit::obtain(true).is_none());
        let readers:Vec<_>=(0..64).map(|_|PathPermit::obtain(false).unwrap()).collect();
        assert!(PathPermit::obtain(false).is_none());
        drop(writers);assert!(PathPermit::obtain(true).is_some());
        drop(readers);assert!(PathPermit::obtain(false).is_some());
    }
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
