//! SMB2 QUERY_DIRECTORY replies from an actual ZXS namespace query result.
//! Serialization cannot commit its original search or write a socket.
use super::{Error, Result,direct::MAX_FRAME_BYTES,fixed_reply::{response_header,standalone_header}};
use crate::smb::{backend::{EndOfRoot,FullRootRecords,RootCursorEffect,RootPendingToken,RootScanEnd},
    connection::sessions::ordinary::{BrowsePart,PendingRootQuery,VerifiedMember}};
use std::num::NonZeroU64;

const OUTPUT_OFFSET: u16 = 72; // 64-byte SMB2 header + 8-byte fixed body
const EMPTY_ERROR: [u8; 8] = [9, 0, 0, 0, 0, 0, 0, 0];
const NO_SUCH_FILE: u32 = 0xc000_000f;
const NO_MORE_FILES: u32 = 0x8000_0006;
pub(in crate::smb) const DIRECTORY_COMMAND:u16=14;

#[derive(Clone,Copy)]
#[repr(u32)]
pub(in crate::smb) enum DirectoryFailure{
    InfoLengthMismatch=0xc000_0004,InvalidInfoClass=0xc000_0003,InvalidParameter=0xc000_000d,
    AccessDenied=0xc000_0022,ObjectNameInvalid=0xc000_0033,Resources=0xc000_009a,NotSupported=0xc000_00bb,
    NetworkNameDeleted=0xc000_00c9,FileClosed=0xc000_0128,ServerUnavailable=0xc000_0466,
    VolumeDismounted=0xc000_026e,FileNotAvailable=0xc000_0467,BufferOverflow=0x8000_0005,
}

enum ResultBody {
    Complete { body: [u8; 8], records: FullRootRecords },
    End(RootScanEnd),
}

#[must_use]
pub(in crate::smb) struct DirectoryReply(ResultBody);

impl DirectoryReply {
    /// MS-SMB2 §2.2.34. The count covers these records, not any padding
    /// later needed between SMB2 compound response members.
    pub(in crate::smb) fn complete_root(records: FullRootRecords) -> Result<Self> {
        let n = u32::try_from(records.data().len()).map_err(|_| Error::Limit)?;
        if n == 0 {
            return Err(Error::InvalidRequest);
        }
        let mut body = [0; 8];
        body[..2].copy_from_slice(&9u16.to_le_bytes());
        body[2..4].copy_from_slice(&OUTPUT_OFFSET.to_le_bytes());
        body[4..8].copy_from_slice(&n.to_le_bytes());
        Ok(Self(ResultBody::Complete { body, records }))
    }

    /// MS-FSA §2.1.5.6.3 / MS-SMB2 §§2.2.2, 3.3.4.4. A real no-match
    /// outcome has an ERROR body and no directory data, including the
    /// NO_MORE_FILES warning. An overflow never passes this constructor.
    pub(in crate::smb) fn no_matching_root(end: RootScanEnd) -> Self {
        Self(ResultBody::End(end))
    }

    pub(in crate::smb) fn status(&self) -> u32 {
        match &self.0 {
            ResultBody::Complete { .. } => 0,
            ResultBody::End(end) => match end.kind() {
                EndOfRoot::NoInitialMatch => NO_SUCH_FILE,
                EndOfRoot::Exhausted => NO_MORE_FILES,
            },
        }
    }

    /// The wire part never takes, writes or substitutes a search position.
    pub(in crate::smb) fn pending(&self)->RootPendingToken<'_>{
        match &self.0{
            ResultBody::Complete{records,..}=>records.pending(),
            ResultBody::End(end)=>end.pending(),
        }
    }
    pub(in crate::smb) fn into_effect(self)->RootCursorEffect{
        match self.0{
            ResultBody::Complete{records,..}=>records.into_effect(),
            ResultBody::End(end)=>end.into_effect(),
        }
    }
    /// The original query result and still-unsubmitted search position
    /// stay owned together. These bytes are not authorization to respond.
    pub(in crate::smb) fn parts(&self) -> (&[u8], &[u8]) {
        match &self.0 {
            ResultBody::Complete { body, records } => (body, records.data()),
            ResultBody::End(_) => (&EMPTY_ERROR, &[]),
        }
    }
}

// A proposal travels through all stages in the same allocation as the
// bytes describing it. There is no attach-another-proposal operation.
pub(in crate::smb) struct UnsignedDirectory{record:Vec<u8>,sid:NonZeroU64,original:PendingRootQuery}
pub(in crate::smb) struct PayableDirectory(UnsignedDirectory);
pub(in crate::smb) struct SignedDirectory(UnsignedDirectory);

impl UnsignedDirectory {
    pub(in crate::smb) fn new(member:&VerifiedMember<'_,'_>,original:PendingRootQuery)->Result<Self>{
        let sid=original.sid();
        if !original.matches(member){return Err(Error::InvalidRequest);}
        let head=standalone_header(member.message(),sid,DIRECTORY_COMMAND,original.reply().status())?;
        Self::build(member,original,head,false)
    }
    pub(in crate::smb) fn new_related(part:&BrowsePart<'_,'_>,member:&VerifiedMember<'_,'_>,original:PendingRootQuery)->Result<Self>{
        if !original.matches_browse(part,member){return Err(Error::InvalidRequest);}
        let (m,sid,tree,_)=part.fields();
        let mut head=response_header(m,sid,DIRECTORY_COMMAND,original.reply().status(),0)?;
        head[36..40].copy_from_slice(&tree.to_le_bytes());
        Self::build(member,original,head,true)
    }
    fn build(member:&VerifiedMember<'_,'_>,original:PendingRootQuery,head:[u8;64],pad:bool)->Result<Self>{
        let sid=original.sid();
        let reply=original.reply();let (fixed,data)=reply.parts();
        let maximum=member.message().directory_output(member.max_transact()).map_err(|_|Error::InvalidRequest)?;
        let body=64usize.checked_add(fixed.len()).and_then(|n|n.checked_add(data.len())).ok_or(Error::Limit)?;
        let smb=if pad{body.checked_add(7).ok_or(Error::Limit)?&!7}else{body};
        if fixed.len()!=8||data.len() as u64>u64::from(maximum)||smb>MAX_FRAME_BYTES||
            smb as u64>u64::from(member.max_transact())+256{return Err(Error::Limit);}
        let mut record=Vec::new();record.try_reserve_exact(4+smb).map_err(|_|Error::Limit)?;
        record.extend_from_slice(&(smb as u32).to_be_bytes());record.extend_from_slice(&head);
        record.extend_from_slice(fixed);record.extend_from_slice(data);
        record.resize(4+smb,0); // padding in the final member's CMAC, never in OutputBufferLength
        Ok(Self{record,sid,original})
    }
    pub(in crate::smb) fn credit(mut self,grant:u16)->PayableDirectory{
        self.record[18..20].copy_from_slice(&grant.to_le_bytes());PayableDirectory(self)
    }
}
impl PayableDirectory {
    pub(in crate::smb) fn sid(&self)->NonZeroU64{self.0.sid}
    pub(in crate::smb) fn for_mac(&self)->&[u8]{&self.0.record[4..]}
    pub(in crate::smb) fn signed(mut self,tag:[u8;16])->SignedDirectory{
        self.0.record[52..68].copy_from_slice(&tag);SignedDirectory(self.0)
    }
}
impl SignedDirectory {
    pub(in crate::smb) fn into_send(self)->(Vec<u8>,PendingRootQuery){(self.0.record,self.0.original)}
}
