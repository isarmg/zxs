//! One paid, standalone SET_INFO rename on its original OPEN. Ordinary
//! FILE, ordinary directory and retained restore use distinct storage
//! authority and receipts. No replacement, purge or raw-path OPEN.
use super::{CheckedBatch,Connection,Fault,Payload,Result,SyncReply,UnsignedSmall,VerifiedMember,current,
    root_io::RequestSlot};
use super::super::opens::Opened;
use crate::smb::{
    backend::{OrdinaryRenameError,PreparedDirectoryRename,PreparedOrdinaryRename,PreparedRetainedRestore,RestoreError,path::NameError},
    wire::{fixed_reply::RestoreFailure,io_fields::{IoFieldError,OpenReference,WireOpenId},io_reply::FileReply,
        message::{Message,RELATED,Routing},path_fields::FieldError,set_info::{Error as SetError,RenameError,RenameRequest}},
};
use std::{io,num::NonZeroU64};

impl CheckedBatch<'_,'_>{
    pub(in crate::smb) fn try_send_named_rename(&mut self)->Result<bool>{
        self.before_reply()?;
        let frame=self.original.frame;
        let [m]=frame.messages()else{return Ok(false);};
        let Routing::Sync{tree:tid}=m.header.routing else{return Ok(false);};
        if m.header.flags&RELATED!=0||!m.is_basic_rename_request(){return Ok(false);}
        let sid=self.ids.at(0).ok_or(Fault::SessionState)?;
        let original=RequestSlot{sid,tree:tid,mid:m.header.message};
        let (pending,draft)={
            let (session,trees,opens)=self.owner.member_tables(sid,&self.revision)?;
            // MS-SMB2 §3.3.5.21: Session, Tree and actual Open in that
            // Session, including the persistent/volatile pair, before SET.
            let Some(tree)=trees.root(tid)else{return self.rename_refusal(m,sid,RestoreFailure::NetworkNameDeleted);};
            let ids=match m.set_info_target(){
                Ok(OpenReference::Explicit(ids))=>ids,
                Err(IoFieldError::NoExplicitOpen)=>return self.rename_refusal(m,sid,RestoreFailure::FileClosed),
                _=>return self.rename_refusal(m,sid,RestoreFailure::InvalidParameter),
            };
            let Some(slot)=opens.find_mut(ids).filter(|slot|slot.tree()==tid)else{
                return self.rename_refusal(m,sid,RestoreFailure::FileClosed);
            };
            match self.original.payload(0){
                Some(Payload::Covered)=>{},
                Some(Payload::InvalidParameter)=>return self.rename_refusal(m,sid,RestoreFailure::InvalidParameter),
                _=>return Err(Fault::SessionState),
            }
            let member=VerifiedMember{message:m,ordinal:0,session};
            let request=match RenameRequest::parse(member){
                Ok(v)=>v,Err((_,e))=>return self.rename_refusal(m,sid,parse_failure(e)?),
            };
            if request.open()!=OpenReference::Explicit(ids){return Err(Fault::WrongReply);}
            let receipt=match slot.object_mut(){
                Opened::File(open)=>match open.rename_standalone(tree,&request){
                    Ok(v)=>NamedReceipt::OrdinaryFile(v),
                    Err(e)=>return self.rename_refusal(m,sid,ordinary_rename_failure(e)?),
                },
                Opened::Directory(open)=>match open.rename_ordinary(tree,&request){
                    Ok(v)=>NamedReceipt::OrdinaryDirectory(v),
                    Err(e)=>return self.rename_refusal(m,sid,ordinary_rename_failure(e)?),
                },
                Opened::Retained(open)=>match open.restore(tree,&request){
                    Ok(v)=>NamedReceipt::Restore(v),
                    Err(e)=>return self.rename_refusal(m,sid,restore_failure(e)?),
                },
                _=>return self.rename_refusal(m,sid,RestoreFailure::NotSupported),
            };
            let pending=PendingRename{original,ids,receipt};
            let draft=UnsignedSmall::named_renamed(request.original(),&pending).map_err(|_|Fault::WrongReply)?;
            (pending,draft)
        };
        self.next=1;
        self.small_replies(std::iter::once(draft))?.send_renamed(pending)?;
        Ok(true)
    }

    pub(super) fn rename_refusal(&mut self,m:&Message<'_>,sid:NonZeroU64,status:RestoreFailure)->Result<bool>{
        self.owner.member_scope(sid,&self.revision)?;
        let draft=UnsignedSmall::rename_refusal(&RenameRefusal{raw:m,sid,status}).map_err(|_|Fault::WrongReply)?;
        self.next=1;self.small_replies(std::iter::once(draft))?.send()?;Ok(true)
    }
}
fn name_failure(e:NameError)->RestoreFailure{
    match e{
        NameError::Field(_)=>RestoreFailure::InvalidParameter,
        NameError::InvalidName=>RestoreFailure::ObjectNameInvalid,
        NameError::TooLong=>RestoreFailure::NameTooLong,
        NameError::Resources=>RestoreFailure::Resources,
    }
}
pub(super) fn parse_failure(e:RenameError)->Result<RestoreFailure>{
    use RestoreFailure as W;
    Ok(match e{
        RenameError::Name(e)=>name_failure(e),
        RenameError::Fields(e)=>match e{
            SetError::Session(e)=>return Err(e),
            SetError::WrongCommand=>return Err(Fault::WrongReply),
            SetError::NotInProfile=>W::NotSupported,
            SetError::InformationLength=>W::InformationLengthMismatch,
            SetError::NoExplicitOpen=>W::FileClosed,
            SetError::Malformed|SetError::ExceedsMaxTransact|SetError::NegativeEof=>W::InvalidParameter,
        },
    })
}
pub(super) fn restore_failure(e:RestoreError)->Result<RestoreFailure>{
    use RestoreFailure as W;
    Ok(match e{
        RestoreError::Session(e)=>return Err(e),
        RestoreError::WrongTree=>W::NetworkNameDeleted,
        RestoreError::NotRetainedFile=>W::NotSupported,
        RestoreError::AccessDenied=>W::AccessDenied,
        RestoreError::Name(e)=>name_failure(e),
        RestoreError::Original(e)=>return rename_io_failure(e),
    })
}
pub(super) fn ordinary_rename_failure(e:OrdinaryRenameError)->Result<RestoreFailure>{
    use RestoreFailure as W;
    Ok(match e{
        OrdinaryRenameError::Session(e)=>return Err(e),
        OrdinaryRenameError::WrongTree=>W::NetworkNameDeleted,
        OrdinaryRenameError::AccessDenied=>W::AccessDenied,
        OrdinaryRenameError::Name(NameError::Field(FieldError::Unsupported))=>W::NotSupported,
        OrdinaryRenameError::Name(e)=>name_failure(e),
        OrdinaryRenameError::SharingViolation=>W::SharingViolation,
        OrdinaryRenameError::DeletePending=>W::DeletePending,
        OrdinaryRenameError::Original(e)=>return rename_io_failure(e),
    })
}
fn rename_io_failure(e:io::Error)->Result<RestoreFailure>{
    use RestoreFailure as W;
    Ok(match e.kind(){
        io::ErrorKind::ConnectionAborted=>return Err(Fault::WriteOutcomeUnknown),
        io::ErrorKind::AlreadyExists=>W::ObjectNameCollision,
        io::ErrorKind::NotFound|io::ErrorKind::NotADirectory=>W::ObjectPathNotFound,
        io::ErrorKind::PermissionDenied=>W::AccessDenied,
        io::ErrorKind::StorageFull=>W::DiskFull,
        io::ErrorKind::ReadOnlyFilesystem=>W::MediaWriteProtected,
        io::ErrorKind::NotConnected=>W::VolumeDismounted,
        io::ErrorKind::WouldBlock=>W::FileNotAvailable,
        io::ErrorKind::OutOfMemory=>W::Resources,
        io::ErrorKind::Unsupported=>W::NotSupported,
        _=>W::ServerUnavailable,
    })
}

pub(in crate::smb) struct RenameRefusal<'a,'r>{raw:&'a Message<'r>,sid:NonZeroU64,status:RestoreFailure}
impl<'a,'r> RenameRefusal<'a,'r>{
    pub(in crate::smb) fn fields(&self)->(&'a Message<'r>,NonZeroU64,RestoreFailure){(self.raw,self.sid,self.status)}
}
#[must_use]
pub(in crate::smb) struct PendingRename{original:RequestSlot,ids:WireOpenId,receipt:NamedReceipt}
enum NamedReceipt{
    OrdinaryFile(PreparedOrdinaryRename),
    OrdinaryDirectory(PreparedDirectoryRename),
    Restore(PreparedRetainedRestore),
}
impl PendingRename{
    pub(in crate::smb) fn sid(&self)->NonZeroU64{self.original.sid}
    pub(in crate::smb) fn matches(&self,member:&VerifiedMember<'_,'_>)->bool{
        self.original.matches(member,self.sid())&&
            member.message().set_info_target().is_ok_and(|id|id==OpenReference::Explicit(self.ids))
    }
    pub(in crate::smb) fn reply(&self)->FileReply<'_>{
        match &self.receipt{
            NamedReceipt::OrdinaryFile(v)=>FileReply::completed_rename(v),
            NamedReceipt::OrdinaryDirectory(v)=>FileReply::completed_directory_rename(v),
            NamedReceipt::Restore(v)=>FileReply::completed_restore(v),
        }
    }
    fn check(&self,c:&Connection)->Result<()>{
        let tree=self.original.root(c)?;
        let slot=c.open_table(self.sid())?.find(self.ids).filter(|s|s.tree()==self.original.tree).ok_or(Fault::SessionState)?;
        match (&self.receipt,slot.object()){
            (NamedReceipt::OrdinaryFile(moved),Opened::File(open))=>moved.require_current(open,tree),
            (NamedReceipt::OrdinaryDirectory(moved),Opened::Directory(open))=>moved.require_current(open,tree),
            (NamedReceipt::Restore(moved),Opened::Retained(open))=>moved.require_current(open,tree),
            _=>return Err(Fault::SessionState),
        }.map_err(|_|Fault::SessionState)
    }
}
impl SyncReply<'_>{
    fn send_renamed(self,pending:PendingRename)->Result<()>{
        // Confirm the same move and held OPEN, never look up an old
        // pathname. An uncertain outcome has no signed status.
        pending.check(self.tx.plan.owner)?;
        let SyncReply{tx,ids,revision,count}=self.write_original()?.0;
        tx.publish_with(|c|{current(c,ids,revision,count)?;pending.check(c)})
    }
}

#[cfg(test)]
#[path="../../../../../../tests/smb/restore_reply.rs"]
mod tests;
