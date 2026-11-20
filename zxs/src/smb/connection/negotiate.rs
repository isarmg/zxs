//! Local inputs for one successful direct NEGOTIATE. A ServerGuid belongs
//! to one genuine SMB server initialization, not each connection or a disk
//! installation. No production issuer or listener is added by this module.
use super::{Fault,Opening,OpeningSend,Result};
use crate::smb::wire::{Error,negotiate_reply};
use std::time::{Duration,SystemTime,UNIX_EPOCH};

/// All transports of one running server borrow its same nonsecret GUID in
/// MS-DTYP packet byte order. MS-SMB2 3.3.3 generates a NEW GUID at server
/// initialization. This is neither durable installation identity, user/SID,
/// client GUID nor authority to authenticate. Only fixtures can mint one here.
pub(in crate::smb) struct RunningServerGuid { wire: [u8;16] }

fn fresh_salt(dst: &mut [u8;32]) -> Result<()> {
    // Existing ZXS Linux getrandom(0) wrapper; handles EINTR and short fills.
    // No pathname, host crypto library, architecture-specific syscall number
    // or weaker fallback. The bytes are a fresh public salt, NOT a session key.
    unsafe extern "C" { fn zxs_random(out: *mut u8, length: usize) -> std::os::raw::c_int; }
    // SAFETY: dst is live/writable for the exact synchronous fill.
    if unsafe { zxs_random(dst.as_mut_ptr(),dst.len()) } != 0 { return Err(Fault::Entropy); }
    Ok(())
}

fn filetime_delta(delta: std::result::Result<Duration,Duration>) -> Option<u64> {
    const UNIX_TICKS: u128 = 116_444_736_000_000_000;
    let ticks = match delta {
        Ok(after) => UNIX_TICKS.checked_add(after.as_nanos()/100)?,
        // Floor on the absolute timeline; an instant 1 ns before the Unix
        // epoch is one FILETIME tick earlier, not the epoch itself.
        Err(before) => UNIX_TICKS.checked_sub(before.as_nanos().div_ceil(100))?,
    };
    ticks.try_into().ok()
}

impl Opening {
    /// No externally supplied clock, random bytes, grant or algorithm switch.
    /// A locally complete byte buffer is still NOT a published connection.
    pub(in crate::smb) fn respond(self, server: &RunningServerGuid) -> Result<OpeningSend> {
        self.respond_from(server,fresh_salt,SystemTime::now)
    }
    fn respond_from(self, server: &RunningServerGuid, draw: impl FnOnce(&mut [u8;32])->Result<()>,
        now: impl FnOnce()->SystemTime) -> Result<OpeningSend> {
        if server.wire == [0;16] { return Err(Fault::Identity); }
        let mut salt = [0;32]; draw(&mut salt)?;
        let time = filetime_delta(now().duration_since(UNIX_EPOCH).map_err(|e| e.duration())).ok_or(Fault::Clock)?;
        let bytes = negotiate_reply::encode(&self.original,self.hash.signing_choice(),self.grant,
            &server.wire,time,&salt).map_err(|e| if e == Error::Limit { Fault::Resources } else { Fault::Opening })?;
        self.bind(bytes)
    }
}

#[cfg(test)]
#[path = "../../test/smb/negotiate_reply.rs"]
mod tests;
