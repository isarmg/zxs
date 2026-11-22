//! An SMB OPEN on a real authorized ordinary volume root or nested directory.
//! Default rights are read-only; a qualified nested OPEN may request explicit
//! DELETE only for the bounded relabel lane, not data writes or disposition.
//! Core chooses the kind; no FILE error is a directory test.
use super::{BindError,Facts,Grant,Held,Object,OpenCoordinator,OriginalRequest,ParentWrite,ReserveError,Share,
    attribute_tag::PreparedAttributeTag,disposition::DELETE_ACCESS,file_open::{FileStandardError,normalized_name_error,normalized_query},io::{CheckedClose,CloseAttributes,CloseCompletion},path::{NameError,TreeName}};
use crate::{DirectoryId,PoolId,VolumeId,fs::MAX_PATH_DEPTH,
    smb::{RustSessionWitness,connection::{Fault as ConnectionFault,sessions::ordinary::VerifiedMember},
        wire::{create_fields::{CurrentName,Disposition,Error as CreateError,SpecialOpen},io_fields::{CloseMode,IoFieldError,OpenReference},
            message::{RELATED,Routing},query_fields::{FileClass,InfoClass}}},
    system::reader::smb::{directory::{RustDirectoryName,RustOrdinaryDirectory,RustOrdinaryKind},runtime::RustRoot}};
use std::{convert::Infallible,io,sync::{Arc,Weak}};

const LIST:u32=1;
const TRAVERSE:u32=0x20;
const READ_ATTRIBUTES:u32=0x80;
const SYNCHRONIZE:u32=0x0010_0000;
const MAXIMUM_ALLOWED:u32=0x0200_0000;
const RIGHTS:u32=LIST|TRAVERSE|READ_ATTRIBUTES|SYNCHRONIZE;
const DIRECTORY:u32=1;
const NON_DIRECTORY:u32=0x40;
pub(super) const DIR_OPTIONS:u32=0x0020_4807; // No effectful options, special access, caching or mutation.

#[derive(Debug)]
pub(in crate::smb) enum DirectoryOpenError{
    Fields(CreateError),Special(SpecialOpen),Name(NameError),ContextRequired,UnsupportedOptions,
    FileIsDirectory,NotADirectory,AccessDenied,ShareViolation,DeletePending,Resources,Session(ConnectionFault),Original(io::Error),WrongTree,
}
#[derive(Debug)]
pub(in crate::smb) enum DirectoryCloseError{
    Fields(IoFieldError),Session(ConnectionFault),Original(io::Error),WrongTree,
}

pub(super) fn readonly_grant(desired:u32)->Result<Grant,DirectoryOpenError>{
    existing_grant(desired,false)
}
fn existing_grant(desired:u32,allow_directory_relabel:bool)->Result<Grant,DirectoryOpenError>{
    let extra=if allow_directory_relabel{DELETE_ACCESS}else{0};
    if desired==0||desired&!(RIGHTS|MAXIMUM_ALLOWED|extra)!=0{return Err(DirectoryOpenError::AccessDenied);}
    // Do not grow existing MAXIMUM_ALLOWED opens; this slice requires an
    // explicitly requested DELETE and independent actual writability.
    let access=if desired&MAXIMUM_ALLOWED!=0{RIGHTS|(desired&extra)}
        else{desired|if desired&TRAVERSE!=0{LIST}else{0}};
    Grant::from_effective(access).ok_or(DirectoryOpenError::AccessDenied)
}
fn object(pool:PoolId,volume:VolumeId,nested:Option<DirectoryId>)->Object{
    nested.map_or(Object::OrdinaryRoot(pool,volume),|id|Object::Directory(pool,id))
}
fn parent_write(allows:bool)->ParentWrite{if allows{ParentWrite::Granted}else{ParentWrite::Denied}}
fn require_grant(dir:&RustOrdinaryDirectory,tree:&RustRoot,member:&RustSessionWitness,grant:Grant)->io::Result<()>{
    if member.is_create()&&grant.0&DELETE_ACCESS!=0{dir.require_relabel_grant(tree,member)}
    else{dir.require_current(tree,member)}
}

struct Opening<'a>{tree:&'a RustRoot,member:&'a RustSessionWitness,facts:Facts}
impl OriginalRequest for Opening<'_>{
    type Held=RustOrdinaryDirectory;
    type Error=DirectoryOpenError;
    fn recheck(&self,dir:&RustOrdinaryDirectory)->Result<Facts,DirectoryOpenError>{
        if !dir.belongs_to(self.tree){return Err(DirectoryOpenError::WrongTree);}
        require_grant(dir,self.tree,self.member,self.facts.grant).map_err(DirectoryOpenError::Original)?;
        Ok(Facts{
            object:object(self.tree.pool(),dir.volume(),dir.nested()),grant:self.facts.grant,
            parent_write:parent_write(dir.parent_allows_file_write()),
        })
    }
}

#[must_use]
pub(in crate::smb) struct DirectoryOpen{held:Held<RustOrdinaryDirectory>}

/// A confirmed ordinary F directory stream has a single selected logical
/// name, no byte stream and no directory-deletion lane. Physical directory
/// size, subdirectory count and inode link counts are not those facts.
#[must_use]
pub(in crate::smb) struct PreparedDirectoryStandard{open:Weak<()>,scope:RustSessionWitness,output:u32}
impl PreparedDirectoryStandard{
    pub(in crate::smb) fn output(&self)->u32{self.output}
    pub(in crate::smb) fn require_current(&self,open:&DirectoryOpen,tree:&RustRoot)->io::Result<()>{
        let slot=&open.held._slot;
        let original=&open.held.original;
        if !self.scope.is_query_info()||!self.open.ptr_eq(&Arc::downgrade(&slot.identity))||
            !slot.bound||slot.retained||slot.released||slot.object!=object(tree.pool(),original.volume(),original.nested()){
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        open.require_current(tree,&self.scope)
    }
}

/// Class 48 needs only the already authorized, by-name original OPEN,
/// not FILE_LIST_DIRECTORY or an additional attributes grant.
#[must_use]
pub(in crate::smb) struct PreparedDirectoryName{view:RustDirectoryName,scope:RustSessionWitness,output:u32}
impl PreparedDirectoryName{
    pub(in crate::smb) fn output(&self)->u32{self.output}
    pub(in crate::smb) fn path(&self)->&str{self.view.path()}
    pub(in crate::smb) fn require_current(&self,open:&DirectoryOpen,tree:&RustRoot)->io::Result<()>{
        open.held.original.require_normalized_name(tree,&self.scope,&self.view)
    }
}

impl OpenCoordinator{
    /// None is a real FILE kind, not an arbitrary retry or a failed directory
    /// check. That branch still needs to select and hold its own actual FILE.
    pub(in crate::smb) fn prepare_directory_open(self:&Arc<Self>,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<Option<DirectoryOpen>,DirectoryOpenError>{
        let fields=member.message().create_fields().map_err(DirectoryOpenError::Fields)?;
        let intent=fields.current_intent().map_err(DirectoryOpenError::Special)?;
        let CurrentName::Relative(raw)=intent.name() else{return Err(DirectoryOpenError::UnsupportedOptions);};
        if fields.disposition()!=Disposition::Open{return Err(DirectoryOpenError::UnsupportedOptions);}
        let name=TreeName::create_relative(raw).map_err(DirectoryOpenError::Name)?;
        let mut components=name.components();
        let label=components.next().ok_or(DirectoryOpenError::Name(NameError::InvalidName))?;
        let mut names=Vec::new();
        for name in components{
            if names.len()>=MAX_PATH_DEPTH{return Err(DirectoryOpenError::Name(NameError::TooLong));}
            names.try_reserve(1).map_err(|_|DirectoryOpenError::Resources)?;
            names.push(name);
        }
        let scope=member.rust_session_witness().map_err(DirectoryOpenError::Session)?;
        let selection=tree.choose_ordinary_kind(&scope,label,&names).map_err(DirectoryOpenError::Original)?;
        let options=fields.effective_options();
        let original=match selection{
            RustOrdinaryKind::Directory(original)=>{
                if options&NON_DIRECTORY!=0{return Err(DirectoryOpenError::FileIsDirectory);}
                if intent.maximal_access().is_some()||intent.disk_id(){return Err(DirectoryOpenError::ContextRequired);}
                original
            }
            RustOrdinaryKind::File if options&DIRECTORY!=0=>return Err(DirectoryOpenError::NotADirectory),
            RustOrdinaryKind::File=>return Ok(None),
        };
        if options&!DIR_OPTIONS!=0{return Err(DirectoryOpenError::UnsupportedOptions);}
        let grant=existing_grant(fields.desired_access(),original.parent_allows_file_write())?;
        let share=Share::from_bits(u32::from(fields.share_access())).ok_or(DirectoryOpenError::AccessDenied)?;
        let facts=Facts{
            object:object(tree.pool(),original.volume(),original.nested()),grant,
            parent_write:parent_write(original.parent_allows_file_write()),
        };
        let opening=Opening{tree,member:&scope,facts};
        let pending=self.reserve(&opening,facts,share).map_err(|e|match e{
            ReserveError::SharingViolation=>DirectoryOpenError::ShareViolation,
            ReserveError::DeletePending=>DirectoryOpenError::DeletePending,
            ReserveError::Resources=>DirectoryOpenError::Resources,
            ReserveError::Unavailable=>DirectoryOpenError::Original(io::Error::other("original directory OPEN order is unavailable")),
        })?;
        let media=original.acquire().map_err(DirectoryOpenError::Original)?;
        let held=pending.bind(media).map_err(|e|match e{
            BindError::Original(e)=>e,BindError::Changed=>DirectoryOpenError::WrongTree,
        })?;
        Ok(Some(DirectoryOpen{held}))
    }
}
impl DirectoryOpen{
    pub(super) fn from_bound(held:Held<RustOrdinaryDirectory>)->Self{Self{held}}
    pub(in crate::smb) fn allows_list_directory(&self)->bool{self.held.grant.0&LIST!=0}
    pub(in crate::smb) fn require_current(&self,tree:&RustRoot,scope:&RustSessionWitness)->io::Result<()>{
        require_grant(&self.held.original,tree,scope,self.held.grant)
    }
    pub(in crate::smb) fn prepare_standard(&self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<PreparedDirectoryStandard,FileStandardError>{
        let raw=member.message();
        let query=raw.query_info(member.max_transact()).map_err(FileStandardError::Fields)?;
        if query.class!=InfoClass::File(FileClass::Standard)||member.ordinal()!=0||raw.header.flags&RELATED!=0||
            !matches!(raw.header.routing,Routing::Sync{..})||raw.raw.get(20..24)!=Some(&[0;4][..])||
            !matches!(raw.query_info_target(),Ok(OpenReference::Explicit(_))){return Err(FileStandardError::NotSupported);}
        let scope=member.rust_session_witness().map_err(FileStandardError::Session)?;
        // Standard does not require READ_ATTRIBUTES or FILE_LIST_DIRECTORY;
        // its no-data fields describe the already authorized original OPEN.
        let proof=PreparedDirectoryStandard{open:Arc::downgrade(&self.held._slot.identity),scope,output:query.output_bytes};
        proof.require_current(self,tree).map_err(FileStandardError::Original)?;
        Ok(proof)
    }
    pub(in crate::smb) fn prepare_normalized_name(&self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<PreparedDirectoryName,FileStandardError>{
        let (scope,output)=normalized_query(member)?;
        let view=self.held.original.normalized_name(tree,&scope).map_err(normalized_name_error)?;
        Ok(PreparedDirectoryName{view,scope,output})
    }
    pub(in crate::smb) fn prepare_attribute_tag(&self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<PreparedAttributeTag,FileStandardError>{
        PreparedAttributeTag::prepare(&self.held,member,|scope|self.require_current(tree,scope))
    }
    pub(in crate::smb) fn require_attribute_tag(&self,tree:&RustRoot,proof:&PreparedAttributeTag)->io::Result<()>{
        proof.require(&self.held,|scope|self.require_current(tree,scope))
    }
    pub(in crate::smb) fn close(self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<CloseCompletion,(Self,DirectoryCloseError)>{
        let how=(||{
            let cmd=member.message().close_request().map_err(DirectoryCloseError::Fields)?;
            if !self.held.original.belongs_to(tree){return Err(DirectoryCloseError::WrongTree);}
            let scope=member.rust_session_witness().map_err(DirectoryCloseError::Session)?;
            if !scope.is_close(){return Err(DirectoryCloseError::WrongTree);}
            // Losing the original storage or mode must not keep a read-only
            // handle alive. This does not re-select a path or modify a directory.
            tree.ensure_current(&scope).map_err(DirectoryCloseError::Original)?;
            Ok(cmd.mode)
        })();
        let how=match how{Ok(mode)=>mode,Err(e)=>return Err((self,e))};
        Ok(self.held.close(how).unwrap_or_else(|impossible|match impossible{}))
    }
}
impl CheckedClose for RustOrdinaryDirectory{
    type Error=Infallible;
    fn close_checked(&mut self,_how:CloseMode)->Result<Option<CloseAttributes>,Infallible>{
        // No fully verified logical FILETIMEs, and never substitute F's OS times.
        Ok(None)
    }
}


use super::{query::{Pattern,root::{Child,Fault as PageFault}},root_open::RootQueryError};
use crate::{smb::{connection::sessions::ordinary::WrittenOrdinaryQuery,wire::query_reply::DirectoryReply},
    system::reader::smb::directory::RustDirectoryPage};

/// The real Core page, actual read/scan grant, and one uncommitted search on
/// its exact DirectoryOpen. A dropped packet cannot advance it.
#[must_use]
pub(in crate::smb) struct PreparedOrdinaryQuery{
    reply:DirectoryReply,source:RustDirectoryPage,scope:RustSessionWitness,
}
impl PreparedOrdinaryQuery{
    pub(in crate::smb) fn reply(&self)->&DirectoryReply{&self.reply}
    pub(in crate::smb) fn require_current(&self,open:&DirectoryOpen,tree:&RustRoot)->io::Result<()>{
        if !open.allows_list_directory()||!self.reply.pending().belongs_to(&open.held){
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        open.held.original.require_page(tree,&self.scope,&self.source)
    }
}
impl DirectoryOpen{
    pub(in crate::smb) fn prepare_query(&mut self,tree:&RustRoot,member:&VerifiedMember<'_,'_>,room:usize)
        ->Result<PreparedOrdinaryQuery,RootQueryError>{
        let m=member.message();let limit=member.max_transact();
        m.directory_output(limit).map_err(RootQueryError::Fields)?;
        let scope=member.rust_session_witness().map_err(RootQueryError::Session)?;
        if !self.held.original.belongs_to(tree){return Err(RootQueryError::WrongTree);}
        if !self.allows_list_directory(){return Err(RootQueryError::AccessDenied);}
        let query=m.query_directory(limit).map_err(RootQueryError::Fields)?;
        let (page,source)=self.held.prepare_ordinary_query_with(&query,room,|dir,pattern,after,count|{
            if !pattern.supports_ordinary(dir.nested().is_some()){return Err(PageFault::UnsupportedPattern);}
            let matches=|name:&str,work:&mut usize|pattern.match_bounded(name,work);
            let filter=if matches!(pattern,Pattern::All){None}else{
                Some(&matches as &dyn Fn(&str,&mut usize)->io::Result<bool>)
            };
            let source=dir.page(tree,&scope,after.map(str::as_bytes).unwrap_or_default(),count,filter)
                .map_err(PageFault::Original)?;
            let mut rows=Vec::new();
            for (name,file) in source.rows(){
                rows.try_reserve(1).map_err(|_|PageFault::Resources)?;
                let mut owned=String::new();
                owned.try_reserve_exact(name.len()).map_err(|_|PageFault::Resources)?;
                owned.push_str(name);
                // MS-FSCC §2.1.9: this store supports numeric FILE IDs;
                // a given nested DirectoryId has no established 64-bit ID.
                let (number,eof)=file.map_or((u64::MAX,None),|(number,size)|(number,Some(size)));
                rows.push(Child{name:owned,number,eof});
            }
            Ok((rows,source))
        })?;
        let reply=page.into_reply()?;
        Ok(PreparedOrdinaryQuery{reply,source,scope})
    }
    pub(in crate::smb) fn apply_written_query(&mut self,tree:&RustRoot,receipt:WrittenOrdinaryQuery)->io::Result<()>{
        let query=receipt.into_prepared();
        query.require_current(self,tree)?;
        if !query.reply.into_effect().apply(&mut self.held){return Err(io::ErrorKind::PermissionDenied.into());}
        Ok(())
    }
}

mod rename;
pub(crate) use rename::OrdinaryDirectoryRenameAdmission;
pub(in crate::smb) use rename::PreparedDirectoryRename;

#[cfg(test)]
#[path="../../test/smb/backend_directory_open.rs"]
mod tests;
