//! Original member-scoped handoff worker. Native CLOSE or same-tree R
//! SET_INFO supplies the request; here a current SQL boundary and physical
//! source, verified target and exact original receipt decide its outcome.
use super::{InitialReader,Site,SmbSession,Tree,TreeOrigin,denied,live,os};
use crate::system::reader::{smb::{file_transfer::Handoff,first_work::{Current,MovedFile,MoveOwner}},
    file_location::Place,account::share_mode::{Mode,Setting}};
use crate::{OperationId,Role,display,hash::Sha256};
use std::{io,os::fd::{AsRawFd,BorrowedFd,FromRawFd,OwnedFd},sync::atomic::{AtomicBool,AtomicUsize,Ordering},thread};

static ACTIVE:AtomicUsize=AtomicUsize::new(0);
pub(super) struct Permit;
impl Permit{
    pub(super) fn take()->Option<Self>{
        ACTIVE.fetch_update(Ordering::AcqRel,Ordering::Acquire,|n|(n<8).then_some(n+1)).ok()?;
        Some(Self)
    }
}
impl Drop for Permit{fn drop(&mut self){ACTIVE.fetch_sub(1,Ordering::AcqRel);}}

unsafe extern "C"{
    fn zxs_member_xfer_pending(member:i32)->i32;
    fn zxs_xfer_take(member:i32,worker:i32,kind:*mut u8,folder:*mut u8,name:*mut u8,mark:*mut u8,parent_len:*mut u16,parents:*mut u8)->i32;
    fn zxs_xfer_reply(fd:i32,status:u8,file:*const u8,op:*const u8)->i32;
    fn zxs_member_held(member:i32)->i32;
}
pub(super) enum Request{
    Delete([u8;16]),Mark,Restore{folder:String,parents:Vec<String>,name:String},Rename(String),
    Move{folder:String,parents:Vec<String>,name:String},
    CopyMove{folder:String,parents:Vec<String>,name:String},
}
pub(super) fn pending(member:i32)->i32{unsafe{zxs_member_xfer_pending(member)}}
pub(super) fn take(member:i32,origin:&TreeOrigin)->io::Result<(Request,OwnedFd)>{
    let(mut kind,mut folder,mut name,mut mark)=(0,[0u8;256],[0u8;223],[0u8;16]);
    let(mut plen,mut raw)=(0,[0u8;4096]);
    let fd=unsafe{zxs_xfer_take(member,origin.worker.as_raw_fd(),&mut kind,folder.as_mut_ptr(),name.as_mut_ptr(),mark.as_mut_ptr(),&mut plen,raw.as_mut_ptr())};
    os(fd)?;
    let answer=unsafe{OwnedFd::from_raw_fd(fd)};
    let f=usize::from(folder[0]);let n=usize::from(name[0]);
    let ask=match kind{
        1 if f==0&&n==0&&plen==0&&mark!=[0;16]=>Request::Delete(mark),
        3 if f==0&&n==0&&plen==0=>Request::Mark,
        4 if f==0&&n>0&&n<=222&&plen==0&&mark==[0;16]=>Request::Rename(display::component(&name[1..1+n],222)?.to_owned()),
        code @ (2|5|6) if f>0&&n>0&&f<=255&&n<=222&&mark==[0;16]=>{
            let folder=display::component(&folder[1..1+f],255)?.to_owned();
            let parents=if plen==0{Vec::new()}else{super::super::super::ordinary_directory::components(raw.get(..usize::from(plen)).ok_or_else(denied)?)?};
            let name=display::component(&name[1..1+n],222)?.to_owned();
            match code{2=>Request::Restore{folder,parents,name},5=>Request::Move{folder,parents,name},
                _=>Request::CopyMove{folder,parents,name}}
        },
        _=>return Err(denied()),
    };
    Ok((ask,answer))
}
pub(super) fn refuse(answer:OwnedFd,why:u8){let _=unsafe{zxs_xfer_reply(answer.as_raw_fd(),why,std::ptr::null(),std::ptr::null())};}
fn reply(answer:OwnedFd,result:io::Result<(crate::FileId,OperationId)>){
    match result{
        Ok((file,op))=>{let _=unsafe{zxs_xfer_reply(answer.as_raw_fd(),0,file.0.as_ptr(),op.0.as_ptr())};}
        Err(e)=>refuse(answer,match e.kind(){
            io::ErrorKind::WouldBlock=>1,io::ErrorKind::StorageFull=>2,io::ErrorKind::AlreadyExists=>3,
            io::ErrorKind::NotFound=>4,io::ErrorKind::NotConnected=>6,io::ErrorKind::ConnectionAborted=>7,
            io::ErrorKind::Unsupported=>8,_=>5,
        }),
    }
}

/// Cross-filesystem copies never run on a folder's READ/enumeration owner.
/// A completed worker only returns one ready held destination and borrowed
/// authority to the *same* surviving native member owner. No raw-FD reuse
/// is considered identity, and no SQLite object crosses the thread.
pub(super) struct OrdinaryJob<'scope>{owner:MoveOwner,task:thread::ScopedJoinHandle<'scope,Complete>}
struct Complete{answer:OwnedFd,member:OwnedFd,result:io::Result<MovedFile>,_permit:Permit}
/// The local dispatcher can leave for reasons other than peer shutdown.
/// Drop this inside the scope before joining any borrowed worker.
pub(super) struct ExitOrdinaryMoves<'a>(pub(super) &'a AtomicBool);
impl Drop for ExitOrdinaryMoves<'_>{fn drop(&mut self){self.0.store(false,Ordering::Release);}}
fn check_dispatcher(dispatcher:&AtomicBool)->io::Result<()>{
    if dispatcher.load(Ordering::Acquire){Ok(())}else{Err(denied())}
}
fn still_dispatched(dispatcher:&AtomicBool,evidence:impl FnOnce()->io::Result<()>,peer:impl FnOnce()->io::Result<()>)
    ->io::Result<()>{
    check_dispatcher(dispatcher)?;
    evidence()?; // BOUND metadata may block; do not use yesterday's owner after it.
    check_dispatcher(dispatcher)?;
    peer()?; // only original PIDFD/socket polls and weak/atomic ownership
    check_dispatcher(dispatcher)
}
impl OrdinaryJob<'_>{
    pub(super) fn finished(&self)->bool{self.task.is_finished()}
    pub(super) fn belongs_to(&self,current:&Current<'_,'_,'_>)->bool{self.owner.is_member(current)}
    pub(super) fn finish(self,site:Site<'_>,origin:&TreeOrigin,branch:&OwnedFd,current:&mut Current<'_,'_,'_>){
        let Ok(done)=self.task.join()else{return;};
        let held=||self.owner.check().is_ok()&&!(site.stopping)()&&live(origin,branch,0)&&
            unsafe{zxs_member_held(done.member.as_raw_fd())}==0&&unsafe{zxs_member_held(done.answer.as_raw_fd())}==0;
        if !self.owner.is_member(current)||!held()||site.validate().is_err(){return;}
        let result=done.result.and_then(|target|current.adopt_moved_file(target)).map(|m|(m.to.file,m.op));
        // Adoption advances the sequence. Don't use the pre-adoption
        // sequence to prove the exact reply, but still require both peers.
        if site.validate().is_ok()&&!(site.stopping)()&&live(origin,branch,0)&&unsafe{zxs_member_held(done.member.as_raw_fd())}==0&&
            unsafe{zxs_member_held(done.answer.as_raw_fd())}==0{reply(done.answer,result);}
    }
}
pub(super) fn start_ordinary<'scope,'env:'scope>(scope:&'scope thread::Scope<'scope,'env>,site:Site<'env>,
    origin:&'env TreeOrigin,branch:&'env OwnedFd,dispatcher:&'env AtomicBool,member:i32,current:&mut Current<'_,'_,'_>,
    answer:OwnedFd,request:Request)->Option<OrdinaryJob<'scope>>{
    let Request::CopyMove{folder,parents,name}=request else{refuse(answer,5);return None;};
    let Some(permit)=Permit::take()else{refuse(answer,1);return None;};
    let Ok(opened)=current.begin_ordinary_move()else{refuse(answer,5);return None;};
    let owner=current.move_owner();let worker_owner=owner.clone();
    // SAFETY: the owner loop holds this exact endpoint through this call.
    let Ok(held)=unsafe{BorrowedFd::borrow_raw(member)}.try_clone_to_owned()else{refuse(answer,1);return None;};
    let Ok(terminal)=answer.try_clone()else{refuse(answer,1);return None;};
    let result=thread::Builder::new().name("zxs-ordinary-volume-handoff".into()).spawn_scoped(scope,move||{
        let still=||still_dispatched(dispatcher,||site.validate(),||{
            worker_owner.check()?;
            if (site.stopping)()||!live(origin,branch,0)||unsafe{zxs_member_held(held.as_raw_fd())}!=0||
               unsafe{zxs_member_held(terminal.as_raw_fd())}!=0{return Err(denied());}
            Ok(())
        });
        let result=(||->io::Result<MovedFile>{
            still()?;
            let src=InitialReader::alongside(site.dir,site.mark,site.pool,site.shape)?;
            let session=SmbSession{source:&src,original:origin.slot.original.clone()};
            let from=Tree{session:&session,setting:opened.grant,parent:opened.place.parent};
            let (target,parent)=src.at(|db,c|{
                from.require(db,&c,true)?;
                let target=site.plan.current(db,&c,&folder)?.filter(|g|g.mode==Mode::ReadWrite&&g.volume!=from.setting.volume)
                    .ok_or_else(denied)?;
                let route=super::super::super::ordinary_directory::Route::by_names(db,&c,target.volume,&parents)?;
                Ok((target,route.parent()))
            })?;
            let to=Tree{session:&session,setting:target,parent};
            let moved=from.transfer_ordinary_to(&to,&opened,name,OperationId::new()?,&still)?;
            to.prepare_moved_file(moved,&opened)
        })();
        Complete{answer:terminal,member:held,result,_permit:permit}
    });
    match result{Ok(task)=>Some(OrdinaryJob{owner,task}),Err(_)=>{refuse(answer,1);None}}
}
static RENAME_WORK:AtomicUsize=AtomicUsize::new(0);
pub(super) fn rename(site:Site<'_>,answer:OwnedFd,request:&Request,current:&mut Current<'_,'_,'_>,
    member_alive:&impl Fn()->io::Result<()>){
    if RENAME_WORK.fetch_update(Ordering::AcqRel,Ordering::Acquire,|n|(n<4).then_some(n+1)).is_err(){
        refuse(answer,1);return;
    }
    struct Release;
    impl Drop for Release{fn drop(&mut self){RENAME_WORK.fetch_sub(1,Ordering::AcqRel);}}
    let _release=Release;
    let live=||->io::Result<()>{
        site.validate()?;
        member_alive()?;
        if unsafe{zxs_member_held(answer.as_raw_fd())}!=0{return Err(denied());}
        Ok(())
    };
    let outcome=(||{
        live()?;
        let op=OperationId::new()?;
        match request{
            Request::Rename(name)=>current.rename_native(op,name,&live,|_|Ok(()))?,
            Request::Move{folder,parents,name}=>{
                current.require_move_folder(|db,c|site.plan.current(db,c,folder))?;
                current.move_native_parent(parents,op,name,&live,|_|Ok(()))?;
            },
            _=>return Err(denied()),
        }
        Ok((current.file(),op))
    })();
    if live().is_ok(){reply(answer,outcome);}
}

fn mark_for(auth:&SmbSession<'_>,source:&Place,wanted:Setting,nonce:[u8;8])->io::Result<OperationId>{
    if nonce==[0;8]{return Err(denied());}
    let mut h=Sha256::new();h.update(b"ZXS-NATIVE/DISPOSITION-1\0")?;
    // The stamp is private to the already pidfd-authorized member; it is
    // not a new login or on-wire capability. Its nonce distinguishes a
    // re-mark of the same link from an older still-closing native FSP.
    for v in [auth.source.pool.0,auth.original.instance,auth.original.account.0,auth.original.op.0,source.file.0,source.op.0,
        source.volume.0,source.parent.bytes(),wanted.share.0,wanted.op.0]{h.update(&v)?;}
    h.update(&auth.original.generation.to_be_bytes())?;h.update(&source.generation.to_be_bytes())?;
    h.update(&nonce)?;
    let mut stamp=[0u8;16];stamp[..8].copy_from_slice(&nonce);stamp[8..].copy_from_slice(&h.finish()[..8]);
    Ok(OperationId(stamp))
}
pub(super) fn run(site:Site<'_>,origin:&TreeOrigin,branch:&OwnedFd,answer:OwnedFd,request:Request,source:Place,
    ordinary:Option<Setting>,_permit:Permit){
    let still=||->io::Result<()>{
        if (site.stopping)()||!live(origin,branch,0)||unsafe{zxs_member_held(answer.as_raw_fd())}!=0{return Err(denied());}
        Ok(())
    };
    let result=(||{
        still()?;site.validate()?;
        let src=InitialReader::alongside(site.dir,site.mark,site.pool,site.shape)?;
        let auth=SmbSession{source:&src,original:origin.slot.original.clone()};
        let (tree,ask)=match request{
            Request::Mark if source.role==Role::File=>{
                let expected=ordinary.ok_or_else(denied)?;
                let tree=auth.connect_id(expected.share)?.scoped(source.parent);
                if tree.setting!=expected||expected.mode!=Mode::ReadWrite{return Err(denied());}
                let member=tree.open_current(&source.name)?;
                if *member.native_placement()!=source{return Err(denied());}
                member.native_key()?;still()?;
                // Proves the current original source to native disposition;
                // it does not issue an Op or touch the retained filesystem.
                let nonce=OperationId::new()?.0[..8].try_into().expect("disposition nonce width");
                return Ok((source.file,mark_for(&auth,&source,expected,nonce)?));
            },
            Request::Delete(mark) if source.role==Role::File=>{
                let expected=ordinary.ok_or_else(denied)?;
                let tree=auth.connect_id(expected.share)?.scoped(source.parent);
                let nonce=mark[..8].try_into().expect("disposition nonce width");
                if tree.setting!=expected||mark_for(&auth,&source,expected,nonce)?.0!=mark{return Err(denied());}
                (tree,Handoff::Delete(source.clone()))
            },
            Request::Restore{folder,parents,name} if source.role==Role::Recycle&&ordinary.is_none()=>{
                let setting=src.at(|db,c|{
                    auth.require_current(db,&c)?;
                    site.plan.current(db,&c,&folder)?.filter(|g|g.mode==Mode::ReadWrite).ok_or_else(denied)
                })?;
                let tree=Tree{session:&auth,setting,parent:crate::fs::directory_birth::DirectoryParent::Root};
                let (_,route)=tree.ordinary_route(&parents,true)?;
                (tree.scoped(route.parent()),Handoff::RestoreBound{source:source.clone(),target:name})
            },
            _=>return Err(denied()),
        };
        let op=OperationId::new()?;
        let moved=match ask{
            Handoff::Delete(source)=>tree.transfer_last_close(source,op,&still)?,
            other=>tree.transfer(&other,op,&still)?,
        };
        if moved.from!=source{return Err(io::ErrorKind::ConnectionAborted.into());}
        Ok((moved.to.file,moved.op))
    })();
    if still().is_err()||site.validate().is_err(){return;}
    reply(answer,result)
}

#[cfg(test)]
mod tests{
    use super::*;
    use crate::{system::reader::tests::transfer_pool,test_support::PrivateTemp};
    use std::path::Path;

    #[test]
    fn local_member_loop_drop_revokes_a_worker_before_scope_join_with_live_endpoints(){
        let active=AtomicBool::new(true);
        let (member,client)=std::os::unix::net::UnixStream::pair().unwrap();
        let (answer,request)=std::os::unix::net::UnixStream::pair().unwrap();
        thread::scope(|scope|{
            let close=ExitOrdinaryMoves(&active);
            let (unblock,wait)=std::sync::mpsc::channel::<()>();
            let active=&active;
            let job=scope.spawn(move||{
                wait.recv().unwrap();
                assert_eq!(unsafe{zxs_member_held(member.as_raw_fd())},0);
                assert_eq!(unsafe{zxs_member_held(answer.as_raw_fd())},0);
                assert!(check_dispatcher(active).is_err());
            });
            assert!(check_dispatcher(active).is_ok());
            drop(close);unblock.send(()).unwrap();job.join().unwrap();
            drop(client);drop(request);
        });
    }
    #[test]
    fn closing_during_bound_evidence_does_not_carry_old_admission_into_commit(){
        let active=AtomicBool::new(true);
        let end=ExitOrdinaryMoves(&active);
        let denied=still_dispatched(&active,||{drop(end);Ok(())},||panic!("expired owner reached the peer/terminal gate"));
        assert!(denied.is_err());
    }

    #[test]
    #[ignore="uses two true storage roots and independent SQLite connections, not an SMB service"]
    fn disposition_proofs_cannot_be_replayed_as_a_fresh_mark_or_after_regrant(){
        let private=PrivateTemp::under(Path::new("/tmp"));let other=PrivateTemp::under(Path::new("/dev/shm"));
        let (path,pool,_,share)=transfer_pool(&private,&other.0.join("actual-R"));
        let source=InitialReader::open(&path,pool).unwrap();let db=InitialReader::open(&path,pool).unwrap();
        let auth=SmbSession::fixture_authenticated(&source,"files.smb").unwrap();
        let tree=auth.connect(share).unwrap();
        let file=tree.create_file(OperationId([0xcb;16]),"to-retain.txt").unwrap().file;
        let place=source.at(|c,m|Place::current(c,&m,file)).unwrap();
        let old_read=tree.open_current("to-retain.txt").unwrap();
        let mark=mark_for(&auth,&place,tree.setting,[1;8]).unwrap();
        // A second FSP's newer mark cannot be cleared as though it were
        // the abandoned older FSP's marker (even for the same actor).
        assert_ne!(mark,mark_for(&auth,&place,tree.setting,[2;8]).unwrap());
        let mut other_place=place.clone();other_place.generation+=1;
        assert_ne!(mark,mark_for(&auth,&other_place,tree.setting,[1;8]).unwrap());
        let admin=db.authenticate_web("first.admin","correct horse battery staple").unwrap();
        admin.set_share_mode(OperationId([0xcc;16]),share,tree.setting.op,Mode::Disabled).unwrap();
        admin.set_share_mode(OperationId([0xcd;16]),share,OperationId([0xcc;16]),Mode::ReadWrite).unwrap();
        let reset=auth.connect(share).unwrap();
        assert_ne!(mark,mark_for(&auth,&place,reset.setting,[1;8]).unwrap());
        assert!(old_read.native_key().is_err());
        db.at(|db,_|{
            let q=db.prepare(c"SELECT count(*) FROM main.operation WHERE kind=12")?;
            assert!(crate::system::reader::work_record::start_one(&q)?);assert_eq!(q.integer(0)?,0);q.done()
        }).unwrap();
    }
}
