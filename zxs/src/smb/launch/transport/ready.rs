//! Original control record and listener readiness; no supplied FD or
//! network address can become an input to either operation.
use super::{Fault,InheritedRun,MAX_LISTENERS,Result};
use crate::{smb::{connection::sessions::RunningServerGuid,readiness::{self,Interest,Watch}},
    system::reader::smb::runtime::QualifiedRustInstall};
use std::{io::ErrorKind,os::fd::{AsFd,AsRawFd},sync::{Arc,atomic::Ordering},time::Instant};

impl InheritedRun {
    pub(in crate::smb::launch) fn publish_prepared(self:&Arc<Self>,selected:&QualifiedRustInstall,server:&RunningServerGuid,
        gate:&crate::smb::launch::entry::InstalledTerm)->Result<()>{
        if gate.stopping()||!selected.is_original_start(self)||!server.is_original_start(self){return Err(Fault::Handoff);}
        selected.require_current().map_err(|_|Fault::Handoff)?;
        self.supervisor.require_live()?;
        self.ready.compare_exchange(0,1,Ordering::AcqRel,Ordering::Acquire).map_err(|_|Fault::Handoff)?;
        let packet=self.start.prepared_frame();
        let fd=self.supervisor.socket.as_raw_fd();
        loop {
            if gate.stopping()||Instant::now()>=self.until{return Err(Fault::Handoff);}
            self.supervisor.require_live()?;
            // SAFETY: send uses only the retained supervisor socket and the
            // complete fixed packet. SO_PASSCRED/SO_PASSPIDFD are peer-side.
            let n=unsafe{libc::send(fd,packet.as_ptr().cast(),packet.len(),libc::MSG_DONTWAIT|libc::MSG_NOSIGNAL)};
            if n==packet.len() as isize{break;}
            if n>=0{return Err(Fault::Handoff);} // SeqPacket cannot accept a short acknowledgement
            match std::io::Error::last_os_error().kind(){
                ErrorKind::Interrupted=>continue,
                ErrorKind::WouldBlock=>{
                    let left=self.until.saturating_duration_since(Instant::now());
                    let mut p=[Watch::new(self.supervisor.socket.as_fd(),Interest::Write)];
                    let ms=left.as_millis().saturating_add(1).min(100) as i32;
                    let result=readiness::wait(&mut p,ms);
                    if result.is_err_and(|e|e.kind()!=ErrorKind::Interrupted)||p[0].events().broken(){
                        return Err(Fault::Handoff);
                    }
                },
                _=>return Err(Fault::Handoff),
            }
        }
        // SAFETY: only write-half closes; the same source proof is retained
        // until all accepted peers and the original Core owner are gone.
        if unsafe{libc::shutdown(fd,libc::SHUT_WR)}!=0||gate.stopping()||Instant::now()>=self.until{
            return Err(Fault::Handoff);
        }
        self.supervisor.require_live()?;
        Ok(())
    }
    pub(in crate::smb::launch) fn await_pair(&self,term:&crate::smb::launch::entry::InstalledTerm)->Result<()>{
        let gate=self.pair.lock().map_err(|_|Fault::Handoff)?.take().ok_or(Fault::Handoff)?;
        gate.wait(self.env.pair.until()?,||{
            if term.stopping(){return Err(Fault::Handoff);}
            self.supervisor.require_live()
        })?;
        self.ready.compare_exchange(1,2,Ordering::Release,Ordering::Acquire).map_err(|_|Fault::Handoff)?;
        Ok(())
    }
    pub(in crate::smb::launch) fn next_listener(&self,from:usize,admit:bool)->Result<Option<usize>>{
        if self.ready.load(Ordering::Acquire)!=2{return Err(Fault::Handoff);}
        let n=self.listeners.len();
        let mut p:[Watch<'_>;MAX_LISTENERS+1]=std::array::from_fn(|_|Watch::vacant());
        p[0]=self.supervisor.parent.poll_watch();
        if admit{
            for (dst,s) in p[1..].iter_mut().zip(&self.listeners){
                *dst=Watch::new(s.as_fd(),Interest::Read);
            }
        }
        let used=if admit{n+1}else{1};
        // Readiness of borrowed originals is not an endpoint or permission.
        let r=match readiness::wait(&mut p[..used],100){
            Ok(r)=>r,
            Err(e) if e.kind()==ErrorKind::Interrupted=>return Ok(None),
            Err(_)=>return Err(Fault::Handoff),
        };
        if p[0].events().any()||p[1..used].iter().any(|v|v.events().broken()){
            return Err(Fault::Handoff);
        }
        if !admit||r==0{return Ok(None);}
        Ok((0..n).map(|j|(from+j)%n).find(|&i|p[i+1].events().readable()))
    }
}
