//! The MS-SMB2 3.3.5.5.3 final GSS step pins ClientGuid -> dialect per
//! running server. This small bookkeeping table is not a principal database.
use super::{Global,Origin,Run};
use crate::smb::connection::{Connection,Fault,Result};
use std::sync::{Arc,Weak,atomic::Ordering};

const TRANSPORT_CAP:usize=128;
pub(super) struct Tracked { origin:Weak<Origin>, guid:[u8;16] }
pub(super) struct Dialect { guid:[u8;16], selected:u16 }
impl Global {
    pub(super) fn pinned(&self,origin:&Arc<Origin>,guid:[u8;16],dialect:u16)->bool {
        self.transports.iter().any(|t|t.guid==guid && Weak::ptr_eq(&t.origin,&Arc::downgrade(origin))) &&
            self.clients.iter().any(|c|c.guid==guid && c.selected==dialect)
    }
    fn prune_transports(&mut self) {
        self.transports.retain(|t|t.origin.upgrade().is_some_and(|o|!o.closed.load(Ordering::Acquire)));
        // 3.3.7.1 permits removing a Client entry once no negotiated
        // transport for that GUID remains (including one not yet logged in).
        let transports=&self.transports;
        self.clients.retain(|v|transports.iter().any(|t|t.guid==v.guid));
    }
}
/// A new reader consumes the same per-run capacity as a negotiated transport
/// BEFORE its first blocking read. Promotion transfers this slot atomically;
/// an incomplete prefix, failed reply or abandoned reader gives it back.
#[must_use]
pub(in crate::smb::connection) struct AcceptSeat { run:Arc<Run>, reserved:bool }
impl AcceptSeat {
    pub(in crate::smb::connection) fn reserve(run:&Arc<Run>)->Result<Self> {
        let mut g=run.index.lock().map_err(|_|Fault::SessionState)?;
        g.prune_transports();
        if g.transports.len()+g.accepting>=TRANSPORT_CAP {return Err(Fault::SessionLimit);}
        g.accepting+=1;Ok(Self{run:Arc::clone(run),reserved:true})
    }
    pub(in crate::smb::connection) fn promote(mut self,c:&Connection)->Result<()> {
        c.alive()?;
        if !Arc::ptr_eq(&self.run,&c.origin.run) {return Err(Fault::SessionState);}
        let mut g=self.run.index.lock().map_err(|_|Fault::SessionState)?;
        if g.accepting==0 || g.transports.iter().any(|t|Weak::ptr_eq(&t.origin,&Arc::downgrade(&c.origin))) {
            return Err(Fault::SessionState);
        }
        g.transports.try_reserve(1).map_err(|_|Fault::Resources)?;
        g.transports.push(Tracked{origin:Arc::downgrade(&c.origin),guid:c.client.guid});
        g.accepting-=1;self.reserved=false;Ok(())
    }
}
impl Drop for AcceptSeat {
    fn drop(&mut self) {
        if self.reserved && let Ok(mut g)=self.run.index.lock() {g.accepting-=1;}
    }
}
impl Connection {
    pub(super) fn pin_client_dialect(&self)->Result<()> {
        self.alive()?;
        let mut g=self.origin.run.index.lock().map_err(|_|Fault::SessionState)?;
        g.prune_transports();
        let origin=Arc::downgrade(&self.origin);let guid=self.client.guid;
        if !g.transports.iter().any(|t|t.guid==guid && Weak::ptr_eq(&t.origin,&origin)) {
            return Err(Fault::SessionState);
        }
        let selected=self.server.dialect; // from our actual accepted NEGOTIATE, not today's peer fields
        if let Some(c)=g.clients.iter().find(|v|v.guid==guid) {
            return if c.selected==selected {Ok(())} else {Err(Fault::ClientDialect)};
        }
        if g.clients.len()>=TRANSPORT_CAP {return Err(Fault::SessionLimit);}
        g.clients.try_reserve(1).map_err(|_|Fault::Resources)?;g.clients.push(Dialect{guid,selected});Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../tests/smb/client_dialect.rs"]
mod tests;
