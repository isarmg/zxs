//! One standalone QUERY_INFO FileNormalizedNameInformation reply. Only
//! the exact original authorized ordinary FILE or directory name can produce wire bytes.
use super::{Error,Result,direct::MAX_FRAME_BYTES,
    fixed_reply::{QUERY_INFO,information_too_small,standalone_header},
    query_fields::{FileClass,InfoClass}};
use crate::smb::connection::sessions::ordinary::{PendingFileName,VerifiedMember};
use std::num::NonZeroU64;

const MIN_OUTPUT:u32=8; // FSA: align(4-byte length plus one UTF-16 unit, 4)
const HEADER:usize=64;
const OUTPUT_OFFSET:u16=72;
const INFO_LENGTH_MISMATCH:u32=0xc000_0004;
const BUFFER_OVERFLOW:u32=0x8000_0005;

struct NameBody{status:u32,fixed:[u8;16],fixed_len:usize,name_bytes:usize}
impl NameBody{
    fn from_path(path:&str,output:u32)->Result<Self>{
        // Core builds the case-preserving absolute path in the share.
        // Never return a relative basename, empty root or UNC path here.
        if !path.starts_with('\\')||path.len()<2||path.starts_with("\\\\"){return Err(Error::InvalidRequest);}
        let mut fixed=[0;16];
        if output<MIN_OUTPUT{
            information_too_small(&mut fixed);
            return Ok(Self{status:INFO_LENGTH_MISMATCH,fixed,fixed_len:16,name_bytes:0});
        }
        let units=path.encode_utf16().count();
        if units>32_760{return Err(Error::Limit);}
        let full=units*2;
        let available=(output as usize-4)&!1; // whole 16-bit code units
        let name_bytes=full.min(available);
        fixed[..2].copy_from_slice(&9u16.to_le_bytes());
        fixed[2..4].copy_from_slice(&OUTPUT_OFFSET.to_le_bytes());
        fixed[4..8].copy_from_slice(&((4+name_bytes) as u32).to_le_bytes());
        // Keep the full FileNameLength in a partial response, without
        // returning a terminator or more bytes than the original budget.
        fixed[8..12].copy_from_slice(&(full as u32).to_le_bytes());
        let status=if name_bytes<full{BUFFER_OVERFLOW}else{0};
        Ok(Self{status,fixed,fixed_len:12,name_bytes})
    }
    fn len(&self)->usize{self.fixed_len+self.name_bytes}
    fn append(&self,path:&str,out:&mut Vec<u8>){
        out.extend_from_slice(&self.fixed[..self.fixed_len]);
        for unit in path.encode_utf16().take(self.name_bytes/2){out.extend_from_slice(&unit.to_le_bytes());}
    }
}

pub(in crate::smb) struct UnsignedFileName{record:Vec<u8>,sid:NonZeroU64,original:PendingFileName}
pub(in crate::smb) struct PayableFileName(UnsignedFileName);
pub(in crate::smb) struct SignedFileName(UnsignedFileName);
impl UnsignedFileName{
    pub(in crate::smb) fn from_original(member:&VerifiedMember<'_,'_>,original:PendingFileName)->Result<Self>{
        let sid=original.sid();
        if !original.matches(member){return Err(Error::InvalidRequest);}
        let info=member.message().query_info(member.max_transact()).map_err(|_|Error::InvalidRequest)?;
        let (output,path)=original.payload();
        if info.output_bytes!=output||info.class!=InfoClass::File(FileClass::NormalizedName){return Err(Error::InvalidRequest);}
        let body=NameBody::from_path(path,output)?;
        let size=HEADER+body.len();
        if size>MAX_FRAME_BYTES||size as u64>u64::from(member.max_transact())+256{return Err(Error::Limit);}
        let head=standalone_header(member.message(),sid,QUERY_INFO,body.status)?;
        let mut record=Vec::new();record.try_reserve_exact(4+size).map_err(|_|Error::Limit)?;
        record.extend_from_slice(&(size as u32).to_be_bytes());record.extend_from_slice(&head);
        body.append(path,&mut record);
        Ok(Self{record,sid,original})
    }
    pub(in crate::smb) fn credit(mut self,grant:u16)->PayableFileName{
        self.record[18..20].copy_from_slice(&grant.to_le_bytes());PayableFileName(self)
    }
}
impl PayableFileName{
    pub(in crate::smb) fn sid(&self)->NonZeroU64{self.0.sid}
    pub(in crate::smb) fn for_mac(&self)->&[u8]{&self.0.record[4..]}
    pub(in crate::smb) fn signed(mut self,tag:[u8;16])->SignedFileName{
        self.0.record[52..68].copy_from_slice(&tag);SignedFileName(self.0)
    }
}
impl SignedFileName{
    pub(in crate::smb) fn into_send(self)->(Vec<u8>,PendingFileName){(self.0.record,self.0.original)}
}

#[cfg(test)]
#[path="../../test/smb/file_name_reply.rs"]
mod tests;
