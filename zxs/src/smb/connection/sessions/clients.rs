//! The MS-SMB2 3.3.5.5.3 final GSS step pins ClientGuid -> dialect per
//! running server. This small bookkeeping table is not a principal database.
use super::{Global,Origin};
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
impl Connection {
    pub(in crate::smb::connection) fn register_transport(&self)->Result<()> {
        let mut g=self.origin.run.index.lock().map_err(|_|Fault::SessionState)?;
        self.alive()?;g.prune_transports();
        if g.transports.iter().any(|t|Weak::ptr_eq(&t.origin,&Arc::downgrade(&self.origin))) {return Err(Fault::SessionState);}
        if g.transports.len()>=TRANSPORT_CAP {return Err(Fault::SessionLimit);}
        g.transports.try_reserve(1).map_err(|_|Fault::Resources)?;
        g.transports.push(Tracked{origin:Arc::downgrade(&self.origin),guid:self.client.guid});Ok(())
    }
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
#[path = "../../../test/smb/client_dialect.rs"]
mod tests;
