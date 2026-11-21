//! The first real OPEN: an already present read-only logical ZXS root.
//! No disk probe, host path, client ID, file writer or response here.
use super::{BindError,Facts,Grant,Held,Object,OpenCoordinator,OriginalRequest,ParentWrite,ReserveError,Share,
    attribute_tag::PreparedAttributeTag,file_open::FileStandardError};
use super::io::{CheckedClose,CloseAttributes,CloseCompletion};
use crate::{smb::{RustSessionWitness,connection::{Fault as ConnectionFault,sessions::ordinary::VerifiedMember},
    wire::{create_fields::{CurrentName,Error as CreateError,MaximumAccessQuery,SpecialOpen},io_fields::{CloseMode,IoFieldError},
          query_fields::{FileClass,InfoClass}}},
    system::reader::smb::runtime::{RustRoot,RustRootStamp}};
use std::{convert::Infallible,io,rc::{Rc,Weak},sync::Arc};

mod capacity;
pub(in crate::smb) use capacity::{FS_SIZE_BYTES,PreparedRootFsSize,RetiredRootFsSize};

const MAXIMUM_ALLOWED:u32=0x0200_0000;
const LIST_DIRECTORY:u32=0x0000_0001;
const TRAVERSE:u32=0x0000_0020;
const READ_ATTRIBUTES:u32=0x0000_0080;
const SYNCHRONIZE:u32=0x0010_0000;
const ROOT_RIGHTS:u32=LIST_DIRECTORY|TRAVERSE|READ_ATTRIBUTES|SYNCHRONIZE;
const ROOT_OPTIONS:u32=0x0020_4807; // directory, harmless write/sequential/random hints, backup without elevation, no reparse

#[derive(Debug)]
pub(in crate::smb) enum RootOpenError{
    Fields(CreateError),Special(SpecialOpen),NonRoot,ContextRequired,UnsupportedOptions,
    AccessDenied,ShareViolation,Resources,Session(ConnectionFault),Original(io::Error),WrongTree,
}
#[derive(Debug)]
pub(in crate::smb) enum RootCloseError{
    Fields(IoFieldError),Session(ConnectionFault),Original(io::Error),WrongTree,
}

fn grant(desired:u32)->Result<Grant,RootOpenError>{
    // MS-SMB2 §3.3.5.9: deny zero access. No generic/EA/ACL/owner
    // or write access is silently narrowed and represented as granted.
    if desired==0||desired&!(ROOT_RIGHTS|MAXIMUM_ALLOWED)!=0{return Err(RootOpenError::AccessDenied);}
    let actual=if desired&MAXIMUM_ALLOWED!=0{ROOT_RIGHTS}else{
        // The SMB FILE_EXECUTE bit is FILE_TRAVERSE here, and §3.3.5.9
        // also requires FILE_READ_DATA on an Open that includes that bit.
        desired|if desired&TRAVERSE!=0{LIST_DIRECTORY}else{0}
    };
    Grant::from_effective(actual).ok_or(RootOpenError::AccessDenied)
}

fn root_maximum(query:Option<MaximumAccessQuery>)->Result<Option<Grant>,RootOpenError>{
    match query{
        None=>Ok(None),
        Some(MaximumAccessQuery::Unconditional)=>grant(MAXIMUM_ALLOWED).map(Some),
        // This virtual object has no authoritative ChangeTime. A supplied
        // timestamp is not evidence of equality, or of an unchanged ACL.
        Some(MaximumAccessQuery::CompareChangeTime(_))=>Err(RootOpenError::ContextRequired),
    }
}

struct Opened{same_tree:RustRootStamp,identity:Rc<()>}
impl CheckedClose for Opened {
    type Error=Infallible;
    fn close_checked(&mut self,_:CloseMode)->Result<Option<CloseAttributes>,Infallible>{
        // There is no physical inode to collect authoritative times from.
        // Consuming the real held open retires it; no postquery false zeros.
        Ok(None)
    }
}
struct Request<'a>{tree:&'a RustRoot, scope:RustSessionWitness,facts:Facts}
impl OriginalRequest for Request<'_>{
    type Held=Opened;
    type Error=RootOpenError;
    fn recheck(&self,held:&Opened)->Result<Facts,RootOpenError>{
        if !self.tree.matches_stamp(&held.same_tree){return Err(RootOpenError::WrongTree);}
        self.tree.ensure_current(&self.scope).map_err(RootOpenError::Original)?;
        Ok(self.facts)
    }
}

#[must_use]
pub(in crate::smb) struct RootOpen{held:Held<Opened>,maximum:Option<Grant>,disk_id:Option<(u64,u64)>}
impl OpenCoordinator{
    /// The dispatcher resolves the real TID in this current member's session
    /// before calling. No file-id assignment, reply or other route happens here.
    pub(in crate::smb) fn prepare_root_open(self:&Arc<Self>,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<RootOpen,RootOpenError>{
        let fields=member.message().create_fields().map_err(RootOpenError::Fields)?;
        let intent=fields.current_intent().map_err(RootOpenError::Special)?;
        if intent.name()!=CurrentName::Root{return Err(RootOpenError::NonRoot);}
        let maximum=root_maximum(intent.maximal_access())?;
        if fields.effective_options()&!ROOT_OPTIONS!=0{return Err(RootOpenError::UnsupportedOptions);}
        let access=grant(fields.desired_access())?;
        let share=Share::from_bits(u32::from(fields.share_access())).ok_or(RootOpenError::AccessDenied)?;
        let scope=member.rust_session_witness().map_err(RootOpenError::Session)?;
        let disk_id=if intent.disk_id(){
            Some(tree.logical_root_disk_id(&scope).map_err(RootOpenError::Original)?)
        }else{
            tree.ensure_current(&scope).map_err(RootOpenError::Original)?;
            None
        };
        // The virtual root is specifically non-exclusive for readers. As
        // with RestrictExclusiveOpens, the effective vote includes sharing
        // read even if a read-only caller omitted it. Root never grants W/D.
        let facts=Facts{object:Object::Namespace(tree.pool()),grant:access,parent_write:ParentWrite::Denied};
        let request=Request{tree,scope,facts};
        let pending=self.reserve(&request,facts,share).map_err(|why|match why{
            ReserveError::SharingViolation=>RootOpenError::ShareViolation,
            ReserveError::DeletePending=>RootOpenError::AccessDenied,
            ReserveError::Resources=>RootOpenError::Resources,
            ReserveError::Unavailable=>RootOpenError::Original(io::Error::other("unavailable original OPEN coordinator")),
        })?;
        let held=pending.bind(Opened{same_tree:tree.stamp(),identity:Rc::new(())}).map_err(|why|match why{
            BindError::Original(e)=>e,BindError::Changed=>RootOpenError::WrongTree,
        })?;
        Ok(RootOpen{held,maximum,disk_id})
    }
}
impl RootOpen{
    pub(in crate::smb) fn allows_list_directory(&self)->bool{self.held.grant.0&LIST_DIRECTORY!=0}
    pub(in crate::smb) fn allows_read_attributes(&self)->bool{self.held.grant.0&READ_ATTRIBUTES!=0}
    /// Response fact for the authenticated user's original root CREATE.
    /// Not this OPEN's granted mask, nor the share-wide rights ceiling.
    pub(in crate::smb) fn maximal_access_answer(&self)->Option<u32>{self.maximum.map(|g|g.0)}
    pub(in crate::smb) fn disk_id_answer(&self)->Option<(u64,u64)>{self.disk_id}
    /// For the owning reply phase only: its scope is from the actual paid
    /// CREATE or other matching request, not reconstructed via a wire ID.
    pub(in crate::smb) fn require_current(&self,tree:&RustRoot,scope:&RustSessionWitness)->io::Result<()>{
        if !tree.matches_stamp(&self.held.original.same_tree){
            return Err(io::Error::new(io::ErrorKind::NotFound,"not the original logical root OPEN"));
        }
        if scope.is_create()&&let Some(expected)=self.disk_id{
            if tree.logical_root_disk_id(scope)?!=expected{return Err(io::ErrorKind::PermissionDenied.into());}
            Ok(())
        }else{
            tree.ensure_current(scope)
        }
    }
    pub(in crate::smb) fn prepare_attribute_tag(&self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<PreparedAttributeTag,FileStandardError>{
        PreparedAttributeTag::prepare(&self.held,member,|scope|self.require_current(tree,scope))
    }
    pub(in crate::smb) fn require_attribute_tag(&self,tree:&RustRoot,proof:&PreparedAttributeTag)->io::Result<()>{
        proof.require(&self.held,|scope|self.require_current(tree,scope))
    }
    pub(in crate::smb) fn close(self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<CloseCompletion,(Self,RootCloseError)>{
        fn checked(open:&RootOpen,tree:&RustRoot,member:&VerifiedMember<'_,'_>)->Result<CloseMode,RootCloseError>{
            let cmd=member.message().close_request().map_err(RootCloseError::Fields)?;
            if !tree.matches_stamp(&open.held.original.same_tree){return Err(RootCloseError::WrongTree);}
            let scope=member.rust_session_witness().map_err(RootCloseError::Session)?;
            tree.ensure_current(&scope).map_err(RootCloseError::Original)?;
            Ok(cmd.mode)
        }
        let how=match checked(&self,tree,member){Ok(v)=>v,Err(e)=>return Err((self,e))};
        Ok(self.held.close(how).unwrap_or_else(|impossible|match impossible{}))
    }
}

/// Querying the entire logical volume through an actual root OPEN. This is
/// not a stat of the root or any one mounted host disk.
#[derive(Debug)]
pub(in crate::smb) enum RootFsError{
    Fields(crate::smb::wire::query_fields::QueryError),NotSupported,WrongTree,Session(ConnectionFault),Original(io::Error),
}
#[must_use]
pub(in crate::smb) struct PreparedRootInternal{
    ids:(u64,u64),scope:RustSessionWitness,origin:Weak<()>,output:u32,
}
impl PreparedRootInternal{
    pub(in crate::smb) fn values(&self)->(u32,u64){(self.output,self.ids.0)}
    pub(in crate::smb) fn require_current(&self,open:&RootOpen,tree:&RustRoot)->io::Result<()>{
        if !self.origin.ptr_eq(&Rc::downgrade(&open.held.original.identity))||
           !tree.matches_stamp(&open.held.original.same_tree)||
           tree.logical_root_disk_id(&self.scope)?!=self.ids{
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(())
    }
}
impl RootOpen{
    pub(in crate::smb) fn prepare_internal(&self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<PreparedRootInternal,RootFsError>{
        let info=member.message().query_info(member.max_transact()).map_err(RootFsError::Fields)?;
        if info.class!=InfoClass::File(FileClass::Internal){return Err(RootFsError::NotSupported);}
        if !tree.matches_stamp(&self.held.original.same_tree){return Err(RootFsError::WrongTree);}
        let scope=member.rust_session_witness().map_err(RootFsError::Session)?;
        let ids=tree.logical_root_disk_id(&scope).map_err(RootFsError::Original)?;
        Ok(PreparedRootInternal{ids,scope,origin:Rc::downgrade(&self.held.original.identity),output:info.output_bytes})
    }
}
#[must_use]
pub(in crate::smb) struct PreparedRootFsAttributes{
    source:RustRootProjection, scope:RustSessionWitness, origin:Weak<()>, output:u32,
}
impl PreparedRootFsAttributes{
    pub(in crate::smb) fn output(&self)->u32{self.output}
    pub(in crate::smb) fn policy(&self)->(bool,u32){
        // The virtual volume uses Core's common display-name limit.
        // Physical-media and per-object budgets still apply at their own
        // operations. Neither this flag nor the limit grants new rights.
        (!self.source.has_readwrite_child(),crate::display::MAX as u32)
    }
    pub(in crate::smb) fn require_current(&self,open:&RootOpen,tree:&RustRoot)->io::Result<()>{
        if !self.origin.ptr_eq(&Rc::downgrade(&open.held.original.identity))||
           !tree.matches_stamp(&open.held.original.same_tree){
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        tree.require_projection(&self.scope,&self.source)
    }
    pub(in crate::smb) fn finish_related_close(self,open:RootOpen,tree:&RustRoot,close:&VerifiedMember<'_,'_>)
        ->io::Result<(CloseCompletion,RetiredRootFs)>{
        self.require_current(&open,tree)?;
        let last=close.rust_session_witness().map_err(|_|io::ErrorKind::PermissionDenied)?;
        if !last.is_close(){return Err(io::ErrorKind::PermissionDenied.into());}
        let done=open.close(tree,close).map_err(|_|io::ErrorKind::PermissionDenied)?;
        // That actual original OPEN no longer exists. The before/after
        // send proof below retains the precise tree snapshot, not a route,
        // share vote, open marker or the ability to run another command.
        Ok((done,RetiredRootFs{source:self.source,query:self.scope,close:last}))
    }
}

#[must_use]
pub(in crate::smb) struct RetiredRootFs{
    source:RustRootProjection,query:RustSessionWitness,close:RustSessionWitness,
}
impl RetiredRootFs {
    pub(in crate::smb) fn require_current(&self,tree:&RustRoot)->io::Result<()>{
        tree.ensure_current(&self.close)?;
        tree.require_projection(&self.query,&self.source)
    }
}
impl RootOpen{
    pub(in crate::smb) fn prepare_fs_attributes(&self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<PreparedRootFsAttributes,RootFsError>{
        use crate::smb::wire::query_fields::{FsClass,InfoClass};
        let scope=member.rust_session_witness().map_err(RootFsError::Session)?;
        if !scope.is_query_info()||!tree.matches_stamp(&self.held.original.same_tree){return Err(RootFsError::WrongTree);}
        let query=member.message().query_info(member.max_transact()).map_err(RootFsError::Fields)?;
        if query.class!=InfoClass::Filesystem(FsClass::Attribute){return Err(RootFsError::NotSupported);}
        // FileFsAttributeInformation has no per-file ACL query or stored
        // data result. A live original OPEN suffices for this class.
        let source=tree.project(&scope).map_err(RootFsError::Original)?;
        Ok(PreparedRootFsAttributes{source,scope,origin:Rc::downgrade(&self.held.original.identity),output:query.output_bytes})
    }
}

#[cfg(test)]
#[path="../../test/smb/backend_root_open.rs"]
mod tests;


use super::query::{PageStop,PatternError,root::{Child,Fault as PageFault,ReplyFault}};
use crate::smb::{connection::sessions::ordinary::WrittenRootQuery,wire::{query_fields::QueryError,query_reply::DirectoryReply}};
use crate::system::reader::smb::runtime::RustRootProjection;

#[derive(Debug)]
pub(in crate::smb) enum RootQueryError{
    Fields(QueryError),Session(ConnectionFault),WrongTree,AccessDenied,Pending,
    InvalidPattern,BufferTooSmall,FirstRecordOverflow,Resources,Original(io::Error),
}
impl From<PageFault> for RootQueryError{
    fn from(v:PageFault)->Self{
        match v{
            PageFault::NotNamespace=>Self::WrongTree,
            PageFault::ListDenied=>Self::AccessDenied,
            PageFault::PendingReply=>Self::Pending,
            PageFault::Pattern(PatternError::Resources)=>Self::Resources,
            PageFault::Pattern(_)=>Self::InvalidPattern,
            PageFault::UnsupportedPattern=>Self::Fields(QueryError::NotSupported),
            PageFault::Budget(PageStop::ClientLengthMismatch|PageStop::ResponseSpace)=>Self::BufferTooSmall,
            PageFault::Budget(PageStop::FirstRecordOverflow{..})=>Self::FirstRecordOverflow,
            PageFault::Resources=>Self::Resources,
            PageFault::Original(e)=>Self::Original(e),
            _=>Self::Original(io::Error::new(io::ErrorKind::InvalidData,"bad original directory page")),
        }
    }
}

impl From<ReplyFault> for RootQueryError{
    fn from(fault:ReplyFault)->Self{
        match fault{
            ReplyFault::Resources=>Self::Resources,
            ReplyFault::FirstRecordOverflow=>Self::FirstRecordOverflow,
            ReplyFault::Original=>Self::Original(io::Error::new(io::ErrorKind::InvalidData,"wrong original directory reply")),
        }
    }
}

#[must_use]
pub(in crate::smb) struct PreparedRootQuery{
    reply:DirectoryReply,
    source:RustRootProjection,
    scope:RustSessionWitness,
}
impl PreparedRootQuery{
    pub(in crate::smb) fn reply(&self)->&DirectoryReply{&self.reply}
    pub(in crate::smb) fn require_current(&self,open:&RootOpen,tree:&RustRoot)->io::Result<()>{
        // Check original tree and OPEN *before* hitting the DB. The actual
        // command grant cannot be reacquired from an equivalent numeric ID.
        if !tree.matches_stamp(&open.held.original.same_tree)||
           !self.reply.pending().belongs_to(&open.held)||
           open.held.grant.0&LIST_DIRECTORY==0{
            return Err(io::Error::new(io::ErrorKind::PermissionDenied,"not this original directory proposal"));
        }
        tree.require_projection(&self.scope,&self.source)
    }
}
impl RootOpen{
    pub(in crate::smb) fn prepare_query(
        &mut self,tree:&RustRoot,member:&VerifiedMember<'_,'_>,room:usize,
    )->Result<PreparedRootQuery,RootQueryError>{
        let m=member.message();let limit=member.max_transact();
        m.directory_output(limit).map_err(RootQueryError::Fields)?;
        let scope=member.rust_session_witness().map_err(RootQueryError::Session)?;
        if !tree.matches_stamp(&self.held.original.same_tree){return Err(RootQueryError::WrongTree);}
        if self.held.grant.0&LIST_DIRECTORY==0{return Err(RootQueryError::AccessDenied);}
        let q=m.query_directory(limit).map_err(RootQueryError::Fields)?;
        let (page,source)=self.held.prepare_root_query_with(&q,room,|_held|{
            let source=tree.project(&scope)?;
            let mut rows=Vec::new();
            for (name,number) in source.children(){
                rows.try_reserve(1).map_err(|_|io::ErrorKind::OutOfMemory)?;
                let mut owned=String::new();owned.try_reserve_exact(name.len()).map_err(|_|io::ErrorKind::OutOfMemory)?;
                owned.push_str(name);rows.push(Child{name:owned,number,eof:None});
            }
            Ok((rows,source))
        })?;
        let reply=page.into_reply()?;
        let prepared=PreparedRootQuery{reply,source,scope};
        Ok(prepared)
    }

    pub(in crate::smb) fn apply_written_query(
        &mut self,tree:&RustRoot,wire:WrittenRootQuery,
    )->io::Result<()>{
        let original=wire.into_prepared();
        original.require_current(self,tree)?;
        if !original.reply.into_effect().apply(&mut self.held){
            return Err(io::Error::new(io::ErrorKind::PermissionDenied,"stale original search proposal"));
        }
        Ok(())
    }
}
