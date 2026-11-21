//! One actually bound ordinary FILE and its existing Core staging engine.
//! No open-by-number, persistent mirror, automatic SAVE on Drop or history.
use super::{
    BindError,Facts,Grant,Held,Object,OpenCoordinator,OriginalRequest,ParentWrite,ReserveError,Share,
    attribute_tag::PreparedAttributeTag,
    file_access::PreparedFileAccess,
    disposition::{DELETE_ACCESS,DispositionError,PreparedDisposition,Snapshot},
    file_open::{EXECUTE,FILE_READ_RIGHTS,FileAnswers,FileCloseError,FileOpenError,FileReadError,FileStandardError,MAXIMUM_ALLOWED,READ_DATA,
        allocation,normalized_name_error,normalized_query},
    io::{CheckedWrite,CloseCompletion,FlushCompletion,ReadCompletion,ReadFault,WriteCompletion,WriteFault,
        WriteOpenMode,flush_held,seal_read,write_held},
};
use crate::{
    smb::{RustSessionWitness,connection::{Fault as ConnectionFault,sessions::ordinary::VerifiedMember},
        wire::{io_fields::IoFieldError,query_fields::{FileClass,InfoClass},set_info::{Change,FileSetRequest}}},
    system::reader::smb::{RustStagedName,RustStagedReadProof,RustStagedStandard,RustStagedState,RustStagedWriter,
        file_number::SmbFileIndex,runtime::RustRoot,work_change::RustWriterSelection},
};
use std::{io as stdio,sync::Arc};

const WRITE_DATA:u32=2;
const APPEND_DATA:u32=4;
const WRITE_RIGHTS:u32=WRITE_DATA|APPEND_DATA;
pub(super) const FILE_RIGHTS:u32=FILE_READ_RIGHTS|WRITE_RIGHTS|DELETE_ACCESS;

pub(super) fn explicit_writer(desired:u32)->bool{desired&(WRITE_RIGHTS|0x4000_0000)!=0}
pub(super) fn write_grant(desired:u32)->Result<Grant,FileOpenError>{
    // No claim to WRITE_EA/WRITE_ATTRIBUTES/ACL or generic access not
    // backed by this object's actual operations.
    if desired&!(FILE_RIGHTS|MAXIMUM_ALLOWED)!=0{return Err(FileOpenError::AccessDenied);}
    let rights=if desired&MAXIMUM_ALLOWED!=0{FILE_RIGHTS}
        else{desired|if desired&EXECUTE!=0{READ_DATA}else{0}};
    if rights&WRITE_RIGHTS==0{return Err(FileOpenError::AccessDenied);}
    Grant::from_effective(rights).ok_or(FileOpenError::AccessDenied)
}

struct Opening<'a>{tree:&'a RustRoot,member:&'a RustSessionWitness,facts:Facts}
impl OriginalRequest for Opening<'_>{
    type Held=RustStagedWriter;
    type Error=FileOpenError;
    fn recheck(&self,writer:&RustStagedWriter)->Result<Facts,FileOpenError>{
        if !writer.belongs_to(self.tree)||self.facts.object!=Object::File(self.tree.pool(),writer.file()){
            return Err(FileOpenError::WrongTree);
        }
        writer.require_current(self.tree,self.member).map_err(FileOpenError::Original)?;
        Ok(self.facts)
    }
}

#[must_use]
pub(in crate::smb) struct WritableFileOpen{held:Held<RustStagedWriter>,mode:WriteOpenMode,answer_maximal:bool,disk:Option<SmbFileIndex>}
impl OpenCoordinator{
    pub(super) fn bind_writable(self:&Arc<Self>,tree:&RustRoot,member:&RustSessionWitness,
        original:RustWriterSelection<'_>,grant:Grant,share:Share,write_through:bool,delete_on_close:bool,answers:FileAnswers,
    )->Result<WritableFileOpen,FileOpenError>{
        let facts=Facts{object:Object::File(tree.pool(),original.file()),grant,parent_write:ParentWrite::Granted};
        let request=Opening{tree,member,facts};
        let pending=self.reserve(&request,facts,share).map_err(|e|match e{
            ReserveError::SharingViolation=>FileOpenError::ShareViolation,
            ReserveError::DeletePending=>FileOpenError::DeletePending,
            ReserveError::Resources=>FileOpenError::Resources,
            ReserveError::Unavailable=>FileOpenError::Original(stdio::Error::other("unavailable original OPEN coordinator")),
        })?;
        // The global vote is not the true FILE writer. Acquiring the
        // original nonblocking FILE EX can still reject even a compatible vote.
        let writer=original.acquire().map_err(FileOpenError::Original)?;
        let stage=RustStagedWriter::open(writer,tree,member).map_err(FileOpenError::Original)?;
        let held=pending.bind(stage).map_err(|e|match e{
            BindError::Original(e)=>e,BindError::Changed=>FileOpenError::WrongTree,
        })?;
        let open=WritableFileOpen::from_bound(held,write_through,answers,tree,member).map_err(FileOpenError::Original)?;
        if delete_on_close{
            let plan=tree.plan_writer_delete(&open.held.original,member).map_err(FileOpenError::Original)?;
            open.held.enroll_close_option(plan).map_err(FileOpenError::Original)?;
        }
        Ok(open)
    }
}

#[derive(Debug)]
pub(in crate::smb) enum FileWriteError{
    Fields(IoFieldError),Session(ConnectionFault),WrongTree,Fault(WriteFault<stdio::Error>),NotSupported,
}
#[must_use]
pub(in crate::smb) struct PreparedFileWrite{result:WriteCompletion,original:OriginalTurn}
#[must_use]
pub(in crate::smb) struct PreparedFileFlush{result:FlushCompletion,original:OriginalTurn}
#[must_use]
pub(in crate::smb) struct PreparedFileEof{original:OriginalTurn}
#[must_use]
pub(in crate::smb) struct PreparedWriterRead{
    result:Result<ReadCompletion<Vec<u8>>,ReadFault<stdio::Error>>,
    proof:RustStagedReadProof,scope:RustSessionWitness,
}
#[must_use]
pub(in crate::smb) struct PreparedWriterStandard{
    original:RustStagedStandard,scope:RustSessionWitness,output:u32,delete_pending:bool,
}
impl PreparedWriterStandard{
    pub(in crate::smb) fn values(&self)->(u32,u64,u64,bool){
        let eof=self.original.eof();(self.output,allocation(eof),eof,self.delete_pending)
    }
    pub(in crate::smb) fn require_current(&self,open:&WritableFileOpen,tree:&RustRoot)->stdio::Result<()>{
        open.held.original.require_standard(tree,&self.scope,&self.original)?;
        if open.held._slot.pending_delete()?!=self.delete_pending{return Err(stdio::ErrorKind::WouldBlock.into());}
        Ok(())
    }
}
#[must_use]
pub(in crate::smb) struct PreparedWriterName{view:RustStagedName,scope:RustSessionWitness,output:u32}
impl PreparedWriterName{
    pub(in crate::smb) fn output(&self)->u32{self.output}
    pub(in crate::smb) fn path(&self)->&str{self.view.path()}
    pub(in crate::smb) fn require_current(&self,open:&WritableFileOpen,tree:&RustRoot)->stdio::Result<()>{
        open.held.original.require_normalized_name(tree,&self.scope,&self.view)
    }
}
struct OriginalTurn{scope:RustSessionWitness,view:RustStagedState}
impl OriginalTurn{
    fn accepted(scope:RustSessionWitness,open:&WritableFileOpen)->Self{
        // Constructed only after actual acceptance or confirmed FLUSH.
        // Its exact state must stay current through the original sender;
        // no later failure is reported as if storage rejected the turn.
        Self{scope,view:open.held.original.state()}
    }
    fn require_current(&self,open:&WritableFileOpen,tree:&RustRoot)->stdio::Result<()>{
        open.held.original.require_state(tree,&self.scope,&self.view)
    }
}
impl PreparedFileWrite{
    pub(in crate::smb) fn completion(&self)->&WriteCompletion{&self.result}
    pub(in crate::smb) fn require_current(&self,open:&WritableFileOpen,tree:&RustRoot)->stdio::Result<()>{
        self.original.require_current(open,tree)
    }
}
impl PreparedFileFlush{
    pub(in crate::smb) fn completion(&self)->&FlushCompletion{&self.result}
    pub(in crate::smb) fn require_current(&self,open:&WritableFileOpen,tree:&RustRoot)->stdio::Result<()>{
        self.original.require_current(open,tree)
    }
}
impl PreparedFileEof{
    pub(in crate::smb) fn require_current(&self,open:&WritableFileOpen,tree:&RustRoot)->stdio::Result<()>{
        self.original.require_current(open,tree)
    }
}
impl PreparedWriterRead{
    pub(in crate::smb) fn result(&self)->&Result<ReadCompletion<Vec<u8>>,ReadFault<stdio::Error>>{&self.result}
    pub(in crate::smb) fn require_current(&self,open:&WritableFileOpen,tree:&RustRoot)->stdio::Result<()>{
        open.held.original.require_read(tree,&self.scope,&self.proof)
    }
}

struct WriteTurn<'t>{
    actual:&'t mut RustStagedWriter,mode:WriteOpenMode,tree:&'t RustRoot,scope:&'t RustSessionWitness,
}
impl CheckedWrite for WriteTurn<'_>{
    type Error=stdio::Error;
    fn writer_eof_checked(&self)->stdio::Result<u64>{self.actual.checked_write_eof(self.tree,self.scope)}
    fn write_open_mode(&self)->WriteOpenMode{self.mode}
    fn write_checked(&mut self,offset:u64,input:&[u8])->stdio::Result<u32>{
        self.actual.write(self.tree,self.scope,offset,input)
    }
    fn flush_checked(&mut self)->stdio::Result<()>{self.actual.flush(self.tree,self.scope)}
}

/// An invalid CLOSE before the original is taken can keep the OPEN.
/// Once the exact explicit close uses its FILE, no failure restores it.
pub(in crate::smb) enum WritableCloseFault{
    NotStarted(WritableFileOpen,FileCloseError),
    Retired(FileCloseError),
}

impl WritableFileOpen{
    pub(super) fn from_bound(held:Held<RustStagedWriter>,write_through:bool,answers:FileAnswers,
        tree:&RustRoot,scope:&RustSessionWitness)->stdio::Result<Self>{
        let mode=if write_through{WriteOpenMode::BufferedWriteThrough}else{WriteOpenMode::Buffered};
        let disk=if answers.disk{Some(held.original.smb_index(tree,scope)?)}else{None};
        Ok(Self{held,mode,answer_maximal:answers.maximal(),disk})
    }
    pub(in crate::smb) fn maximal_access_answer(&self)->Option<u32>{
        // The existing writer's exact mode and physical writability are
        // both rechecked by require_current on each side of CREATE output.
        self.answer_maximal.then_some(FILE_RIGHTS)
    }
    pub(in crate::smb) fn disk_id_answer(&self)->Option<(u64,u64)>{self.disk.map(SmbFileIndex::qfid)}
    pub(in crate::smb) fn smb_index(&self,tree:&RustRoot,scope:&RustSessionWitness)->stdio::Result<SmbFileIndex>{
        self.held.original.smb_index(tree,scope)
    }
    pub(in crate::smb) fn allows_write(&self)->bool{self.held.grant.0&WRITE_RIGHTS!=0}
    pub(in crate::smb) fn allows_read_data(&self)->bool{self.held.grant.0&READ_DATA!=0}
    pub(in crate::smb) fn size(&self)->u64{self.held.original.eof()}
    pub(in crate::smb) fn logical_allocation(&self)->u64{allocation(self.size())}
    pub(in crate::smb) fn prepare_standard(&self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<PreparedWriterStandard,FileStandardError>{
        let query=member.message().query_info(member.max_transact()).map_err(FileStandardError::Fields)?;
        if query.class!=InfoClass::File(FileClass::Standard){return Err(FileStandardError::NotSupported);}
        // An actually held OPEN suffices; this class does not require
        // FILE_READ_ATTRIBUTES or permission to read the staged bytes.
        let scope=member.rust_session_witness().map_err(FileStandardError::Session)?;
        let original=self.held.original.standard(tree,&scope).map_err(FileStandardError::Original)?;
        let delete_pending=self.held._slot.pending_delete().map_err(FileStandardError::Original)?;
        Ok(PreparedWriterStandard{original,scope,output:query.output_bytes,delete_pending})
    }
    pub(in crate::smb) fn prepare_normalized_name(&self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<PreparedWriterName,FileStandardError>{
        let (scope,output)=normalized_query(member)?;
        let view=self.held.original.normalized_name(tree,&scope).map_err(normalized_name_error)?;
        Ok(PreparedWriterName{view,scope,output})
    }
    pub(in crate::smb) fn prepare_attribute_tag(&self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<PreparedAttributeTag,FileStandardError>{
        // These fixed attributes describe the held logical FILE, not the
        // private write draft. Do not refresh, flush or report its bytes.
        PreparedAttributeTag::prepare(&self.held,member,|scope|self.held.original.require_current(tree,scope))
    }
    pub(in crate::smb) fn require_attribute_tag(&self,tree:&RustRoot,proof:&PreparedAttributeTag)->stdio::Result<()>{
        proof.require(&self.held,|scope|self.held.original.require_current(tree,scope))
    }
    pub(in crate::smb) fn prepare_file_access(&self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<PreparedFileAccess,FileStandardError>{
        // The grant is immutable and belongs to this writer, not the state
        // or permissions of its separate base reader or private staging.
        PreparedFileAccess::prepare(&self.held,Object::File(tree.pool(),self.held.original.file()),member,
            |scope|self.require_current(tree,scope))
    }
    pub(in crate::smb) fn require_file_access(&self,tree:&RustRoot,proof:&PreparedFileAccess)->stdio::Result<()>{
        proof.require(&self.held,Object::File(tree.pool(),self.held.original.file()),|scope|self.require_current(tree,scope))
    }
    pub(in crate::smb) fn require_current(&self,tree:&RustRoot,member:&RustSessionWitness)->stdio::Result<()>{
        self.held.original.require_current(tree,member)?;
        if member.is_create(){
            if let Some(expected)=self.disk && self.smb_index(tree,member)?!=expected{
                return Err(stdio::ErrorKind::PermissionDenied.into());
            }
            self.held._slot.require_publishable_file(tree,member)?;
        }
        Ok(())
    }
    pub(in crate::smb) fn set_disposition(&self,tree:&RustRoot,request:&FileSetRequest<'_,'_>)
        ->Result<PreparedDisposition,DispositionError>{
        if !self.held.original.belongs_to(tree){return Err(DispositionError::WrongTree);}
        self.held.set_disposition(tree,request,|f,scope|f.require_current(tree,scope),
            |f,scope|tree.plan_writer_delete(f,scope))
    }
    pub(super) fn require_disposition(&self,tree:&RustRoot,scope:&RustSessionWitness,snapshot:&Snapshot)->stdio::Result<()>{
        self.held.original.require_current(tree,scope)?;
        self.held.require_disposition(tree,scope,snapshot)
    }
    fn begin(&self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)->Result<RustSessionWitness,FileWriteError>{
        if !self.held.original.belongs_to(tree){return Err(FileWriteError::WrongTree);}
        member.rust_session_witness().map_err(FileWriteError::Session)
    }
    pub(in crate::smb) fn write(&mut self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<PreparedFileWrite,FileWriteError>{
        let request=member.message().write_request(member.max_write()).map_err(FileWriteError::Fields)?;
        let scope=self.begin(tree,member)?;
        let result=write_held(self.held._slot.object,self.held.grant,&mut WriteTurn{
            actual:&mut self.held.original,mode:self.mode,tree,scope:&scope,
        },request.data).map_err(FileWriteError::Fault)?;
        Ok(PreparedFileWrite{result,original:OriginalTurn::accepted(scope,self)})
    }
    pub(in crate::smb) fn flush(&mut self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<PreparedFileFlush,FileWriteError>{
        member.message().flush_request().map_err(FileWriteError::Fields)?;
        let scope=self.begin(tree,member)?;
        let result=flush_held(self.held._slot.object,self.held.grant,&mut WriteTurn{
            actual:&mut self.held.original,mode:self.mode,tree,scope:&scope,
        }).map_err(FileWriteError::Fault)?;
        Ok(PreparedFileFlush{result,original:OriginalTurn::accepted(scope,self)})
    }
    pub(in crate::smb) fn set_eof(&mut self,tree:&RustRoot,request:&FileSetRequest<'_,'_>)
        ->Result<PreparedFileEof,FileWriteError>{
        let Change::EndOfFile(end)=request.change() else{return Err(FileWriteError::NotSupported);};
        if self.held.grant.0&WRITE_DATA==0{return Err(FileWriteError::Fault(WriteFault::AccessDenied));}
        let scope=self.begin(tree,request.original())?;
        self.held.original.resize(tree,&scope,end).map_err(|e|FileWriteError::Fault(WriteFault::Original(e)))?;
        Ok(PreparedFileEof{original:OriginalTurn::accepted(scope,self)})
    }
    pub(in crate::smb) fn read(&mut self,tree:&RustRoot,member:&VerifiedMember<'_,'_>,room:usize)
        ->Result<PreparedWriterRead,FileReadError>{
        let request=member.message().read_request(member.max_read()).map_err(FileReadError::Fields)?;
        if !self.held.original.belongs_to(tree){return Err(FileReadError::WrongTree);}
        let scope=member.rust_session_witness().map_err(FileReadError::Session)?;
        self.held.check_read(request.range,room).map_err(FileReadError::Fault)?;
        let (data,proof)=self.held.original.read(tree,&scope,request.range.offset(),request.range.length())
            .map_err(|e|FileReadError::Fault(ReadFault::Original(e)))?;
        let result=match seal_read(request.range,data){
            Ok(data)=>Ok(data),Err(ReadFault::EndOfFile)=>Err(ReadFault::EndOfFile),Err(e)=>return Err(FileReadError::Fault(e)),
        };
        Ok(PreparedWriterRead{result,proof,scope})
    }
    pub(in crate::smb) fn close(self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<CloseCompletion,WritableCloseFault>{
        let how=(||{
            let request=member.message().close_request().map_err(FileCloseError::Fields)?;
            if !self.held.original.belongs_to(tree){return Err(FileCloseError::WrongTree);}
            let scope=member.rust_session_witness().map_err(FileCloseError::Session)?;
            if !scope.is_close(){return Err(FileCloseError::WrongTree);}
            tree.ensure_current(&scope).map_err(FileCloseError::Original)?;
            Ok((scope,request.mode))
        })();
        let (scope,how)=match how{Ok(b)=>b,Err(e)=>return Err(WritableCloseFault::NotStarted(self,e))};
        self.held.close_file_with(how,tree,&scope,|stage,_|{
            stage.flush(tree,&scope)?;
            // There is no complete authoritative logical timestamp group.
            Ok(None)
        }).map_err(|e:stdio::Error|WritableCloseFault::Retired(FileCloseError::Original(e)))
    }
}

#[cfg(test)]
#[path="../../test/smb/backend_writable_file.rs"]
mod tests;
