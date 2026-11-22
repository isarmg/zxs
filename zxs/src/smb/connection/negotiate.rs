//! Local inputs for one successful direct NEGOTIATE. A ServerGuid belongs
//! to one genuine SMB server initialization, not each connection or a disk
//! installation. No production issuer or listener is added by this module.
use super::{Fault,Opening,OpeningSend,Result};
use crate::smb::wire::{Error,negotiate_reply};
pub(in crate::smb) use super::sessions::RunningServerGuid;
use crate::smb::local::{filetime_delta,random};
use std::time::{SystemTime,UNIX_EPOCH};

impl Opening {
    /// No externally supplied clock, random bytes, grant or algorithm switch.
    /// A locally complete byte buffer is still NOT a published connection.
    pub(in crate::smb) fn respond(self, server: &RunningServerGuid) -> Result<OpeningSend> {
        self.respond_from(server,|salt| if random(salt) {Ok(())} else {Err(Fault::Entropy)},SystemTime::now)
    }
    fn respond_from(self, server: &RunningServerGuid, draw: impl FnOnce(&mut [u8;32])->Result<()>,
        now: impl FnOnce()->SystemTime) -> Result<OpeningSend> {
        if server.wire() == &[0;16] { return Err(Fault::Identity); }
        let mut salt = [0;32]; draw(&mut salt)?;
        let time = filetime_delta(now().duration_since(UNIX_EPOCH).map_err(|e| e.duration())).ok_or(Fault::Clock)?;
        let bytes = negotiate_reply::encode(&self.original,self.hash.signing_choice(),self.grant,
            server.wire(),time,&salt).map_err(|e| if e == Error::Limit { Fault::Resources } else { Fault::Opening })?;
        self.bind(bytes,server)
    }
}

#[cfg(test)]
#[path = "../../../tests/smb/negotiate_reply.rs"]
mod tests;

#[cfg(test)]
include!("../../../tests/unit/attached/smb/connection/negotiate/items.rs");
