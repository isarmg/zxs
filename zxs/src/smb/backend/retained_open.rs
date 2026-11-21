//! Current read-only R selection inside the one ZXS tree. The exact same
//! FILE share object still applies to a retained generation. No second
//! share, writable R or server-constructed native session. DELETE on an
//! eligible retained FILE is granted only for an actual restore, not purge.
use super::{
    BindError,Facts,Grant,Held,Object,OpenCoordinator,OriginalRequest,ParentWrite,ReserveError,Share,
    directory_open::DirectoryOpenError as OpenError,
    file_open::{EXECUTE,FILE_READ_RIGHTS,FileCloseError,FileReadError,MAXIMUM_ALLOWED,READ_DATA,allocation},
    io::{CloseCompletion,ReadCompletion,ReadFault,seal_read},
    path::{NameError,TreeName},
};
use crate::{
    smb::{RustSessionWitness,connection::sessions::ordinary::VerifiedMember,
        wire::{create_fields::{CurrentName,Disposition,MaximumAccessQuery},set_info::RenameRequest}},
    system::reader::smb::{RustRetainedDirectory,RustRetainedFile,RustRetainedKind,RustRetainedReadProof,RustRetainedRestore,runtime::{RustRoot,RustRootStamp}},
};
use std::{convert::Infallible,io,sync::Arc};

const DIRECTORY:u32=1;
const NON_DIRECTORY:u32=0x40;
const DIR_OPTIONS:u32=0x0020_4807;
const FILE_OPTIONS:u32=0x0020_4846;

const RESTORE_DELETE:u32=0x0001_0000;

fn ceiling(restore:bool)->u32{FILE_READ_RIGHTS|if restore{RESTORE_DELETE}else{0}}
fn retained_grant(desired:u32,restore:bool)->Result<Grant,OpenError>{
    let allowed=ceiling(restore);
    if desired==0||desired&!(allowed|MAXIMUM_ALLOWED)!=0{return Err(OpenError::AccessDenied);}
    let exact=if desired&MAXIMUM_ALLOWED!=0{allowed}
        else{desired|if desired&EXECUTE!=0{READ_DATA}else{0}};
    Grant::from_effective(exact).ok_or(OpenError::AccessDenied)
}
fn directory(pool:crate::PoolId,round:Option<crate::OperationId>)->Object{
    round.map_or(Object::RecycleRoot(pool),|id|Object::DeleteRound(pool,id))
}
fn reserve_error(e:ReserveError)->OpenError{
    match e{
        ReserveError::SharingViolation=>OpenError::ShareViolation,
        ReserveError::DeletePending=>OpenError::DeletePending,
        ReserveError::Resources=>OpenError::Resources,
        ReserveError::Unavailable=>OpenError::Original(io::Error::other("unavailable original OPEN coordinator")),
    }
}
fn bind_error(e:BindError<OpenError>)->OpenError{
    match e{BindError::Changed=>OpenError::WrongTree,BindError::Original(e)=>e}
}
fn has_restore_destination(tree:&RustRoot,scope:&RustSessionWitness)->io::Result<bool>{
    // A share-level upper bound only. Every actual target is selected a
    // second time under its own current RW setting and physical proof.
    Ok(tree.project(scope)?.has_readwrite_child())
}
struct Opening<'a>{tree:&'a RustRoot,scope:&'a RustSessionWitness,facts:Facts}
struct DirectoryOpening<'a>(Opening<'a>);
impl OriginalRequest for DirectoryOpening<'_>{
    type Held=RustRetainedDirectory;
    type Error=OpenError;
    fn recheck(&self,dir:&RustRetainedDirectory)->Result<Facts,OpenError>{
        let (tree,scope,facts)=(self.0.tree,self.0.scope,self.0.facts);
        if !dir.belongs_to(tree)||directory(tree.pool(),dir.deletion())!=facts.object{return Err(OpenError::WrongTree);}
        dir.require_current(tree,scope).map_err(OpenError::Original)?;
        Ok(facts)
    }
}
fn recheck_file_ceiling(tree:&RustRoot,scope:&RustSessionWitness,grant:Grant,maximum:Option<u32>)->io::Result<()>{
    if maximum.is_some()||grant.0&RESTORE_DELETE!=0 {
        let current=ceiling(has_restore_destination(tree,scope)?);
        if grant.0&!current!=0||maximum.is_some_and(|saved|saved!=current){
            return Err(io::ErrorKind::PermissionDenied.into());
        }
    }
    Ok(())
}
struct FileOpening<'a>(Opening<'a>,Option<u32>);
impl OriginalRequest for FileOpening<'_>{
    type Held=RustRetainedFile;
    type Error=OpenError;
    fn recheck(&self,held:&RustRetainedFile)->Result<Facts,OpenError>{
        let (tree,scope,facts)=(self.0.tree,self.0.scope,self.0.facts);
        if !held.belongs_to(tree)||Object::File(tree.pool(),held.file())!=facts.object{return Err(OpenError::WrongTree);}
        held.require_current(tree,scope).map_err(OpenError::Original)?;
        recheck_file_ceiling(tree,scope,facts.grant,self.1).map_err(OpenError::Original)?;
        Ok(facts)
    }
}

#[must_use]
pub(in crate::smb) struct RetainedOpen(Kind,Option<u32>);
enum Kind{Directory(Held<RustRetainedDirectory>),File(Held<RustRetainedFile>)}
impl OpenCoordinator{
    /// None means the exact top-level component names another branch of
    /// this plan. Never fall back to ordinary lookup for an absent, offline,
    /// malformed or permission-denied retained object.
    pub(in crate::smb) fn prepare_retained_open(self:&Arc<Self>,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<Option<RetainedOpen>,OpenError>{
        let fields=member.message().create_fields().map_err(OpenError::Fields)?;
        let intent=fields.current_intent().map_err(OpenError::Special)?;
        let CurrentName::Relative(raw)=intent.name() else{return Ok(None);};
        let name=TreeName::create_relative(raw).map_err(OpenError::Name)?;
        let mut parts=name.components();
        let label=parts.next().ok_or(OpenError::Name(NameError::InvalidName))?;
        if !tree.names_retained_role(label){return Ok(None);}
        if fields.disposition()!=Disposition::Open{return Err(OpenError::UnsupportedOptions);}
        if intent.disk_id(){return Err(OpenError::ContextRequired);}
        let maximum=match intent.maximal_access(){
            None=>false,
            Some(MaximumAccessQuery::Unconditional)=>true,
            // No actual comparable ChangeTime for an R logical FILE.
            Some(MaximumAccessQuery::CompareChangeTime(_))=>return Err(OpenError::ContextRequired),
        };
        let scope=member.rust_session_witness().map_err(OpenError::Session)?;
        let share=Share::from_bits(u32::from(fields.share_access())).ok_or(OpenError::AccessDenied)?;
        let desired=fields.desired_access();
        if desired&!(FILE_READ_RIGHTS|MAXIMUM_ALLOWED|RESTORE_DELETE)!=0{return Err(OpenError::AccessDenied);}
        let mut rest=["";2];let mut n=0usize;
        for part in parts{
            if n==rest.len(){return Err(OpenError::NotADirectory);}
            rest[n]=part;n+=1;
        }
        let selected=tree.choose_retained(&scope,label,&rest[..n]).map_err(OpenError::Original)?;
        let options=fields.effective_options();
        let (object,answer)=match selected{
            RustRetainedKind::Directory(dir)=>{
                if maximum{return Err(OpenError::ContextRequired);}
                if options&NON_DIRECTORY!=0{return Err(OpenError::FileIsDirectory);}
                if options&!DIR_OPTIONS!=0{return Err(OpenError::UnsupportedOptions);}
                let grant=retained_grant(desired,false)?;
                let facts=Facts{object:directory(tree.pool(),dir.deletion()),grant,parent_write:ParentWrite::Denied};
                let opening=DirectoryOpening(Opening{tree,scope:&scope,facts});
                let pending=self.reserve(&opening,facts,share).map_err(reserve_error)?;
                let real=dir.acquire().map_err(OpenError::Original)?;
                (Kind::Directory(pending.bind(real).map_err(bind_error)?),None)
            },
            RustRetainedKind::File(file)=>{
                if options&DIRECTORY!=0{return Err(OpenError::NotADirectory);}
                if options&!FILE_OPTIONS!=0{return Err(OpenError::UnsupportedOptions);}
                let restore=(maximum||desired&(RESTORE_DELETE|MAXIMUM_ALLOWED)!=0)&&
                    has_restore_destination(tree,&scope).map_err(OpenError::Original)?;
                let answer=maximum.then(||ceiling(restore));
                let grant=retained_grant(desired,restore)?;
                let facts=Facts{object:Object::File(tree.pool(),file.file()),grant,parent_write:ParentWrite::Denied};
                let opening=FileOpening(Opening{tree,scope:&scope,facts},answer);
                let pending=self.reserve(&opening,facts,share).map_err(reserve_error)?.as_retained();
                let real=file.acquire(share.0&super::D!=0).map_err(OpenError::Original)?;
                (Kind::File(pending.bind(real).map_err(bind_error)?),answer)
            },
        };
        Ok(Some(RetainedOpen(object,answer)))
    }
}

#[must_use]
pub(in crate::smb) struct PreparedRetainedRead{
    result:Result<ReadCompletion<Vec<u8>>,ReadFault<io::Error>>,
    scope:RustSessionWitness,proof:RustRetainedReadProof,
}
impl PreparedRetainedRead{
    pub(in crate::smb) fn result(&self)->&Result<ReadCompletion<Vec<u8>>,ReadFault<io::Error>>{&self.result}
    pub(in crate::smb) fn require_current(&self,open:&RetainedOpen,tree:&RustRoot)->io::Result<()>{
        let Kind::File(file)=&open.0 else{return Err(io::ErrorKind::PermissionDenied.into());};
        file.original.require_read(tree,&self.scope,&self.proof)
    }
}

impl RetainedOpen{
    pub(in crate::smb) fn is_directory(&self)->bool{matches!(&self.0,Kind::Directory(_))}
    pub(in crate::smb) fn allows_read_data(&self)->bool{
        match &self.0{Kind::Directory(dir)=>dir.grant.0&READ_DATA!=0,Kind::File(file)=>file.grant.0&READ_DATA!=0}
    }
    pub(in crate::smb) fn size(&self)->u64{match &self.0{Kind::Directory(_)=>0,Kind::File(file)=>file.original.size()}}
    pub(in crate::smb) fn logical_allocation(&self)->u64{allocation(self.size())}
    pub(in crate::smb) fn maximal_access_answer(&self)->Option<u32>{self.1}
    pub(in crate::smb) fn require_current(&self,tree:&RustRoot,scope:&RustSessionWitness)->io::Result<()>{
        match &self.0{
            Kind::Directory(dir)=>dir.original.require_current(tree,scope),
            Kind::File(file)=>{
                file.original.require_current(tree,scope)?;
                if scope.is_create(){recheck_file_ceiling(tree,scope,file.grant,self.1)?;}
                Ok(())
            },
        }
    }
    pub(in crate::smb) fn read(&self,tree:&RustRoot,member:&VerifiedMember<'_,'_>,room:usize)
        ->Result<PreparedRetainedRead,FileReadError>{
        let request=member.message().read_request(member.max_read()).map_err(FileReadError::Fields)?;
        let Kind::File(file)=&self.0 else{return Err(FileReadError::Fault(ReadFault::NotDataFile));};
        if !file.original.belongs_to(tree){return Err(FileReadError::WrongTree);}
        let scope=member.rust_session_witness().map_err(FileReadError::Session)?;
        file.check_read(request.range,room).map_err(FileReadError::Fault)?;
        let (data,proof)=file.original.read(tree,&scope,request.range.offset(),request.range.length())
            .map_err(|e|FileReadError::Fault(ReadFault::Original(e)))?;
        let result=match seal_read(request.range,data){
            Ok(data)=>Ok(data),Err(ReadFault::EndOfFile)=>Err(ReadFault::EndOfFile),Err(e)=>return Err(FileReadError::Fault(e)),
        };
        Ok(PreparedRetainedRead{result,scope,proof})
    }
    pub(in crate::smb) fn close(self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<CloseCompletion,(Self,FileCloseError)>{
        let mode=(||{
            let cmd=member.message().close_request().map_err(FileCloseError::Fields)?;
            let original=match &self.0{
                Kind::Directory(dir)=>dir.original.belongs_to(tree),
                Kind::File(file)=>file.original.belongs_to(tree),
            };
            if !original{return Err(FileCloseError::WrongTree);}
            let scope=member.rust_session_witness().map_err(FileCloseError::Session)?;
            if !scope.is_close(){return Err(FileCloseError::WrongTree);}
            // Explicitly release this read-only hold even if its actual R
            // disappeared. This never restores, purges or looks up a name.
            tree.ensure_current(&scope).map_err(FileCloseError::Original)?;
            Ok(cmd.mode)
        })();
        let mode=match mode{Ok(mode)=>mode,Err(error)=>return Err((self,error))};
        let result=match self.0{
            Kind::Directory(dir)=>dir.close_with(mode,|_,_|Ok::<_,Infallible>(None)),
            Kind::File(file)=>file.close_with(mode,|_,_|Ok::<_,Infallible>(None)),
        };
        Ok(result.unwrap_or_else(|impossible|match impossible{}))
    }
}

#[derive(Debug)]
pub(in crate::smb) enum RestoreError{
    WrongTree,NotRetainedFile,AccessDenied,Name(NameError),
    Session(crate::smb::connection::Fault),Original(io::Error),
}

#[must_use]
pub(in crate::smb) struct PreparedRetainedRestore{scope:RustSessionWitness,receipt:RustRetainedRestore}
impl PreparedRetainedRestore{
    pub(in crate::smb) fn require_current(&self,open:&RetainedOpen,tree:&RustRoot)->io::Result<()>{
        let Kind::File(file)=&open.0 else{return Err(io::ErrorKind::PermissionDenied.into());};
        if file.grant.0&RESTORE_DELETE==0{return Err(io::ErrorKind::PermissionDenied.into());}
        self.receipt.require_current(tree,&file.original,&self.scope)
    }
    pub(in crate::smb) fn finish_related_close(self,open:RetainedOpen,tree:&RustRoot,close:&VerifiedMember<'_,'_>)
        ->io::Result<(CloseCompletion,RetiredRetainedRestore)>{
        use crate::smb::wire::io_fields::OpenReference;
        if !close.message().close_request().is_ok_and(|p|p.open==OpenReference::PreviousInCompound){
            return Err(io::ErrorKind::InvalidInput.into());
        }
        self.require_current(&open,tree)?;
        let closing=close.rust_session_witness().map_err(|_|io::ErrorKind::PermissionDenied)?;
        if !closing.is_close()||!closing.same_session(&self.scope){return Err(io::ErrorKind::PermissionDenied.into());}
        let completed=open.close(tree,close).map_err(|(_,err)|match err{
            FileCloseError::Original(e)=>e,_=>io::ErrorKind::PermissionDenied.into(),
        })?;
        // The true original File has now retired. Only the historical
        // storage fact and requalification for its two brief replies remain.
        Ok((completed,RetiredRetainedRestore{receipt:self.receipt,tree:tree.stamp(),scope:self.scope,closing}))
    }
}

#[must_use]
pub(in crate::smb) struct RetiredRetainedRestore{
    receipt:RustRetainedRestore,tree:RustRootStamp,scope:RustSessionWitness,closing:RustSessionWitness,
}
impl RetiredRetainedRestore{
    pub(in crate::smb) fn require_current(&self,tree:&RustRoot)->io::Result<()>{
        if !tree.matches_stamp(&self.tree){return Err(io::ErrorKind::PermissionDenied.into());}
        tree.ensure_current(&self.closing)?;
        self.receipt.require_receipt(tree,&self.scope)
    }
}

impl RetainedOpen{
    /// This is not a general rename: the already held file must be in R,
    /// and the full target must resolve to a current authorized RW F in
    /// this original tree. Any collision fails even if the client asked
    /// for replacement (Linux sends that preference routinely).
    pub(in crate::smb) fn restore(&self,tree:&RustRoot,request:&RenameRequest<'_,'_>)
        ->Result<PreparedRetainedRestore,RestoreError>{
        let Kind::File(file)=&self.0 else{return Err(RestoreError::NotRetainedFile);};
        if !file.original.belongs_to(tree){return Err(RestoreError::WrongTree);}
        if file.grant.0&RESTORE_DELETE==0{return Err(RestoreError::AccessDenied);}
        let scope=request.original().rust_session_witness().map_err(RestoreError::Session)?;
        let mut target=request.destination().components();
        let label=target.next().ok_or(RestoreError::Name(NameError::InvalidName))?;
        let leaf=target.next_back().ok_or(RestoreError::Name(NameError::InvalidName))?;
        let mut parents=Vec::new();
        for name in target{
            if parents.len()>=crate::fs::MAX_PATH_DEPTH-1{return Err(RestoreError::Name(NameError::TooLong));}
            parents.try_reserve(1).map_err(|_|RestoreError::Name(NameError::Resources))?;
            parents.push(name);
        }
        let receipt=tree.restore_retained(&file.original,&scope,label,&parents,leaf).map_err(RestoreError::Original)?;
        Ok(PreparedRetainedRestore{scope,receipt})
    }
}

use super::{query::{Pattern,root::{Child,Fault as PageFault}},root_open::RootQueryError};
use crate::{smb::{connection::sessions::ordinary::WrittenRetainedQuery,wire::query_reply::DirectoryReply},
    system::reader::smb::RustRetainedPage};

#[must_use]
pub(in crate::smb) struct PreparedRetainedQuery{
    reply:DirectoryReply,source:RustRetainedPage,scope:RustSessionWitness,
}
impl PreparedRetainedQuery{
    pub(in crate::smb) fn reply(&self)->&DirectoryReply{&self.reply}
    pub(in crate::smb) fn require_current(&self,open:&RetainedOpen,tree:&RustRoot)->io::Result<()>{
        let Kind::Directory(dir)=&open.0 else{return Err(io::ErrorKind::PermissionDenied.into());};
        if dir.grant.0&READ_DATA==0||!self.reply.pending().belongs_to(dir){return Err(io::ErrorKind::PermissionDenied.into());}
        dir.original.require_page(tree,&self.scope,&self.source)
    }
}
impl RetainedOpen{
    pub(in crate::smb) fn prepare_query(&mut self,tree:&RustRoot,member:&VerifiedMember<'_,'_>,room:usize)
        ->Result<PreparedRetainedQuery,RootQueryError>{
        let m=member.message();let limit=member.max_transact();
        m.directory_output(limit).map_err(RootQueryError::Fields)?;
        let scope=member.rust_session_witness().map_err(RootQueryError::Session)?;
        let Kind::Directory(dir)=&mut self.0 else{return Err(RootQueryError::WrongTree);};
        if !dir.original.belongs_to(tree){return Err(RootQueryError::WrongTree);}
        if dir.grant.0&READ_DATA==0{return Err(RootQueryError::AccessDenied);}
        let query=m.query_directory(limit).map_err(RootQueryError::Fields)?;
        let (page,source)=dir.prepare_ordinary_query_with(&query,room,|original,pattern,after,count|{
            if !pattern.supports_ordinary(original.deletion().is_some()){return Err(PageFault::UnsupportedPattern);}
            let matches=|name:&str,work:&mut usize|pattern.match_bounded(name,work);
            let filter=if matches!(pattern,Pattern::All){None}else{
                Some(&matches as &dyn Fn(&str,&mut usize)->io::Result<bool>)
            };
            let source=original.page(tree,&scope,after,count,filter).map_err(PageFault::Original)?;
            let mut rows=Vec::new();
            for (name,file) in source.rows(){
                rows.try_reserve(1).map_err(|_|PageFault::Resources)?;
                let mut spelling=String::new();
                spelling.try_reserve_exact(name.len()).map_err(|_|PageFault::Resources)?;
                spelling.push_str(name);
                // The full DeleteOp remains the directory name. No
                // invented or truncated FILE number for a delete round.
                let (number,eof)=file.map_or((u64::MAX,None),|(number,eof)|(number,Some(eof)));
                rows.push(Child{name:spelling,number,eof});
            }
            Ok((rows,source))
        })?;
        let reply=page.into_reply()?;
        Ok(PreparedRetainedQuery{reply,source,scope})
    }
    pub(in crate::smb) fn apply_written_query(&mut self,tree:&RustRoot,receipt:WrittenRetainedQuery)->io::Result<()>{
        let query=receipt.into_prepared();
        query.require_current(self,tree)?;
        let Kind::Directory(dir)=&mut self.0 else{return Err(io::ErrorKind::PermissionDenied.into());};
        if !query.reply.into_effect().apply(dir){return Err(io::ErrorKind::PermissionDenied.into());}
        Ok(())
    }
}

#[cfg(test)]
#[path="../../test/smb/backend_retained_open.rs"]
mod tests;
