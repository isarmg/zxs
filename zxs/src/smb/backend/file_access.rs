//! MS-FSA §2.1.5.12.1: the grant of one existing ordinary FILE OPEN.
//! Not a share ceiling, authorization check for a new right, or draft view.
use super::{Grant,Held,Object,Slot,file_open::FileStandardError};
use crate::smb::{RustSessionWitness,connection::sessions::ordinary::VerifiedMember,
    wire::{io_fields::OpenReference,message::{Message,RELATED,Routing},query_fields::{FileClass,InfoClass}}};
use std::{io,sync::{Arc,Weak}};

fn request_output(raw:&Message<'_>,ordinal:usize,maximum:u32)->Result<u32,FileStandardError>{
    let query=raw.query_info(maximum).map_err(FileStandardError::Fields)?;
    if query.class!=InfoClass::File(FileClass::Access)||ordinal!=0||raw.header.flags&RELATED!=0||
        !matches!(raw.header.routing,Routing::Sync{..})||raw.raw.get(20..24)!=Some(&[0;4][..])||
        !matches!(raw.query_info_target(),Ok(OpenReference::Explicit(_))){
        return Err(FileStandardError::NotSupported);
    }
    Ok(query.output_bytes)
}

#[derive(Eq,PartialEq)]
struct Access{file:Object,grant:Grant}
impl Access{
    fn of(slot:&Slot,grant:Grant,file:Object)->io::Result<Self>{
        if !slot.bound||slot.retained||slot.released||!matches!(file,Object::File(..))||slot.object!=file{
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(Self{file,grant})
    }
    fn on(&self,open:&Weak<()>,slot:&Slot,grant:Grant,file:Object)->io::Result<()> {
        if !open.ptr_eq(&Arc::downgrade(&slot.identity))||Self::of(slot,grant,file)?!=*self{
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(())
    }
}

#[must_use]
pub(in crate::smb) struct PreparedFileAccess{open:Weak<()>,scope:RustSessionWitness,output:u32,access:Access}
impl PreparedFileAccess{
    pub(super) fn prepare<T>(held:&Held<T>,file:Object,member:&VerifiedMember<'_,'_>,
        require:impl FnOnce(&RustSessionWitness)->io::Result<()>)->Result<Self,FileStandardError>{
        let output=request_output(member.message(),member.ordinal(),member.max_transact())?;
        let scope=member.rust_session_witness().map_err(FileStandardError::Session)?;
        let access=Access::of(&held._slot,held.grant,file).map_err(FileStandardError::Original)?;
        let proof=Self{open:Arc::downgrade(&held._slot.identity),scope,output,access};
        // Unlike FileBasic/All/NetworkOpen/AttributeTag, this metadata
        // does not need FILE_READ_ATTRIBUTES or permission to read data.
        proof.require(held,file,require).map_err(FileStandardError::Original)?;
        Ok(proof)
    }
    pub(in crate::smb) fn values(&self)->(u32,u32){(self.output,self.access.grant.0)}
    pub(super) fn require<T>(&self,held:&Held<T>,file:Object,
        require:impl FnOnce(&RustSessionWitness)->io::Result<()>)->io::Result<()>{
        if !self.scope.is_query_info(){return Err(io::ErrorKind::PermissionDenied.into());}
        self.access.on(&self.open,&held._slot,held.grant,file)?;
        require(&self.scope)
    }
}

#[cfg(test)]
#[path="../../test/smb/backend_file_access.rs"]
mod tests;
