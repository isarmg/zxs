//! Accept only from the exact host listener retained by the supervisor
//! handoff. No caller-supplied socket, endpoint, or forwarding metadata.
use super::InheritedRun;
use crate::{net::retry_tcp_accept,system::reader::smb::runtime::QualifiedRustInstall};
use std::{io,net::TcpStream,sync::{Arc,atomic::Ordering}};

#[must_use]
pub(in crate::smb) struct OwnedAccepted {
    socket:TcpStream,
    start:Arc<InheritedRun>,
    listener:usize,
}
impl OwnedAccepted {
    pub(in crate::smb) fn unpack(self)->(TcpStream,Arc<InheritedRun>,usize){
        (self.socket,self.start,self.listener)
    }
}
impl InheritedRun {
    pub(in crate::smb) fn accept_original(self:&Arc<Self>,installation:&QualifiedRustInstall,index:usize)
        ->io::Result<Option<OwnedAccepted>>{
        let bad=||io::Error::new(io::ErrorKind::PermissionDenied,"wrong original SMB listener or installation");
        if self.ready.load(Ordering::Acquire)!=2||!installation.is_original_start(self){return Err(bad());}
        installation.require_current()?;
        let listener=self.listeners.get(index).ok_or_else(bad)?;
        if !installation.original_listener_is(index,listener.local_addr()?){return Err(bad());}
        let socket=match listener.accept(){
            Ok((s,_))=>s, // we re-read both addresses from this same socket
            Err(e) if retry_tcp_accept(&e)=>return Ok(None),
            Err(e)=>return Err(e),
        };
        Ok(Some(OwnedAccepted{socket,start:Arc::clone(self),listener:index}))
    }
}
