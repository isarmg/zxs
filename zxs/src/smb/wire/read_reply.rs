//! One capped signed READ reply, carrying the exact original Core receipt.
//! This byte codec cannot select an OPEN, disclose storage, or write a socket.
use super::{Error,Result,direct::MAX_FRAME_BYTES,fixed_reply::standalone_header,io_reply::FileReply};
use crate::smb::connection::sessions::ordinary::{PendingRead,VerifiedMember};
use std::num::NonZeroU64;

pub(in crate::smb) const READ:u16=8;
pub(in crate::smb) const HEADER_AND_READ_FIXED:usize=80;
const END_OF_FILE:u32=0xc000_0011;

#[derive(Clone,Copy)]
#[repr(u32)]
pub(in crate::smb) enum ReadFailure{
    InvalidParameter=0xc000_000d,AccessDenied=0xc000_0022,FileLockConflict=0xc000_0054,Resources=0xc000_009a,FileIsDirectory=0xc000_00ba,
    NotSupported=0xc000_00bb,NetworkNameDeleted=0xc000_00c9,Internal=0xc000_00e5,FileClosed=0xc000_0128,
    FileNotAvailable=0xc000_0467,ServerUnavailable=0xc000_0466,VolumeDismounted=0xc000_026e,
}

pub(in crate::smb) struct UnsignedRead {record:Vec<u8>,sid:NonZeroU64,original:PendingRead}
pub(in crate::smb) struct PayableRead(UnsignedRead);
pub(in crate::smb) struct SignedRead(UnsignedRead);

impl UnsignedRead {
    pub(in crate::smb) fn from_original(member:&VerifiedMember<'_,'_>,original:PendingRead)->Result<Self>{
        let sid=original.sid();
        if !original.matches(member){return Err(Error::InvalidRequest);}
        let request=member.message().read_request(member.max_read()).map_err(|_|Error::InvalidRequest)?;
        let reply=FileReply::read(original.result()).map_err(|_|Error::InvalidRequest)?;
        let status=reply.status();let (body,data)=reply.parts();
        let shaped=match status{
            0=>body.len()==16&&!data.is_empty()&&
                (u64::from(request.range.minimum())..=u64::from(request.range.length())).contains(&(data.len() as u64)),
            END_OF_FILE=>body.len()==8&&data.is_empty(),
            _=>false,
        };
        if reply.command()!=READ||!shaped{return Err(Error::InvalidRequest);}
        let smb_size=64usize.checked_add(body.len()).and_then(|n|n.checked_add(data.len())).ok_or(Error::Limit)?;
        if smb_size>MAX_FRAME_BYTES{return Err(Error::Limit);}
        let head=standalone_header(member.message(),sid,READ,status)?;
        let mut record=Vec::new();record.try_reserve_exact(4+smb_size).map_err(|_|Error::Limit)?;
        record.extend_from_slice(&(smb_size as u32).to_be_bytes());record.extend_from_slice(&head);
        record.extend_from_slice(body);record.extend_from_slice(data);
        Ok(Self{record,sid,original})
    }
    pub(in crate::smb) fn credit(mut self,grant:u16)->PayableRead{
        self.record[18..20].copy_from_slice(&grant.to_le_bytes());PayableRead(self)
    }
}
impl PayableRead {
    pub(in crate::smb) fn sid(&self)->NonZeroU64{self.0.sid}
    pub(in crate::smb) fn for_mac(&self)->&[u8]{&self.0.record[4..]}
    pub(in crate::smb) fn signed(mut self,tag:[u8;16])->SignedRead{
        self.0.record[52..68].copy_from_slice(&tag);SignedRead(self.0)
    }
}
impl SignedRead {
    pub(in crate::smb) fn into_send(self)->(Vec<u8>,PendingRead){(self.0.record,self.0.original)}
}
