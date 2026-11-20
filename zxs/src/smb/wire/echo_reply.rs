//! Only authenticated synchronous ECHO: a fixed 4-byte success or an exact
//! parameter error. No constructor accepts arbitrary status, bodies or data.
use super::{Error,Result,message::{RELATED,SIGNED}};
use crate::smb::connection::sessions::ordinary::VerifiedMember;
use std::num::NonZeroU64;

const ECHO:u16=13;
pub(in crate::smb) const MAX_ECHO_BYTES:usize=72;
pub(in crate::smb) struct UnsignedEcho {raw:[u8;MAX_ECHO_BYTES],len:usize,sid:NonZeroU64}
pub(in crate::smb) struct PayableEcho(UnsignedEcho);
pub(in crate::smb) struct SignedEcho(UnsignedEcho);
impl UnsignedEcho {
    pub(in crate::smb) fn encode(member:&VerifiedMember<'_,'_>,sid:NonZeroU64,compound:bool,more:bool)
        ->Result<Self> {
        if !member.is_echo_session(sid) {return Err(Error::InvalidRequest);}
        let m=member.message(); // immutable parsed view from this exact paid, verified member
        if m.header.command!=ECHO || m.header.flags&SIGNED==0 || (more && !compound) ||
            (m.header.flags&RELATED==0 && m.header.session!=sid.get()) {return Err(Error::InvalidRequest);}
        // The client should zero its Reserved word; we must ignore it and set
        // our own to zero. Trailing original pad was signed on the inbound path.
        let ok=m.raw.len()>=68 && m.raw[64..66]==4u16.to_le_bytes();
        let len=if compound || !ok {72} else {68};
        let mut raw=[0;MAX_ECHO_BYTES];raw[..64].copy_from_slice(&m.raw[..64]);
        let status:u32=if ok {0} else {0xc000_000d};
        raw[8..12].copy_from_slice(&status.to_le_bytes());
        raw[14..16].fill(0); // not signable until the original debit has supplied its grant
        raw[16..20].copy_from_slice(&(m.header.flags|1|SIGNED).to_le_bytes());
        raw[20..24].copy_from_slice(&(if more {len as u32} else {0}).to_le_bytes());
        raw[36..40].fill(0); // ECHO returns no Tree
        raw[40..48].copy_from_slice(&sid.get().to_le_bytes());raw[48..64].fill(0);
        raw[64]=if ok {4} else {9}; // error: no contexts, ByteCount zero, eight fixed bytes
        Ok(Self{raw,len,sid})
    }
    pub(in crate::smb) fn credit(mut self,grant:u16)->PayableEcho {
        self.raw[14..16].copy_from_slice(&grant.to_le_bytes());PayableEcho(self)
    }
}
impl PayableEcho {
    pub(in crate::smb) fn sid(&self)->NonZeroU64 {self.0.sid}
    pub(in crate::smb) fn for_mac(&self)->&[u8] {&self.0.raw[..self.0.len]}
    pub(in crate::smb) fn signed(mut self,tag:[u8;16])->SignedEcho {
        self.0.raw[48..64].copy_from_slice(&tag);SignedEcho(self.0)
    }
}
impl SignedEcho {pub(in crate::smb) fn bytes(&self)->&[u8] {&self.0.raw[..self.0.len]}}
