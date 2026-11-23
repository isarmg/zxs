//! Existing ordinary FILE only. Shape belongs to the original paid member,
//! placement and staged read proof belong to Core, and the separate protocol
//! FileId belongs to the caller's actual Session.OpenTable.
use super::{BindError,Facts,Grant,Held,Object,OpenCoordinator,OriginalRequest,ParentWrite,ReserveError,Share,
    attribute_tag::PreparedAttributeTag,
    file_access::PreparedFileAccess,
    range_lock::{LockError,PreparedLock,RangeIoError},
    disposition::{DELETE_ACCESS,DELETE_ON_CLOSE,DispositionError,PreparedDisposition,Snapshot},
    io::{CloseCompletion,ReadCompletion,ReadFault,seal_read},
    path::{NameError,TreeName},writable_file_open::{self,WritableFileOpen}};
use crate::{fs::MAX_PATH_DEPTH,smb::{RustSessionWitness,
    connection::{Fault as ConnectionFault,sessions::ordinary::VerifiedMember},
    wire::{create_fields::{CurrentIntent,CurrentName,Disposition,Error as CreateError,MaximumAccessQuery,SpecialOpen},io_fields::{IoFieldError,OpenReference},
        lock_fields::LockRequest,message::{Message,RELATED,Routing},query_fields::{FileClass,InfoClass,QueryError},set_info::FileSetRequest}},
    system::reader::smb::{file_number::SmbFileIndex,first_work::{RustFileName,RustFileStandard,RustReadFile,RustReadProof},runtime::RustRoot}};
use std::{io as stdio,sync::Arc};

pub(super) const READ_DATA:u32=0x0000_0001;
pub(super) const EXECUTE:u32=0x0000_0020;
const READ_ATTRIBUTES:u32=0x0000_0080;
const SYNCHRONIZE:u32=0x0010_0000;
pub(super) const MAXIMUM_ALLOWED:u32=0x0200_0000;
pub(super) const FILE_READ_RIGHTS:u32=READ_DATA|EXECUTE|READ_ATTRIBUTES|SYNCHRONIZE;
pub(super) const FILE_OPTIONS:u32=0x0020_4846|DELETE_ON_CLOSE;

#[derive(Debug)]
pub(in crate::smb) enum FileOpenError{
    Fields(CreateError),Special(SpecialOpen),Name(NameError),NotFile,ContextRequired,UnsupportedOptions,
    AccessDenied,ShareViolation,DeletePending,Resources,Range(RangeIoError),Session(ConnectionFault),Original(stdio::Error),WrongTree,
}
#[derive(Debug)]
pub(in crate::smb) enum FileReadError {
    Fields(IoFieldError),Session(ConnectionFault),WrongTree,Fault(ReadFault<stdio::Error>),
}
#[derive(Debug)]
pub(in crate::smb) enum FileCloseError{
    Fields(IoFieldError),Session(ConnectionFault),Original(stdio::Error),WrongTree,
}
/// The original OPEN can only be restored after a pre-operation refusal.
pub(in crate::smb) enum FileCloseFault{
    NotStarted(FileOpen,FileCloseError),
    Retired(FileCloseError),
}
#[derive(Debug)]
pub(in crate::smb) enum FileStandardError{Fields(QueryError),NotSupported,Session(ConnectionFault),Original(stdio::Error)}

fn read_grant(requested:u32,allows_delete:bool)->Result<Grant,FileOpenError>{
    let allowed=FILE_READ_RIGHTS|if allows_delete{DELETE_ACCESS}else{0};
    if requested==0||requested&!(allowed|MAXIMUM_ALLOWED)!=0{return Err(FileOpenError::AccessDenied);}
    let rights=if requested&MAXIMUM_ALLOWED!=0{allowed}
        else{requested|if requested&EXECUTE!=0{READ_DATA}else{0}}; // MS-SMB2 3.3.5.9
    Grant::from_effective(rights).ok_or(FileOpenError::AccessDenied)
}
fn file_ceiling(writable:bool)->u32{
    if writable{writable_file_open::FILE_RIGHTS}else{FILE_READ_RIGHTS}
}
pub(super) fn allocation(eof:u64)->u64{
    // Product virtual display allocation, NOT physical FILE usage or free
    // space. Round in 4KiB units and stay in the signed size field domain.
    const UNIT:u64=4096;const LIMIT:u64=(i64::MAX as u64)&!(UNIT-1);
    (eof.min(LIMIT)+(UNIT-1))&!(UNIT-1)
}

struct Opening<'a>{tree:&'a RustRoot,member:&'a RustSessionWitness,facts:Facts}
impl OriginalRequest for Opening<'_>{
    type Held=RustReadFile;
    type Error=FileOpenError;
    fn recheck(&self,file:&RustReadFile)->Result<Facts,FileOpenError>{
        if !file.belongs_to(self.tree)||self.facts.object!=Object::File(self.tree.pool(),file.file()){
            return Err(FileOpenError::WrongTree);
        }
        file.require_current(self.tree,self.member).map_err(FileOpenError::Original)?;
        Ok(Facts{parent_write:parent_write(file.parent_allows_file_write()),..self.facts})
    }
}
fn parent_write(allows:bool)->ParentWrite{if allows{ParentWrite::Granted}else{ParentWrite::Denied}}
pub(super) fn with_path<T>(raw:&[u8],choose:impl FnOnce(&str,&[&str],&str)->Result<T,FileOpenError>)
    ->Result<T,FileOpenError>{
    let path=TreeName::create_relative(raw).map_err(FileOpenError::Name)?;
    let mut parts=path.components();
    let label=parts.next().ok_or(FileOpenError::NotFile)?;
    let leaf=parts.next_back().ok_or(FileOpenError::NotFile)?;
    let mut parents=Vec::new();
    for name in parts{
        if parents.len()>=MAX_PATH_DEPTH-1{return Err(FileOpenError::Name(NameError::TooLong));}
        parents.try_reserve(1).map_err(|_|FileOpenError::Resources)?;
        parents.push(name);
    }
    choose(label,&parents,leaf)
}

#[must_use]
pub(in crate::smb) enum PreparedNamedFile{Read(FileOpen),Write(WritableFileOpen)}
#[derive(Clone,Copy)]
pub(super) struct FileAnswers{maximal:bool,pub(super) disk:bool}
impl FileAnswers{
    pub(super) fn for_intent(intent:CurrentIntent<'_>)->Result<Self,FileOpenError>{
        let maximal=match intent.maximal_access(){
            None=>false,
            Some(MaximumAccessQuery::Unconditional)=>true,
            Some(MaximumAccessQuery::CompareChangeTime(_))=>return Err(FileOpenError::ContextRequired),
        };
        Ok(Self{maximal,disk:intent.disk_id()})
    }
    pub(super) fn maximal(self)->bool{self.maximal}
}
#[must_use]
pub(in crate::smb) struct FileOpen{held:Held<RustReadFile>,maximal:Option<u32>,disk:Option<SmbFileIndex>}
impl OpenCoordinator{
    pub(in crate::smb) fn prepare_file_open(self:&Arc<Self>,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<PreparedNamedFile,FileOpenError>{
        let fields=member.message().create_fields().map_err(FileOpenError::Fields)?;
        let intent=fields.current_intent().map_err(FileOpenError::Special)?;
        let CurrentName::Relative(raw)=intent.name() else{return Err(FileOpenError::NotFile);};
        let answers=FileAnswers::for_intent(intent)?;
        let options=fields.effective_options();
        if fields.disposition()!=Disposition::Open||options&!FILE_OPTIONS!=0{
            return Err(FileOpenError::UnsupportedOptions);
        }
        let desired=fields.desired_access();
        let share=Share::from_bits(u32::from(fields.share_access())).ok_or(FileOpenError::AccessDenied)?;
        let scope=member.rust_session_witness().map_err(FileOpenError::Session)?;
        let write_through=options&2!=0; // original FILE_WRITE_THROUGH
        let delete_on_close=options&DELETE_ON_CLOSE!=0;
        if delete_on_close&&desired&DELETE_ACCESS==0{return Err(FileOpenError::AccessDenied);}
        if writable_file_open::explicit_writer(desired){
            let access=writable_file_open::write_grant(desired)?;
            let original=with_path(raw,|a,b,c|tree.choose_existing_writer(&scope,a,b,c)
                .map_err(FileOpenError::Original))?;
            return self.bind_writable(tree,&scope,original,access,share,write_through,delete_on_close,answers).map(PreparedNamedFile::Write);
        }
        let original=with_path(raw,|a,b,c|tree.choose_read_file(&scope,a,b,c).map_err(FileOpenError::Original))?;
        let parent=parent_write(original.parent_allows_file_write());
        if desired&MAXIMUM_ALLOWED!=0 && parent==ParentWrite::Granted {
            let access=writable_file_open::write_grant(desired)?;
            let original=original.select_writer().map_err(FileOpenError::Original)?;
            return self.bind_writable(tree,&scope,original,access,share,write_through,delete_on_close,answers).map(PreparedNamedFile::Write);
        }
        let access=read_grant(desired,parent==ParentWrite::Granted)?;
        let facts=Facts{object:Object::File(tree.pool(),original.file()),grant:access,parent_write:parent};
        let opening=Opening{tree,member:&scope,facts};
        let ticket=self.reserve(&opening,facts,share).map_err(|e|match e{
            ReserveError::SharingViolation=>FileOpenError::ShareViolation,
            ReserveError::DeletePending=>FileOpenError::DeletePending,
            ReserveError::Resources=>FileOpenError::Resources,
            ReserveError::Unavailable=>FileOpenError::Original(stdio::Error::other("unavailable original OPEN coordinator")),
        })?;
        let file=original.acquire().map_err(FileOpenError::Original)?;
        let held=ticket.bind(file).map_err(|e|match e{
            BindError::Original(e)=>e,BindError::Changed=>FileOpenError::WrongTree,
        })?;
        let maximal=if answers.maximal||access.0&DELETE_ACCESS!=0{
            let writable=held.original.allows_writable_policy(tree,&scope).map_err(FileOpenError::Original)?;
            if !writable&&access.0&DELETE_ACCESS!=0{
                return Err(FileOpenError::Original(stdio::ErrorKind::ReadOnlyFilesystem.into()));
            }
            answers.maximal.then_some(file_ceiling(writable))
        }else{None};
        let disk=if answers.disk{Some(held.original.smb_index(tree,&scope).map_err(FileOpenError::Original)?)}else{None};
        if delete_on_close{
            let plan=tree.plan_read_file_delete(&held.original,&scope).map_err(FileOpenError::Original)?;
            held.enroll_close_option(plan).map_err(FileOpenError::Original)?;
        }
        Ok(PreparedNamedFile::Read(FileOpen{held,maximal,disk}))
    }
}

#[must_use]
pub(in crate::smb) struct PreparedFileRead{
    result:Result<ReadCompletion<Vec<u8>>,ReadFault<stdio::Error>>,
    proof:RustReadProof,
    member:RustSessionWitness,
}
impl PreparedFileRead {
    pub(in crate::smb) fn result(&self)->&Result<ReadCompletion<Vec<u8>>,ReadFault<stdio::Error>>{&self.result}
    pub(in crate::smb) fn require_current(&self,open:&FileOpen,tree:&RustRoot)->stdio::Result<()>{
        open.held.original.require_read(tree,&self.member,&self.proof)
    }
}
#[must_use]
pub(in crate::smb) struct PreparedFileStandard{view:RustFileStandard,scope:RustSessionWitness,output:u32,delete_pending:bool}
impl PreparedFileStandard{
    pub(in crate::smb) fn values(&self)->(u32,u64,u64,bool){
        let eof=self.view.eof();(self.output,allocation(eof),eof,self.delete_pending)
    }
    pub(in crate::smb) fn require_current(&self,open:&FileOpen,tree:&RustRoot)->stdio::Result<()>{
        open.held.original.require_standard(tree,&self.scope,&self.view)?;
        if open.held._slot.pending_delete()?!=self.delete_pending{return Err(stdio::ErrorKind::WouldBlock.into());}
        Ok(())
    }
}
#[must_use]
pub(in crate::smb) struct PreparedFileName{view:RustFileName,scope:RustSessionWitness,output:u32}
impl PreparedFileName{
    pub(in crate::smb) fn output(&self)->u32{self.output}
    pub(in crate::smb) fn path(&self)->&str{self.view.path()}
    pub(in crate::smb) fn require_current(&self,open:&FileOpen,tree:&RustRoot)->stdio::Result<()>{
        open.held.original.require_normalized_name(tree,&self.scope,&self.view)
    }
}
fn normalized_name_output(original:&Message<'_>,ordinal:usize,maximum:u32)->Result<u32,FileStandardError>{
    let query=original.query_info(maximum).map_err(FileStandardError::Fields)?;
    if query.class!=InfoClass::File(FileClass::NormalizedName){return Err(FileStandardError::NotSupported);}
    // A predecessor or another OPEN cannot lend its FileId or authority.
    if ordinal!=0||original.header.flags&RELATED!=0||
        !matches!(original.header.routing,Routing::Sync{..})||original.raw.get(20..24)!=Some(&[0;4][..])||
        !matches!(original.query_info_target(),Ok(OpenReference::Explicit(_))){
        return Err(FileStandardError::NotSupported);
    }
    Ok(query.output_bytes)
}
pub(super) fn normalized_query(member:&VerifiedMember<'_,'_>)->Result<(RustSessionWitness,u32),FileStandardError>{
    let output=normalized_name_output(member.message(),member.ordinal(),member.max_transact())?;
    let scope=member.rust_session_witness().map_err(FileStandardError::Session)?;
    // No additional FILE_READ_ATTRIBUTES check for this class (MS-SMB2
    // §3.3.5.20.1). These ordinary OPENs cannot be issued by FILE ID.
    Ok((scope,output))
}
pub(super) fn normalized_name_error(error:stdio::Error)->FileStandardError{
    match error.kind(){
        stdio::ErrorKind::Unsupported=>FileStandardError::NotSupported,
        _=>FileStandardError::Original(error),
    }
}
impl FileOpen {
    pub(in crate::smb) fn maximal_access_answer(&self)->Option<u32>{self.maximal}
    pub(in crate::smb) fn disk_id_answer(&self)->Option<(u64,u64)>{self.disk.map(SmbFileIndex::qfid)}
    pub(in crate::smb) fn smb_index(&self,tree:&RustRoot,scope:&RustSessionWitness)->stdio::Result<SmbFileIndex>{
        self.held.original.smb_index(tree,scope)
    }
    pub(in crate::smb) fn size(&self)->u64{self.held.original.eof()}
    pub(in crate::smb) fn logical_allocation(&self)->u64{allocation(self.size())}
    pub(in crate::smb) fn allows_read_data(&self)->bool{self.held.grant.0&READ_DATA!=0}
    pub(in crate::smb) fn require_current(&self,tree:&RustRoot,member:&RustSessionWitness)->stdio::Result<()>{
        let delete=self.held.grant.0&DELETE_ACCESS!=0;
        if member.is_create()&&(self.maximal.is_some()||delete){
            let writable=self.held.original.allows_writable_policy(tree,member)?;
            if delete&&!writable{return Err(stdio::ErrorKind::ReadOnlyFilesystem.into());}
            if self.maximal.is_some_and(|old|old!=file_ceiling(writable)){
                return Err(stdio::ErrorKind::PermissionDenied.into());
            }
        }else{
            self.held.original.require_current(tree,member)?;
        }
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
            |f,scope|tree.plan_read_file_delete(f,scope))
    }
    pub(super) fn require_disposition(&self,tree:&RustRoot,scope:&RustSessionWitness,snapshot:&Snapshot)->stdio::Result<()>{
        self.held.original.require_current(tree,scope)?;
        self.held.require_disposition(tree,scope,snapshot)
    }
    pub(in crate::smb) fn prepare_standard(&self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<PreparedFileStandard,FileStandardError>{
        let query=member.message().query_info(member.max_transact()).map_err(FileStandardError::Fields)?;
        if query.class!=InfoClass::File(FileClass::Standard){return Err(FileStandardError::NotSupported);}
        let scope=member.rust_session_witness().map_err(FileStandardError::Session)?;
        // Unlike FileBasic/All/NetworkOpen, Standard has no additional
        // READ_ATTRIBUTES requirement in MS-SMB2 §3.3.5.20.1.
        let view=self.held.original.standard(tree,&scope).map_err(FileStandardError::Original)?;
        let delete_pending=self.held._slot.pending_delete().map_err(FileStandardError::Original)?;
        Ok(PreparedFileStandard{view,scope,output:query.output_bytes,delete_pending})
    }
    pub(in crate::smb) fn prepare_attribute_tag(&self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<PreparedAttributeTag,FileStandardError>{
        PreparedAttributeTag::prepare(&self.held,member,|scope|self.held.original.require_current(tree,scope))
    }
    pub(in crate::smb) fn require_attribute_tag(&self,tree:&RustRoot,proof:&PreparedAttributeTag)->stdio::Result<()>{
        proof.require(&self.held,|scope|self.held.original.require_current(tree,scope))
    }
    pub(in crate::smb) fn prepare_file_access(&self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<PreparedFileAccess,FileStandardError>{
        PreparedFileAccess::prepare(&self.held,Object::File(tree.pool(),self.held.original.file()),member,
            |scope|self.require_current(tree,scope))
    }
    pub(in crate::smb) fn require_file_access(&self,tree:&RustRoot,proof:&PreparedFileAccess)->stdio::Result<()>{
        proof.require(&self.held,Object::File(tree.pool(),self.held.original.file()),|scope|self.require_current(tree,scope))
    }
    pub(in crate::smb) fn prepare_normalized_name(&self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<PreparedFileName,FileStandardError>{
        let (scope,output)=normalized_query(member)?;
        let view=self.held.original.normalized_name(tree,&scope).map_err(normalized_name_error)?;
        Ok(PreparedFileName{view,scope,output})
    }
    pub(in crate::smb) fn change_lock(&self,tree:&RustRoot,request:&LockRequest<'_,'_>)
        ->Result<PreparedLock,LockError>{
        self.held.change_lock(request,|scope|self.require_current(tree,scope))
    }
    pub(in crate::smb) fn require_lock(&self,tree:&RustRoot,proof:&PreparedLock)->stdio::Result<()>{
        let Held{_slot:slot,..}=&self.held;
        proof.require(slot,|scope|self.require_current(tree,scope))
    }
    pub(in crate::smb) fn read(&self,tree:&RustRoot,member:&VerifiedMember<'_,'_>,room:usize)
        ->Result<PreparedFileRead,FileReadError>{
        let raw=member.message().read_request(member.max_read()).map_err(FileReadError::Fields)?;
        if !self.held.original.belongs_to(tree){return Err(FileReadError::WrongTree);}
        let scope=member.rust_session_witness().map_err(FileReadError::Session)?;
        self.held.check_read(raw.range,room).map_err(FileReadError::Fault)?;
        let Held{_slot:slot,..}=&self.held;
        let _permit=slot.read_permit(raw.range,||self.require_current(tree,&scope))
            .map_err(FileReadError::Fault)?;
        let (data,proof)=self.held.original.read_window(tree,&scope,raw.range.offset(),raw.range.length())
            .map_err(|e|FileReadError::Fault(ReadFault::Original(e)))?;
        let result=match seal_read(raw.range,data){
            Ok(data)=>Ok(data),Err(ReadFault::EndOfFile)=>Err(ReadFault::EndOfFile),Err(e)=>return Err(FileReadError::Fault(e)),
        };
        Ok(PreparedFileRead{result,proof,member:scope})
    }

    pub(in crate::smb) fn close(self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<CloseCompletion,FileCloseFault>{
        let how=(||{
            let request=member.message().close_request().map_err(FileCloseError::Fields)?;
            if !self.held.original.belongs_to(tree){return Err(FileCloseError::WrongTree);}
            let scope=member.rust_session_witness().map_err(FileCloseError::Session)?;
            if !scope.is_close(){return Err(FileCloseError::WrongTree);}
            // The verified original can release its old FILE even when F
            // is offline. An elected delete has its own full current cut.
            tree.ensure_current(&scope).map_err(FileCloseError::Original)?;
            Ok((scope,request.mode))
        })();
        let (scope,how)=match how{Ok(value)=>value,Err(e)=>return Err(FileCloseFault::NotStarted(self,e))};
        // No complete logical timestamps are available; do not use host stat.
        self.held.close_file_with(how,tree,&scope,|_,_|Ok(None))
            .map_err(|e|FileCloseFault::Retired(FileCloseError::Original(e)))
    }
}

mod rename;
pub(crate) use rename::{FileRenameTarget,OrdinaryMoveAdmission,OrdinaryRenameAdmission,OrdinaryReplaceAdmission};
pub(in crate::smb) use rename::{MoveOpenWitness,WorkingMoveGuard,OrdinaryRenameError,PreparedCrossFilesystemMove,PreparedOrdinaryRename,RetiredOrdinaryRename};
pub(super) use rename::{RenameTurn,rename_target,ordinary_parent_leaf};

#[cfg(test)]
#[path="../../../tests/smb/backend_file_open.rs"]
mod tests;
