//! Closed, small synchronous replies to an original paid, signed member.
//! No generic success body or caller-selected wire ID.
use super::{Error,Result,message::{RELATED,SIGNED,Routing}};
use crate::smb::connection::sessions::ordinary::VerifiedMember;
use std::num::NonZeroU64;

const ECHO:u16=13;
const TREE_CONNECT:u16=3;
pub(in crate::smb) const MAX_SMALL_REPLY:usize=72;

#[derive(Clone,Copy)]
#[repr(u32)]
pub(in crate::smb) enum TreeFailure {
    InvalidParameter=0xc000_000d,
    InsufficientResources=0xc000_009a,
    BadNetworkName=0xc000_00cc,
}

pub(in crate::smb) struct UnsignedSmall {raw:[u8;MAX_SMALL_REPLY],len:usize,sid:NonZeroU64}
pub(in crate::smb) struct PayableSmall(UnsignedSmall);
pub(in crate::smb) struct SignedSmall(UnsignedSmall);

impl UnsignedSmall {
    pub(in crate::smb) fn echo(member:&VerifiedMember<'_,'_>,sid:NonZeroU64,compound:bool,more:bool)
        ->Result<Self> {
        let m=member.message();
        // Ignore the client's Reserved word, and zero our own. Incoming
        // command padding was checked as part of that member's signature.
        let ok=m.raw.len()>=68 && m.raw[64..66]==4u16.to_le_bytes();
        let status=if ok {0} else {0xc000_000d};
        let len=if compound || !ok {MAX_SMALL_REPLY} else {68};
        let mut draft=Self::from_member(member,sid,ECHO,status,len,compound,more)?;
        draft.raw[36..40].fill(0); // ECHO returns no Tree
        draft.raw[64]=if ok {4} else {9};
        Ok(draft)
    }

    pub(in crate::smb) fn tree_failure(member:&VerifiedMember<'_,'_>,sid:NonZeroU64,why:TreeFailure)
        ->Result<Self> {
        let m=member.message();
        if m.header.routing!=(Routing::Sync{tree:0}) || m.header.flags&RELATED!=0 || m.raw.get(20..24)!=Some(&[0;4][..]) {
            return Err(Error::InvalidRequest);
        }
        let mut draft=Self::from_member(member,sid,TREE_CONNECT,why as u32,MAX_SMALL_REPLY,false,false)?;
        // MS-SMB2 §§2.2.2/3.3.4.4: no ErrorData for these plain failures;
        // StructureSize=9, eight fixed bytes, zero context count/ByteCount.
        draft.raw[64]=9;
        Ok(draft)
    }

    fn from_member(member:&VerifiedMember<'_,'_>,sid:NonZeroU64,command:u16,
                   status:u32,len:usize,compound:bool,more:bool)->Result<Self> {
        let m=member.message(); // exact original member, including copied header fields
        if m.header.command!=command || !member.response_session_matches(sid) ||
            m.header.flags&SIGNED==0 || (more && !compound) ||
            (m.header.flags&RELATED==0 && m.header.session!=sid.get()) {return Err(Error::InvalidRequest);}
        let mut raw=[0;MAX_SMALL_REPLY];raw[..64].copy_from_slice(m.raw.get(..64).ok_or(Error::InvalidRequest)?);
        raw[8..12].copy_from_slice(&status.to_le_bytes());
        raw[14..16].fill(0); // grant belongs to the originating ledger, not the command
        raw[16..20].copy_from_slice(&(m.header.flags|1|SIGNED).to_le_bytes());
        raw[20..24].copy_from_slice(&(if more {len as u32} else {0}).to_le_bytes());
        raw[40..48].copy_from_slice(&sid.get().to_le_bytes());raw[48..64].fill(0);
        Ok(Self{raw,len,sid})
    }

    pub(in crate::smb) fn credit(mut self,grant:u16)->PayableSmall {
        self.raw[14..16].copy_from_slice(&grant.to_le_bytes());PayableSmall(self)
    }
}
impl PayableSmall {
    pub(in crate::smb) fn sid(&self)->NonZeroU64 {self.0.sid}
    pub(in crate::smb) fn for_mac(&self)->&[u8] {&self.0.raw[..self.0.len]}
    pub(in crate::smb) fn signed(mut self,tag:[u8;16])->SignedSmall {
        self.0.raw[48..64].copy_from_slice(&tag);SignedSmall(self.0)
    }
}
impl SignedSmall {pub(in crate::smb) fn bytes(&self)->&[u8] {&self.0.raw[..self.0.len]}}
