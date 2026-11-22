//! The logical attributes of an original, still held, non-reparse OPEN.
use super::{Grant,Held,Object,Slot,file_open::FileStandardError};
use crate::smb::{RustSessionWitness,connection::sessions::ordinary::VerifiedMember,
    wire::{io_fields::OpenReference,message::{RELATED,Routing},query_fields::{FileClass,InfoClass}}};
use std::{io,sync::{Arc,Weak}};

const READ_ATTRIBUTES:u32=0x0000_0080;
// ZXS's logical unnamed data FILE always has ARCHIVE (MS-FSA §2.1.5.1.1).
// This fixed attribute is the same after birth, reopen, and move to/from R.
// It is not host metadata, a backup claim, or authorization to set attributes
// or EAs. CREATE, QUERY_INFO and directory records use this one projection.
pub(in crate::smb) const FILE_ATTRIBUTES:u32=0x0000_0020; // FILE_ATTRIBUTE_ARCHIVE
pub(in crate::smb) const DIRECTORY_ATTRIBUTES:u32=0x0000_0010; // FILE_ATTRIBUTE_DIRECTORY

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
enum Kind{File,Directory}
impl Kind{
    fn of(slot:&Slot,grant:Grant)->io::Result<Self>{
        // MS-SMB2 §3.3.5.20.1 requires the actual, already granted right.
        // Neither LIST/READ_DATA nor a writable OPEN's other rights qualify.
        if grant.0&READ_ATTRIBUTES==0||!slot.bound||slot.retained||slot.released{
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        match slot.object{
            Object::File(..)=>Ok(Self::File),
            Object::Namespace(..)|Object::OrdinaryRoot(..)|Object::Directory(..)=>Ok(Self::Directory),
            Object::RecycleRoot(..)|Object::DeleteRound(..)=>Err(io::ErrorKind::PermissionDenied.into()),
        }
    }
    fn attributes(self)->u32{
        match self{Self::File=>FILE_ATTRIBUTES,Self::Directory=>DIRECTORY_ATTRIBUTES}
    }
}

#[must_use]
pub(in crate::smb) struct PreparedAttributeTag{open:Weak<()>,scope:RustSessionWitness,output:u32,kind:Kind}
impl PreparedAttributeTag{
    pub(super) fn prepare<T>(held:&Held<T>,member:&VerifiedMember<'_,'_>,
        require:impl FnOnce(&RustSessionWitness)->io::Result<()>,
    )->Result<Self,FileStandardError>{
        let raw=member.message();
        let query=raw.query_info(member.max_transact()).map_err(FileStandardError::Fields)?;
        if query.class!=InfoClass::File(FileClass::AttributeTag)||member.ordinal()!=0||raw.header.flags&RELATED!=0||
            !matches!(raw.header.routing,Routing::Sync{..})||raw.raw.get(20..24)!=Some(&[0;4][..])||
            !matches!(raw.query_info_target(),Ok(OpenReference::Explicit(_))){
            return Err(FileStandardError::NotSupported);
        }
        let scope=member.rust_session_witness().map_err(FileStandardError::Session)?;
        let slot=&held._slot;
        let kind=Kind::of(slot,held.grant).map_err(FileStandardError::Original)?;
        if !scope.is_query_info(){return Err(FileStandardError::Original(io::ErrorKind::PermissionDenied.into()));}
        require(&scope).map_err(FileStandardError::Original)?;
        Ok(Self{open:Arc::downgrade(&slot.identity),scope,output:query.output_bytes,kind})
    }
    pub(in crate::smb) fn values(&self)->(u32,u32,u32){
        // These logical object types have no reparse point, even if the
        // host volume uses different attributes for storing their material.
        (self.output,self.kind.attributes(),0)
    }
    pub(super) fn require<T>(&self,held:&Held<T>,require:impl FnOnce(&RustSessionWitness)->io::Result<()>)
        ->io::Result<()>{
        let slot=&held._slot;
        if !self.scope.is_query_info()||!self.open.ptr_eq(&Arc::downgrade(&slot.identity))||
            Kind::of(slot,held.grant)?!=self.kind{
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        require(&self.scope)
    }
}

#[cfg(test)]
#[path="../../../tests/smb/backend_attribute_tag.rs"]
mod tests;
