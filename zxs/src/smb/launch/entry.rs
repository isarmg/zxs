//! Fixed executable boundary for a verified original root supervisor. There is no
//! alternative CLI, input path, listener constructor or descriptor option.
use super::{Fault,InheritedRun,Result,Role,maintenance::{self,Task},worker::Worker};
use std::{ffi::OsString,io,sync::{Arc,atomic::{AtomicBool,Ordering}},thread::{self,JoinHandle}};

const MAX_PEERS:usize=8;
static STOP:AtomicBool=AtomicBool::new(false);
pub(in crate::smb) fn stopping()->bool{STOP.load(Ordering::Acquire)}
fn stop(){STOP.store(true,Ordering::Release);}

pub(in crate::smb::launch) struct InstalledTerm(());
impl InstalledTerm{
    fn install()->Result<Self>{
        // SAFETY: C signal state is inspected/changed once in the dedicated
        // child, before any lane or inherited credential material is read.
        unsafe{
            let mut act:libc::sigaction=std::mem::zeroed();
            act.sa_sigaction=term as *const () as libc::sighandler_t;
            act.sa_flags=0; // never restart a blocking syscall after TERM
            let mut set:libc::sigset_t=std::mem::zeroed();
            if libc::sigemptyset(&mut act.sa_mask)!=0||libc::sigemptyset(&mut set)!=0||
                libc::sigaddset(&mut set,libc::SIGTERM)!=0||
                libc::sigaction(libc::SIGTERM,&act,std::ptr::null_mut())!=0||
                libc::pthread_sigmask(libc::SIG_UNBLOCK,&set,std::ptr::null_mut())!=0{
                return Err(Fault::Handoff);
            }
        }
        Ok(Self(()))
    }
    pub(in crate::smb::launch) fn stopping(&self)->bool{stopping()}
}
extern "C" fn term(_:libc::c_int){
    // Atomics on this Linux target; no allocation, lock or I/O in the handler.
    stop();
}

fn selected(argv:&mut impl Iterator<Item=OsString>)->Option<Result<Role>>{
    let zero=argv.next()?;
    let bytes=zero.as_os_str().as_encoded_bytes();
    let role=Role::ALL.into_iter().find(|role|role.marker().to_bytes()==bytes)?;
    Some(if argv.next().is_none(){Ok(role)}else{Err(Fault::Shape)})
}

/// Only the main ZXS ELF calls this before its normal foreground entry.
/// Exact argv[0] chooses a guarded path, never authorizes it. Real launch
/// proof still requires the original supervised descriptors and Core cut.
///
/// # Safety
/// Call once at the process entry, before starting threads or taking
/// ownership of inherited descriptor 3 or 4 elsewhere. If selected, this
/// owns those descriptors after verifying the strict child prerequisites.
pub unsafe fn try_supervised_zxs_role()->Option<io::Result<()>>{
    let shape=selected(&mut std::env::args_os())?;
    // SAFETY: only the main entry calls with exclusive inherited FD ownership.
    let result=shape.and_then(|role|unsafe{match role{
        Role::Server=>enter(),
        Role::Preflight=>super::low::enter(InstalledTerm::install()?),
        Role::Web=>{
            #[cfg(all(target_arch="x86_64",target_pointer_width="64"))]
            {super::web::enter(InstalledTerm::install()?)}
            #[cfg(not(all(target_arch="x86_64",target_pointer_width="64")))]
            {Err(Fault::Shape)}
        }
    }});
    Some(result.map_err(|_|io::Error::new(io::ErrorKind::PermissionDenied,
        "the private supervised ZXS role stopped before verification or drain")))
}
unsafe fn enter()->Result<()>{
    let term=InstalledTerm::install()?;
    if term.stopping(){return Err(Fault::Handoff);}
    // SAFETY: import the fixed descriptors in this one process entry only.
    let original=Arc::new(unsafe{InheritedRun::receive()}?);
    let worker=Worker::prepare(Arc::clone(&original))?;
    let mut peers=Vec::<JoinHandle<Result<()>>>::new();
    peers.try_reserve_exact(MAX_PEERS).map_err(|_|Fault::Resources)?;
    let result=thread::scope(|scope|{
        // Qualify thread-local Core readers before READY; the cold tasks
        // cannot start physical recovery until the shared admission seal.
        let (retirement_start,retirement)=maintenance::prepare(scope,Arc::clone(&original),Task::Retirement)?;
        let (rename_start,rename)=maintenance::prepare(scope,Arc::clone(&original),Task::Rename)?;
        let (relabel_start,relabel)=maintenance::prepare(scope,Arc::clone(&original),Task::DirectoryRelabel)?;
        let admitted=worker.prepare_pair(&term).and_then(|()|{
            retirement_start.send(()).map_err(|_|Fault::Handoff)?;
            rename_start.send(()).map_err(|_|Fault::Handoff)?;
            relabel_start.send(()).map_err(|_|Fault::Handoff)
        });
        drop(retirement_start);drop(rename_start);drop(relabel_start);
        let mut from=0usize;
        let result=admitted.and_then(|()|{
            while !stopping(){
                if retirement.is_finished()||rename.is_finished()||relabel.is_finished(){return Err(Fault::Handoff);}
                let mut i=0;
                while i<peers.len(){
                    if peers[i].is_finished(){
                        complete(peers.swap_remove(i))?;
                    }else{i+=1;}
                }
                let Some(index)=worker.first_ready(from,peers.len()<MAX_PEERS)? else{continue;};
                from=index+1; // round robin across actual retained listeners
                let Some((accepted,lane))=worker.accept(index)? else{continue;};
                let spawned=thread::Builder::new().spawn(move||lane.serve(accepted)).map_err(|_|Fault::Resources)?;
                peers.push(spawned); // capacity already reserved before READY
            }
            Ok(())
        });
        stop();
        // Join every original peer and cold authority before releasing the
        // bound startup and its main original S lease. No implicit user FLUSH
        // or newly chosen operation is permitted on this path.
        let mut outcome=result;
        for lane in peers.drain(..){if let Err(e)=complete(lane){outcome=outcome.and(Err(e));}}
        for job in [retirement,rename,relabel]{
            if let Err(e)=job.join().map_err(|_|Fault::Handoff).and_then(|result|result){
                outcome=outcome.and(Err(e));
            }
        }
        outcome
    });
    drop(worker);
    result
}
fn complete(lane:JoinHandle<Result<()>>)->Result<()>{lane.join().map_err(|_|Fault::Handoff)?}

#[cfg(test)]
#[path="../../test/smb/launch_role.rs"]
mod tests;
