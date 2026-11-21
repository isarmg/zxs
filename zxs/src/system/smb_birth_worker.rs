//! One original FILE birth or open-if on an independently held folder
//! capability. The native FSP is not revealed before the final real receipt.
use super::{mover,InitialReader,Site,SmbSession,Tree,TreeOrigin,denied,live,member_packet,os,writer};
use super::super::super::{first_work::Current,share_mode::Setting,smb_stage::{Scratch,Staged}};
use crate::{OperationId,display,fs::{HeldDir,invalid,directory_birth::DirectoryParent},ordinary::portable_birth::Birth,system::reader::volume::OrdinaryRoot};
use std::{cell::Cell,io,os::fd::{AsRawFd,FromRawFd,OwnedFd},sync::atomic::{AtomicBool,AtomicUsize,Ordering}};

pub(super) const BEGIN:u8=248;
pub(super) const END:u8=251;
static READ_BIRTHS:AtomicUsize=AtomicUsize::new(0);
pub(super) struct Permit{_writer:Option<writer::Permit>}
impl Permit{
    pub(super) fn acquire(write:bool)->Option<Self>{
        if write{return writer::Permit::acquire().map(|p|Self{_writer:Some(p)});}
        READ_BIRTHS.fetch_update(Ordering::AcqRel,Ordering::Acquire,|n|(n<64).then_some(n+1)).ok()?;
        Some(Self{_writer:None})
    }
}
impl Drop for Permit{fn drop(&mut self){if self._writer.is_none(){READ_BIRTHS.fetch_sub(1,Ordering::AcqRel);}}}
unsafe extern "C"{
    fn zxs_member_held(fd:i32)->i32;
    fn zxs_member_reply(fd:i32,data:*const u8)->i32;
    fn zxs_birth_reply(fd:i32,data:*const u8,kind:u8)->i32;
    fn zxs_birth_take_ack(fd:i32,id:*const u8)->i32;
    fn zxs_birth_done(fd:i32,id:*const u8)->i32;
    fn zxs_smb_root_wait(fd:i32,session:i32,timeout:i32)->i32;
    fn zxs_read_take(member:i32,worker:i32,offset:*mut u64,count:*mut u32)->i32;
    fn zxs_read_fill(answer:i32,data:*const u8,n:usize)->i32;
    fn zxs_read_accept(answer:i32)->i32;
}
fn reject(error:&io::Error)->u8{
    match error.kind(){
        io::ErrorKind::AlreadyExists=>2,
        io::ErrorKind::WouldBlock=>3,
        io::ErrorKind::StorageFull=>5,
        _=>4,
    }
}
fn emit_prospect(b:&Birth,num:u64)->[u8;88]{
    let mut out=[0u8;88];
    for(i,id)in [b.pool.0,b.volume.0,b.entry.0,b.file.0].iter().enumerate(){out[16*i..16*i+16].copy_from_slice(id);}
    out[64..72].copy_from_slice(&num.to_be_bytes());out
}
pub(super) fn serve(site:Site<'_>,origin:&TreeOrigin,folder:&OwnedFd,running:&AtomicBool,setting:Setting,
    parent:DirectoryParent,expected_root:&HeldDir,member:OwnedFd,cursor:[u8;256],flags:u8,_permit:Permit)->io::Result<()>{
    let name=display::component(&cursor[1..1+usize::from(cursor[0])],222)?;
    if flags>3{return Err(invalid("unknown native birth intent"));}
    let write=flags&2!=0;let open_if=flags&1!=0;
    let fd=member.as_raw_fd();
    let alive=||{
        if !running.load(Ordering::Acquire)||(site.stopping)()||!live(origin,folder,0)||
           unsafe{zxs_member_held(fd)}!=0{return Err(denied());}
        Ok(())
    };
    alive()?;site.validate()?;
    let src=InitialReader::alongside(site.dir,site.mark,site.pool,site.shape)?;
    let session=SmbSession{source:&src,original:origin.slot.original.clone()};
    let tree=Tree{session:&session,setting,parent};
    let (c,route)=src.at(|db,c|{
        tree.require(db,&c,false)?;
        let route=super::super::super::ordinary_directory::Route::at(db,&c,setting.volume,parent)?;
        Ok((c,route))
    })?;
    let root=OrdinaryRoot::at_parent(&src,&c,&route,None)?;
    if root.folder().entity!=expected_root.entity || !root.folder().same_mount(expected_root)?{return Err(denied());}
    let offered=Cell::new(false);let emitted=Cell::new(false);
    let original=||{
        let found=tree.occupied_name(name,write)?;
        if found.is_some()&&!open_if{return Err(io::ErrorKind::AlreadyExists.into());}
        let born=if found.is_none(){
            Some(tree.create_native(&root,name,write,|birth,native|{
                site.validate()?;alive()?;
                let bytes=emit_prospect(birth,native);
                os(unsafe{zxs_birth_reply(fd,bytes.as_ptr(),1)})?;offered.set(true);
                os(unsafe{zxs_birth_take_ack(fd,birth.file.0.as_ptr())})
            },|dir|if write{Ok(Some((Scratch::before_birth(dir)?,OperationId::new()?)))}else{Ok(None)},&alive)?)
        }else{None};
        if write{
            let staged=match born{
                Some((birth,Some((scratch,nonce))))=>{
                    let handle=tree.writer_from_newborn(birth,nonce)?;
                    handle.require_tree_root(expected_root)?;Staged::prepared(handle,scratch)?
                },
                None=>{
                    let handle=tree.open_writer(name,found.ok_or_else(denied)?)?;
                    handle.require_tree_root(expected_root)?;Staged::open(handle)?
                },
                _=>return Err(invalid("missing original write preparation")),
            };
            let data=member_packet(staged.base(),staged.base().native_key()?);
            ready(site,&alive,fd,&data,offered.get())?;emitted.set(true);
            writer::commands(site,origin,folder,setting,fd,&alive,staged)
        }else{
            let mut current=match born{
                Some((b,None))=>tree.reader_from_newborn(b)?,
                Some(_)=>return Err(invalid("unexpected write preparation for reader")),
                None=>tree.open_current_root(&root,name)?.0,
            };
            let data=member_packet(&current,current.native_key()?);
            ready(site,&alive,fd,&data,offered.get())?;emitted.set(true);
            read_only(site,origin,folder,setting,fd,&alive,&mut current)
        }
    };
    let result=original();
    if let Err(e)=&result{
        if !offered.get() && !emitted.get() && alive().is_ok() && site.validate().is_ok(){
            let _=unsafe{zxs_birth_reply(fd,std::ptr::null(),reject(e))};
        }
    }
    result
}
fn ready(site:Site<'_>,alive:&impl Fn()->io::Result<()>,fd:i32,data:&[u8;88],born:bool)->io::Result<()>{
    alive()?;site.validate()?;
    os(unsafe{if born{zxs_birth_done(fd,data[48..64].as_ptr())}
        else{zxs_member_reply(fd,data.as_ptr())}})
}
fn read_only(site:Site<'_>,origin:&TreeOrigin,branch:&OwnedFd,expected:Setting,member:i32,
    alive:&impl Fn()->io::Result<()>,current:&mut Current<'_,'_,'_>)->io::Result<()>{
    std::thread::scope(|scope|{
        let mut moved_capacity=None;
        loop{
            alive()?;
            let r=unsafe{zxs_smb_root_wait(member,origin.session.as_raw_fd(),500)};
            if r==0{continue;}if r!=1{return Err(denied());}
            match mover::pending(member){
                2=>{
                    let _=super::namespace::start_moved_capacity(scope,&mut moved_capacity,site,origin,member,current);
                    continue;
                },
                1=>{
                    let (request,answer)=mover::take(member,origin)?;
                    if matches!(&request,mover::Request::Rename(_)|mover::Request::Move{..}){
                        mover::rename(site,answer,&request,current,alive);
                        continue;
                    }
                    if !matches!(&request,mover::Request::Delete(_)|mover::Request::Mark){mover::refuse(answer,5);continue;}
                    let Some(permit)=mover::Permit::take()else{mover::refuse(answer,1);continue;};
                    mover::run(site,origin,branch,answer,request,current.native_placement().clone(),Some(expected),permit);
                    continue;
                },
                0=>{},
                _=>return Err(denied()),
            }
            let (mut at,mut count)=(0,0);
            let fd=unsafe{zxs_read_take(member,origin.worker.as_raw_fd(),&mut at,&mut count)};
            if fd<0{return Err(denied());}
            let answer=unsafe{OwnedFd::from_raw_fd(fd)};
            alive()?;site.validate()?;
            let read=current.smb_read_window(at,count,|b|os(unsafe{zxs_read_fill(answer.as_raw_fd(),b.as_ptr(),b.len())}));
            if read.is_ok() && alive().is_ok() && site.validate().is_ok(){let _=unsafe{zxs_read_accept(answer.as_raw_fd())};}
        }
    })
}
