//! Paid, signed WRITE/FLUSH/SET_INFO on the originally opened FILE.
//! EOF needs the original writer; basic disposition needs its own DELETE
//! grant and affects only that ordinary FILE's shared open state.
use super::{CheckedBatch,Connection,Fault,Payload,Result,SyncReply,UnsignedSmall,VerifiedMember,current,
    root_io::RequestSlot};
use super::super::opens::Opened;
use crate::smb::{backend::{DispositionError,FileWriteError,PreparedDisposition,PreparedFileEof,PreparedFileFlush,PreparedFileWrite,RangeIoError,WriteFault},
    wire::{fixed_reply::WriteFailure,io_fields::{IoFieldError,OpenReference,WireOpenId},
        io_reply::{FLUSH,FileReply,WRITE},message::{Message,RELATED,Routing},
        set_info::{Change,Error as SetError,FileSetRequest,SET_INFO}}};
use std::{io,num::NonZeroU64};

impl CheckedBatch<'_,'_>{
    pub(in crate::smb) fn try_send_file_write(&mut self)->Result<bool>{
        self.before_reply()?;
        let frame=self.original.frame;
        let [m]=frame.messages() else{return Ok(false);};
        let Routing::Sync{tree:tid}=m.header.routing else{return Ok(false);};
        let verb=m.header.command;
        if !matches!(verb,WRITE|FLUSH|SET_INFO)||m.header.flags&RELATED!=0{return Ok(false);}
        let sid=self.ids.at(0).ok_or(Fault::SessionState)?;
        let original=RequestSlot{sid,tree:tid,mid:m.header.message};
        let (pending,draft)={
            let (scope,trees,opens)=self.owner.member_tables(sid,&self.revision)?;
            // MS-SMB2 §§3.3.5.11/.13/.21: Session, original Tree, and this
            // OPEN's two halves precede data/operation authorization.
            let Some(tree)=trees.root(tid)else{return self.write_refusal(m,sid,WriteFailure::NetworkNameDeleted);};
            let wire=match verb{
                WRITE=>m.write_target(),FLUSH=>m.flush_request().map(|p|p.open),SET_INFO=>m.set_info_target(),
                _=>return Err(Fault::WrongReply),
            };
            let ids=match wire{
                Ok(OpenReference::Explicit(v))=>v,
                Err(IoFieldError::NoExplicitOpen)=>return self.write_refusal(m,sid,WriteFailure::FileClosed),
                _=>return self.write_refusal(m,sid,WriteFailure::InvalidParameter),
            };
            let Some(slot)=opens.find_mut(ids).filter(|s|s.tree()==tid)else{
                return self.write_refusal(m,sid,WriteFailure::FileClosed);
            };
            let payload=self.original.payload(0);
            if verb==SET_INFO{
                match payload{
                    Some(Payload::Covered)=>{},
                    Some(Payload::InvalidParameter)=>return self.write_refusal(m,sid,WriteFailure::InvalidParameter),
                    _=>return Err(Fault::SessionState),
                }
                let member=VerifiedMember{message:m,ordinal:0,session:scope};
                let request=match FileSetRequest::parse(member){
                    Ok(v)=>v,Err((_,e))=>return self.write_refusal(m,sid,set_failure(e)?),
                };
                if request.open()!=OpenReference::Explicit(ids){return Err(Fault::WrongReply);}
                let prepared=match request.change(){
                    Change::EndOfFile(_)=>{
                        let writer=match slot.object_mut(){
                            Opened::Writer(v)=>v,
                            Opened::Retained(r) if r.is_directory()=>return self.write_refusal(m,sid,WriteFailure::InvalidParameter),
                            Opened::File(_)|Opened::Retained(_)=>return self.write_refusal(m,sid,WriteFailure::AccessDenied),
                            Opened::Root(_)|Opened::Directory(_)=>return self.write_refusal(m,sid,WriteFailure::InvalidParameter),
                        };
                        match writer.set_eof(tree,&request){
                            Ok(v)=>Prepared::EndOfFile(v),Err(e)=>return self.write_refusal(m,sid,failure(e)?),
                        }
                    },
                    Change::MarkForDelete|Change::ClearDisposition=>{
                        let changed=match slot.object(){
                            Opened::File(open)=>open.set_disposition(tree,&request),
                            Opened::Writer(open)=>open.set_disposition(tree,&request),
                            // A retained FILE's DELETE is a restore-only grant.
                            Opened::Retained(_)=>return self.write_refusal(m,sid,WriteFailure::AccessDenied),
                            Opened::Root(_)|Opened::Directory(_)=>return self.write_refusal(m,sid,WriteFailure::NotSupported),
                        };
                        match changed{
                            Ok(v)=>Prepared::Disposition(v),Err(e)=>return self.write_refusal(m,sid,disposition_failure(e)?),
                        }
                    },
                };
                let pending=PendingWrite{original,ids,prepared};
                let draft=UnsignedSmall::wrote(request.original(),&pending).map_err(|_|Fault::WrongReply)?;
                (pending,draft)
            }else{
                let Opened::Writer(writer)=slot.object_mut()else{
                    // Actual Root, Directory and read FILE grants contain
                    // no write/append or add permission in this dispatch.
                    return self.write_refusal(m,sid,WriteFailure::AccessDenied);
                };
                if !writer.allows_write(){return self.write_refusal(m,sid,WriteFailure::AccessDenied);}
                match (verb,payload){
                    (WRITE,Some(Payload::Covered))|(FLUSH,Some(Payload::NoVariableRule))=>{},
                    (WRITE,Some(Payload::InvalidParameter))=>return self.write_refusal(m,sid,WriteFailure::InvalidParameter),
                    _=>return Err(Fault::SessionState),
                }
                let member=VerifiedMember{message:m,ordinal:0,session:scope};
                // The actual backend uses this member's distinct MaxWrite,
                // caller flags and private EOF, not this command's routing ID.
                let result=if verb==WRITE{writer.write(tree,&member).map(Prepared::Write)}
                    else{writer.flush(tree,&member).map(Prepared::Flush)};
                let prepared=match result{
                    Ok(v)=>v,Err(e)=>return self.write_refusal(m,sid,failure(e)?),
                };
                let pending=PendingWrite{original,ids,prepared};
                let draft=UnsignedSmall::wrote(&member,&pending).map_err(|_|Fault::WrongReply)?;
                (pending,draft)
            }
        };
        self.next=1;
        self.small_replies(std::iter::once(draft))?.send_file_write(pending)?;
        Ok(true)
    }

    fn write_refusal(&mut self,m:&Message<'_>,sid:NonZeroU64,status:WriteFailure)->Result<bool>{
        self.owner.member_scope(sid,&self.revision)?;
        // A spent but undercharged request gains a signed error, never
        // an executable member or a platform- or client-specific retry.
        let draft=UnsignedSmall::write_refusal(&WriteRefusal{message:m,sid,status}).map_err(|_|Fault::WrongReply)?;
        self.next=1;self.small_replies(std::iter::once(draft))?.send()?;Ok(true)
    }
}

pub(super) fn set_failure(e:SetError)->Result<WriteFailure>{
    use WriteFailure as W;
    Ok(match e{
        SetError::Session(e)=>return Err(e),
        SetError::WrongCommand=>return Err(Fault::WrongReply),
        SetError::NotInProfile=>W::NotSupported,
        SetError::InformationLength=>W::InformationLengthMismatch,
        SetError::NoExplicitOpen=>W::FileClosed,
        SetError::Malformed|SetError::ExceedsMaxTransact|SetError::NegativeEof=>W::InvalidParameter,
    })
}

fn failure(e:FileWriteError)->Result<WriteFailure>{
    use WriteFailure as W;
    Ok(match e{
        FileWriteError::Fields(_)=>W::InvalidParameter,
        FileWriteError::Session(e)=>return Err(e),
        FileWriteError::WrongTree=>W::NetworkNameDeleted,
        FileWriteError::NotSupported=>W::NotSupported,
        FileWriteError::Fault(e)=>match e{
            WriteFault::AccessDenied=>W::AccessDenied,
            WriteFault::NotDataFile|WriteFault::InconsistentLength=>W::InternalError,
            WriteFault::UnbufferedUnavailable|WriteFault::DirectoryFlushUnavailable=>W::NotSupported,
            WriteFault::InvalidWriteCombination|WriteFault::InvalidRange=>W::InvalidParameter,
            WriteFault::Range(e)=>match e{
                RangeIoError::Conflict=>W::FileLockConflict,
                RangeIoError::Resources=>W::Resources,
                RangeIoError::Unavailable=>W::ServerUnavailable,
            },
            WriteFault::Original(e)|WriteFault::Confirmation(e)=>return original_write_failure(e),
        },
    })
}
pub(super) fn disposition_failure(e:DispositionError)->Result<WriteFailure>{
    use WriteFailure as W;
    Ok(match e{
        DispositionError::NotSupported=>W::NotSupported,
        DispositionError::AccessDenied=>W::AccessDenied,
        DispositionError::WrongTree=>W::NetworkNameDeleted,
        DispositionError::Session(e)=>return Err(e),
        DispositionError::Original(e)=>return original_write_failure(e),
    })
}
fn original_write_failure(e:io::Error)->Result<WriteFailure>{
    use WriteFailure as W;
    Ok(match e.kind(){
        io::ErrorKind::ConnectionAborted=>return Err(Fault::WriteOutcomeUnknown),
        io::ErrorKind::PermissionDenied=>W::AccessDenied,
        io::ErrorKind::OutOfMemory=>W::Resources,
        io::ErrorKind::WouldBlock|io::ErrorKind::NotFound=>W::FileNotAvailable,
        io::ErrorKind::StorageFull=>W::DiskFull,
        io::ErrorKind::ReadOnlyFilesystem=>W::MediaWriteProtected,
        io::ErrorKind::NotConnected=>W::VolumeDismounted,
        _=>W::ServerUnavailable,
    })
}

pub(in crate::smb) struct WriteRefusal<'a,'r>{message:&'a Message<'r>,sid:NonZeroU64,status:WriteFailure}
impl<'a,'r> WriteRefusal<'a,'r>{
    pub(in crate::smb) fn fields(&self)->(&'a Message<'r>,NonZeroU64,WriteFailure){(self.message,self.sid,self.status)}
}
enum Prepared{Write(PreparedFileWrite),Flush(PreparedFileFlush),EndOfFile(PreparedFileEof),Disposition(PreparedDisposition)}
#[must_use]
pub(in crate::smb) struct PendingWrite {original:RequestSlot,ids:WireOpenId,prepared:Prepared}
impl PendingWrite{
    pub(in crate::smb) fn sid(&self)->NonZeroU64{self.original.sid}
    pub(in crate::smb) fn matches(&self,m:&VerifiedMember<'_,'_>)->bool{self.original.matches(m,self.sid())}
    pub(in crate::smb) fn reply(&self)->FileReply<'static>{
        match &self.prepared{
            Prepared::Write(p)=>FileReply::completed_write(p.completion()),
            Prepared::Flush(p)=>FileReply::completed_flush(p.completion()),
            Prepared::EndOfFile(p)=>FileReply::completed_end_of_file(p),
            Prepared::Disposition(p)=>FileReply::completed_disposition(p),
        }
    }
    fn check(&self,c:&Connection)->Result<()>{
        let tree=self.original.root(c)?;
        let slot=c.open_table(self.sid())?.find(self.ids).filter(|s|s.tree()==self.original.tree).ok_or(Fault::SessionState)?;
        match (&self.prepared,slot.object()){
            (Prepared::Write(p),Opened::Writer(open))=>p.require_current(open,tree),
            (Prepared::Flush(p),Opened::Writer(open))=>p.require_current(open,tree),
            (Prepared::EndOfFile(p),Opened::Writer(open))=>p.require_current(open,tree),
            (Prepared::Disposition(p),Opened::File(open))=>p.require_file(open,tree),
            (Prepared::Disposition(p),Opened::Writer(open))=>p.require_writer(open,tree),
            _=>return Err(Fault::SessionState),
        }.map_err(|_|Fault::SessionState)
    }
}
impl SyncReply<'_>{
    fn send_file_write(self,done:PendingWrite)->Result<()>{
        done.check(self.tx.plan.owner)?;
        let SyncReply{tx,ids,revision,count}=self.write_original()?.0;
        // The backend result may already be durable. A failed/late send
        // does not announce it or retry the store; drop only this owner.
        tx.publish_with(|c|{current(c,ids,revision,count)?;done.check(c)})
    }
}

#[cfg(test)]
#[path="../../../../../../tests/smb/ambiguous_write.rs"]
mod tests;
