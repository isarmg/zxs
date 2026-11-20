//! One authenticated SMB business root. Root listing uses only the original
//! plan and current SQL; no selected drive has to be online merely to list it.
use super::{InitialReader,Site,SmbSession,TreeOrigin,denied,live,os,serve_folder};
use super::super::super::{share_mode::{Mode,Setting}};
use crate::display;
use std::{io,os::fd::{AsRawFd,FromRawFd,OwnedFd},sync::atomic::{AtomicBool,AtomicUsize,Ordering},thread};

const NAME:usize=255;
const SLOT:usize=1+NAME+16;
const ROW:usize=16;
const OPEN_READ:u8=253;
const OPEN_WRITE:u8=252;
const OPEN_MAX:u8=247;
static READS:AtomicUsize=AtomicUsize::new(0);
static WRITES:AtomicUsize=AtomicUsize::new(0);

unsafe extern "C"{
    fn zxs_smb_tree_reply(fd:i32,bytes:*const u8)->i32;
    fn zxs_smb_root_wait(tree:i32,session:i32,timeout:i32)->i32;
    fn zxs_dir_take(tree:i32,worker:i32,special:*mut u8,name:*mut u8)->i32;
    fn zxs_ns_dir_reply(fd:i32,body:*const u8)->i32;
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
    let child=Site{stopping:&alive,..site};
    thread::scope(|scope|{
        let _stop=Stop(&running);
        let mut jobs=Vec::new();
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
            if matches!(role,OPEN_READ|OPEN_WRITE|OPEN_MAX){
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
