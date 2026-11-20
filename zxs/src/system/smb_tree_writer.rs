//! One actual private writer FSP. No FILE lock, scratch, or wait
//! is shared with the tree's compatible readers.
use super::{mover,Site,TreeOrigin,SmbSession,Tree,InitialReader,denied,live,member_packet,os};
use super::super::super::{smb_stage::Staged,share_mode::{Mode,Setting}};
use crate::{display,fs::{HeldDir,invalid,directory_birth::DirectoryParent}};
use std::{io,os::fd::{AsRawFd,FromRawFd,OwnedFd},sync::atomic::{AtomicBool,AtomicUsize,Ordering}};

const MAX_WRITERS:usize=32;
const MAX_BYTES:usize=64*1024*1024;
const READ:u8=1;
const WRITE:u8=2;
const EOF:u8=3;
const FLUSH:u8=4;
pub(super) const OPEN:u8=254;
static WRITERS:AtomicUsize=AtomicUsize::new(0);
static BYTES:AtomicUsize=AtomicUsize::new(0);

unsafe extern "C"{
    fn zxs_smb_root_wait(file:i32,session:i32,timeout:i32)->i32;
    fn zxs_member_held(member:i32)->i32;
    fn zxs_member_reply(fd:i32,info:*const u8)->i32;
    fn zxs_work_take(member:i32,worker:i32,command:*mut u8,offset:*mut u64,count:*mut u32,settle:*mut u8)->i32;
    fn zxs_work_input(fd:i32,data:*mut u8,count:usize)->i32;
    fn zxs_work_answer_live(fd:i32)->i32;
    fn zxs_work_reply(fd:i32,status:u8,eof:u64)->i32;
    fn zxs_read_fill(fd:i32,data:*const u8,count:usize)->i32;
    fn zxs_read_accept(fd:i32)->i32;
}

struct Limit{used:&'static AtomicUsize,held:usize}
impl Limit{
    fn take(used:&'static AtomicUsize,held:usize,cap:usize)->Option<Self>{
        used.fetch_update(Ordering::AcqRel,Ordering::Acquire,|n|n.checked_add(held).filter(|n|*n<=cap)).ok()?;
        Some(Self{used,held})
    }
}
impl Drop for Limit{
    fn drop(&mut self){self.used.fetch_sub(self.held,Ordering::AcqRel);}
}
pub(super) struct Permit{_slot:Limit}
impl Permit{
    pub(super) fn acquire()->Option<Self>{Limit::take(&WRITERS,1,MAX_WRITERS).map(|slot|Self{_slot:slot})}
}

fn status(e:&io::Error)->u8{
    match e.kind(){
        io::ErrorKind::WouldBlock=>2,
        io::ErrorKind::StorageFull|io::ErrorKind::OutOfMemory=>3,
        io::ErrorKind::InvalidInput=>4,
        _=>1,
    }
}
pub(super) fn serve(site:Site<'_>,origin:&TreeOrigin,tree_peer:&OwnedFd,tree_live:&AtomicBool,
    expected:Setting,parent:DirectoryParent,root:&HeldDir,member:OwnedFd,cursor:[u8;256],_permit:Permit)->io::Result<()>{
    if expected.mode!=Mode::ReadWrite{return Err(denied());}
    let fd=member.as_raw_fd();
    let active=||{
        // Also used at the last WRITE SQL cut: only atomic state and
        // nonblocking polls, no filesystem or request-stream I/O.
        if !tree_live.load(Ordering::Acquire)||(site.stopping)()||!live(origin,tree_peer,0)||
           unsafe{zxs_member_held(fd)}!=0{return Err(denied());}
        Ok(())
    };
    active()?;site.validate()?;
    let name=display::component(&cursor[1..1+usize::from(cursor[0])],222)?;
    let src=InitialReader::alongside(site.dir,site.mark,site.pool,site.shape)?;
    let session=SmbSession{source:&src,original:origin.slot.original.clone()};
    let tree=Tree{session:&session,setting:expected,parent};
    let writer=tree.open_named_writer(name)?;
    writer.require_tree_root(root)?;
    let staged=Staged::open(writer)?;
    let ready=member_packet(staged.base(),staged.base().native_key()?);
    active()?;site.validate()?;
    os(unsafe{zxs_member_reply(fd,ready.as_ptr())})?;
    commands(site,origin,tree_peer,expected,fd,&active,staged)
}
pub(super) fn commands(site:Site<'_>,origin:&TreeOrigin,tree_peer:&OwnedFd,expected:Setting,fd:i32,active:&impl Fn()->io::Result<()>,
    mut staged:Staged<'_,'_,'_>)->io::Result<()>{
    loop{
        active()?;
        let wait=unsafe{zxs_smb_root_wait(fd,origin.session.as_raw_fd(),500)};
        if wait==0{continue;}
        if wait!=1{return Err(denied());}
        match mover::pending(fd){
            1=>{
                let (request,answer)=mover::take(fd,origin)?;
                if !matches!(&request,mover::Request::Delete(_)|mover::Request::Mark){mover::refuse(answer,5);continue;}
                let Some(permit)=mover::Permit::take()else{mover::refuse(answer,1);continue;};
                let source=staged.base().native_placement().clone();
                if matches!(&request,mover::Request::Delete(_)){
                    if staged.transfer_ready().is_err(){mover::refuse(answer,1);continue;}
                    // Native has already drained this explicit CLOSE and
                    // marked the last FILE under its actual share-mode lock.
                    // Release the original staging FILE EX before handoff.
                    drop(staged);
                    mover::run(site,origin,tree_peer,answer,request,source,Some(expected),permit);
                    // The completed native CLOSE consumes the correlated
                    // reply before it closes this still-original control FD.
                    while active().is_ok(){
                        if unsafe{zxs_smb_root_wait(fd,origin.session.as_raw_fd(),500)}!=0{break;}
                    }
                    return Ok(());
                }
                mover::run(site,origin,tree_peer,answer,request,source,Some(expected),permit);
                continue;
            },
            0=>{},
            _=>return Err(denied()),
        }
        let(mut cmd,mut offset,mut count,mut settle)=(0,0,0,0);
        let answer=unsafe{zxs_work_take(fd,origin.worker.as_raw_fd(),&mut cmd,&mut offset,&mut count,&mut settle)};
        if answer<0{return Err(denied());}
        let answer=unsafe{OwnedFd::from_raw_fd(answer)};
        if count>0xffff00{return Err(denied());}
        let out=answer.as_raw_fd();
        let alive=||{active()?;os(unsafe{zxs_work_answer_live(out)})};
        if alive().is_err(){continue;}
        site.validate()?;
        let cap=Limit::take(&BYTES,count as usize,MAX_BYTES);
        if cmd==READ{
            let Some(_bytes)=cap else{continue;};
            let read=staged.read_window(offset,count,&alive,|bytes|
                os(unsafe{zxs_read_fill(out,bytes.as_ptr(),bytes.len())}));
            if read.is_ok()&&alive().is_ok()&&site.validate().is_ok(){
                let _=unsafe{zxs_read_accept(out)};
            }
            continue;
        }
        let mut retire=false;
        let result=(||{
            let _bytes=cap.ok_or_else(||io::Error::from(io::ErrorKind::OutOfMemory))?;
            let mut data=Vec::<u8>::new();
            data.try_reserve_exact(count as usize).map_err(|_|io::Error::from(io::ErrorKind::OutOfMemory))?;
            data.resize(count as usize,0);
            os(unsafe{zxs_work_input(out,data.as_mut_ptr(),data.len())})?;
            alive()?;
            match cmd{
                WRITE=>{
                    staged.write(offset,&data,&alive)?;
                    if settle!=0{
                        // An unconfirmed write-through must not leave an old
                        // native size attached to the enlarged private view.
                        retire=true;
                        staged.flush(&alive)?;
                        retire=false;
                    }
                }
                EOF=>staged.resize(offset,&alive)?,
                FLUSH=>staged.flush(&alive)?,
                _=>return Err(invalid("unsupported private writer command")),
            }
            Ok(())
        })();
        if alive().is_ok()&&site.validate().is_ok(){
            let (code,eof)=result.as_ref().map_or_else(|e|(status(e),0),|_|(0,staged.eof()));
            if unsafe{zxs_work_reply(out,code,eof)}!=0 && result.is_ok(){break;}
        }else if result.is_ok(){
            // Never keep using an FSP whose native view was not updated.
            break;
        }
        if retire{break;}
    }
    Ok(()) // disconnect merely releases this writer, never causes a SAVE
}
