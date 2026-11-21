//! Signed standalone QUERY_INFO on the exact original OPEN. Only root
//! filesystem attributes/size, verified IDs and supported object facts.
use super::{CheckedBatch,Connection,Fault,Payload,Result,RustSessionWitness,SyncReply,UnsignedSmall,VerifiedMember,current,
    root_io::RequestSlot};
use super::super::opens::Opened;
use crate::smb::{
    backend::{FileStandardError,PreparedAttributeTag,PreparedDirectoryStandard,PreparedFileName,PreparedFileStandard,PreparedRetainedStandard,PreparedRootFsAttributes,PreparedRootFsSize,PreparedRootInternal,PreparedWriterName,PreparedWriterStandard,RootFsError},
    wire::{Error as WireError,file_name_reply::UnsignedFileName,fixed_reply::{FileStandardFields,FsInfoFailure},
        io_fields::{IoFieldError,OpenReference,WireOpenId},message::{Message,RELATED,Routing},
        query_fields::{FileClass,FsClass,InfoClass,QueryError}},
};
use crate::system::reader::smb::file_number::SmbFileIndex;
use std::{io,num::NonZeroU64};

pub(in crate::smb) const QUERY_INFO:u16=16;
impl CheckedBatch<'_,'_>{
    pub(in crate::smb) fn try_send_open_info(&mut self)->Result<bool>{
        self.before_reply()?;
        let frame=self.original.frame;
        let [m]=frame.messages() else{return Ok(false);};
        let Routing::Sync{tree:tid}=m.header.routing else{return Ok(false);};
        if m.header.command!=QUERY_INFO||m.header.flags&RELATED!=0{return Ok(false);}
        let sid=self.ids.at(0).ok_or(Fault::SessionState)?;
        let output={
            let (session,trees,opens)=self.owner.member_tables(sid,&self.revision)?;
            let Some(tree)=trees.root(tid)else{return self.fs_info_refusal(m,sid,FsInfoFailure::NetworkNameDeleted);};
            let ids=match m.query_info_target(){
                Ok(OpenReference::Explicit(v))=>v,
                Err(IoFieldError::NoExplicitOpen)=>return self.fs_info_refusal(m,sid,FsInfoFailure::FileClosed),
                _=>return self.fs_info_refusal(m,sid,FsInfoFailure::InvalidParameter),
            };
            let Some(slot)=opens.find(ids).filter(|s|s.tree()==tid)else{
                return self.fs_info_refusal(m,sid,FsInfoFailure::FileClosed);
            };
            match self.original.payload(0){
                Some(Payload::Covered)=>{},
                Some(Payload::InvalidParameter)=>return self.fs_info_refusal(m,sid,FsInfoFailure::InvalidParameter),
                _=>return Err(Fault::SessionState),
            };
            let member=VerifiedMember{message:m,ordinal:0,session};
            let info=match m.query_info(member.max_transact()){
                Ok(v)=>v,Err(e)=>return self.fs_info_refusal(m,sid,query_failure(e)),
            };
            let original=RequestSlot{sid,tree:tid,mid:m.header.message};
            match (slot.object(),info.class){
                (Opened::Root(open),InfoClass::Filesystem(FsClass::Attribute))=>{
                    let prepared=match open.prepare_fs_attributes(tree,&member){
                        Ok(v)=>v,Err(e)=>return self.fs_info_refusal(m,sid,root_failure(e)?),
                    };
                    let pending=PendingRootAttributes{original,ids,prepared};
                    let draft=UnsignedSmall::root_fs_attributes(&member,&pending).map_err(|_|Fault::WrongReply)?;
                    (PendingInfo::Root(pending),draft)
                },
                (Opened::Root(open),InfoClass::Filesystem(FsClass::Size))=>{
                    let prepared=match open.prepare_fs_size(tree,&member){
                        Ok(v)=>v,Err(e)=>return self.fs_info_refusal(m,sid,root_failure(e)?),
                    };
                    let pending=PendingRootSize{original,ids,prepared};
                    let draft=UnsignedSmall::root_fs_size(&member,&pending).map_err(|_|Fault::WrongReply)?;
                    (PendingInfo::Size(pending),draft)
                },
                (Opened::Root(open),InfoClass::File(FileClass::Internal))=>{
                    let prepared=match open.prepare_internal(tree,&member){
                        Ok(v)=>v,Err(e)=>return self.fs_info_refusal(m,sid,root_failure(e)?),
                    };
                    let pending=PendingInternal{original,ids,prepared:InternalPrepared::Root(prepared)};
                    let draft=UnsignedSmall::internal_info(&member,&pending).map_err(|_|Fault::WrongReply)?;
                    (PendingInfo::Internal(pending),draft)
                },
                (open @ (Opened::File(_)|Opened::Writer(_)),InfoClass::File(FileClass::Internal))=>{
                    let scope=member.rust_session_witness()?;
                    let actual=match open{
                        Opened::File(f)=>f.smb_index(tree,&scope),
                        Opened::Writer(f)=>f.smb_index(tree,&scope),
                        _=>return Err(Fault::SessionState),
                    };
                    let index=match actual{
                        Ok(v)=>v,
                        Err(e)=>return self.fs_info_refusal(m,sid,file_origin_failure(e)),
                    };
                    let facts=NamedIndex{scope,output:info.output_bytes,index};
                    let prepared=if matches!(open,Opened::File(_)){InternalPrepared::File(facts)}
                        else{InternalPrepared::Writer(facts)};
                    let pending=PendingInternal{original,ids,prepared};
                    let draft=UnsignedSmall::internal_info(&member,&pending).map_err(|_|Fault::WrongReply)?;
                    (PendingInfo::Internal(pending),draft)
                },
                (open @ (Opened::Directory(_)|Opened::File(_)|Opened::Writer(_)|Opened::Retained(_)),InfoClass::File(FileClass::Standard))=>{
                    let prepared=match open{
                        Opened::Directory(d)=>d.prepare_standard(tree,&member).map(StandardPrepared::Directory),
                        Opened::File(f)=>f.prepare_standard(tree,&member).map(StandardPrepared::Reader),
                        Opened::Writer(f)=>f.prepare_standard(tree,&member).map(StandardPrepared::Writer),
                        Opened::Retained(f)=>f.prepare_standard(tree,&member).map(StandardPrepared::Retained),
                        _=>return Err(Fault::SessionState),
                    };
                    let prepared=match prepared{
                        Ok(v)=>v,Err(e)=>return self.fs_info_refusal(m,sid,file_failure(e)?),
                    };
                    let pending=PendingFileStandard{original,ids,prepared};
                    let draft=UnsignedSmall::file_standard(&member,&pending).map_err(|_|Fault::WrongReply)?;
                    (PendingInfo::File(pending),draft)
                },
                (open @ (Opened::Root(_)|Opened::Directory(_)|Opened::File(_)|Opened::Writer(_)),InfoClass::File(FileClass::AttributeTag))=>{
                    let prepared=match open{
                        Opened::Root(d)=>d.prepare_attribute_tag(tree,&member),
                        Opened::Directory(d)=>d.prepare_attribute_tag(tree,&member),
                        Opened::File(f)=>f.prepare_attribute_tag(tree,&member),
                        Opened::Writer(f)=>f.prepare_attribute_tag(tree,&member),
                        _=>return Err(Fault::SessionState),
                    };
                    let prepared=match prepared{
                        Ok(v)=>v,Err(e)=>return self.fs_info_refusal(m,sid,file_failure(e)?),
                    };
                    let pending=PendingAttributeTag{original,ids,prepared};
                    let draft=UnsignedSmall::attribute_tag(&member,&pending).map_err(|_|Fault::WrongReply)?;
                    (PendingInfo::AttributeTag(pending),draft)
                },
                (open @ (Opened::File(_)|Opened::Writer(_)),InfoClass::File(FileClass::NormalizedName))=>{
                    let prepared=match open{
                        Opened::File(f)=>f.prepare_normalized_name(tree,&member).map(NamePrepared::Reader),
                        Opened::Writer(f)=>f.prepare_normalized_name(tree,&member).map(NamePrepared::Writer),
                        _=>return Err(Fault::SessionState),
                    };
                    let prepared=match prepared{
                        Ok(v)=>v,
                        Err(e)=>return self.fs_info_refusal(m,sid,file_failure(e)?),
                    };
                    let pending=PendingFileName{original,ids,prepared};
                    let draft=match UnsignedFileName::from_original(&member,pending){
                        Ok(v)=>v,
                        Err(WireError::Limit)=>return self.fs_info_refusal(m,sid,FsInfoFailure::Resources),
                        Err(_)=>return Err(Fault::WrongReply),
                    };
                    self.send_file_name(draft)?;
                    return Ok(true);
                },
                _=>return self.fs_info_refusal(m,sid,FsInfoFailure::NotSupported),
            }
        };
        self.next=1;
        self.small_replies(std::iter::once(output.1))?.send_info(output.0)?;
        Ok(true)
    }

    fn fs_info_refusal(&mut self,m:&Message<'_>,sid:NonZeroU64,status:FsInfoFailure)->Result<bool>{
        self.owner.member_scope(sid,&self.revision)?;
        // A consumed but undercharged MID may receive this error; it is
        // not handed an executable VerifiedMember for the object.
        let draft=UnsignedSmall::fs_info_refusal(&FsInfoRefusal{raw:m,sid,status}).map_err(|_|Fault::WrongReply)?;
        self.next=1;self.small_replies(std::iter::once(draft))?.send()?;Ok(true)
    }

    fn send_file_name(&mut self,draft:UnsignedFileName)->Result<()>{
        self.next=1;self.halted=true;
        let ids=&self.ids;let revision=&self.revision;
        let plan=self.owner.plan_debit(&mut self.original.debit)?;
        let payable=draft.credit(plan.credit_response(0).ok_or(Fault::NoReply)?);
        let sid=payable.sid();
        if ids.at(0)!=Some(sid){return Err(Fault::SessionState);}
        plan.owner.member_scope(sid,revision)?;
        let (_,bound)=plan.owner.valid_slot(sid).ok_or(Fault::SessionState)?;
        let (record,pending)=bound.sign_file_name(revision,payable).map_err(|_|Fault::SessionState)?.into_send();
        let tx=plan.bind_record(record)?;
        SyncReply{tx,ids,revision,count:1}.send_file_name(pending)
    }
}
fn query_failure(e:QueryError)->FsInfoFailure{
    match e{
        QueryError::InvalidInfoClass=>FsInfoFailure::InvalidInfoClass,
        QueryError::NotSupported=>FsInfoFailure::NotSupported,
        _=>FsInfoFailure::InvalidParameter,
    }
}
pub(super) fn root_failure(e:RootFsError)->Result<FsInfoFailure>{
    Ok(match e{
        RootFsError::Fields(e)=>query_failure(e),
        RootFsError::NotSupported=>FsInfoFailure::NotSupported,
        RootFsError::WrongTree=>FsInfoFailure::NetworkNameDeleted,
        RootFsError::Session(e)=>return Err(e),
        RootFsError::Original(e)=>match e.kind(){
            io::ErrorKind::PermissionDenied=>FsInfoFailure::AccessDenied,
            io::ErrorKind::OutOfMemory|io::ErrorKind::WouldBlock=>FsInfoFailure::Resources,
            _=>FsInfoFailure::ServerUnavailable,
        },
    })
}
fn file_failure(e:FileStandardError)->Result<FsInfoFailure>{
    Ok(match e{
        FileStandardError::Fields(e)=>query_failure(e),
        FileStandardError::Session(e)=>return Err(e),
        FileStandardError::NotSupported=>FsInfoFailure::NotSupported,
        FileStandardError::Original(e)=>file_origin_failure(e),
    })
}
fn file_origin_failure(e:io::Error)->FsInfoFailure{
    match e.kind(){
        io::ErrorKind::PermissionDenied=>FsInfoFailure::AccessDenied,
        io::ErrorKind::NotConnected=>FsInfoFailure::VolumeDismounted,
        io::ErrorKind::ReadOnlyFilesystem=>FsInfoFailure::MediaWriteProtected,
        io::ErrorKind::NotFound|io::ErrorKind::WouldBlock=>FsInfoFailure::FileNotAvailable,
        io::ErrorKind::OutOfMemory=>FsInfoFailure::Resources,
        _=>FsInfoFailure::ServerUnavailable,
    }
}
pub(in crate::smb) struct FsInfoRefusal<'a,'r>{raw:&'a Message<'r>,sid:NonZeroU64,status:FsInfoFailure}
impl<'a,'r> FsInfoRefusal<'a,'r>{
    pub(in crate::smb) fn fields(&self)->(&'a Message<'r>,NonZeroU64,FsInfoFailure){(self.raw,self.sid,self.status)}
}

#[must_use]
pub(in crate::smb) struct PendingRootAttributes{original:RequestSlot,ids:WireOpenId,prepared:PreparedRootFsAttributes}
impl PendingRootAttributes{
    pub(in crate::smb) fn sid(&self)->NonZeroU64{self.original.sid}
    pub(in crate::smb) fn matches(&self,m:&VerifiedMember<'_,'_>)->bool{self.original.matches(m,self.sid())}
    pub(in crate::smb) fn payload(&self)->(u32,bool,u32){
        let (read_only,maximum)=self.prepared.policy();(self.prepared.output(),read_only,maximum)
    }
    fn check(&self,c:&Connection)->Result<()>{
        let tree=self.original.root(c)?;
        let slot=c.open_table(self.sid())?.find(self.ids).filter(|s|s.tree()==self.original.tree).ok_or(Fault::SessionState)?;
        let Opened::Root(open)=slot.object()else{return Err(Fault::SessionState);};
        self.prepared.require_current(open,tree).map_err(|_|Fault::SessionState)
    }
}
#[must_use]
pub(in crate::smb) struct PendingRootSize{original:RequestSlot,ids:WireOpenId,prepared:PreparedRootFsSize}
impl PendingRootSize{
    pub(in crate::smb) fn sid(&self)->NonZeroU64{self.original.sid}
    pub(in crate::smb) fn matches(&self,m:&VerifiedMember<'_,'_>)->bool{self.original.matches(m,self.sid())}
    pub(in crate::smb) fn payload(&self)->(u32,Option<(u64,u64)>){self.prepared.values()}
    fn check(&self,c:&Connection)->Result<()>{
        let tree=self.original.root(c)?;
        let slot=c.open_table(self.sid())?.find(self.ids).filter(|s|s.tree()==self.original.tree).ok_or(Fault::SessionState)?;
        let Opened::Root(open)=slot.object()else{return Err(Fault::SessionState);};
        self.prepared.require_current(open,tree).map_err(|_|Fault::SessionState)
    }
}
#[must_use]
pub(in crate::smb) struct PendingInternal{original:RequestSlot,ids:WireOpenId,prepared:InternalPrepared}
struct NamedIndex{scope:RustSessionWitness,output:u32,index:SmbFileIndex}
enum InternalPrepared{Root(PreparedRootInternal),File(NamedIndex),Writer(NamedIndex)}
impl PendingInternal{
    pub(in crate::smb) fn sid(&self)->NonZeroU64{self.original.sid}
    pub(in crate::smb) fn matches(&self,m:&VerifiedMember<'_,'_>)->bool{self.original.matches(m,self.sid())}
    pub(in crate::smb) fn values(&self)->(u32,u64){
        match &self.prepared{
            InternalPrepared::Root(root)=>root.values(),
            InternalPrepared::File(v)|InternalPrepared::Writer(v)=>(v.output,v.index.number()),
        }
    }
    fn check(&self,c:&Connection)->Result<()>{
        let tree=self.original.root(c)?;
        let slot=c.open_table(self.sid())?.find(self.ids).filter(|s|s.tree()==self.original.tree).ok_or(Fault::SessionState)?;
        let (actual,expected)=match (&self.prepared,slot.object()){
            (InternalPrepared::Root(v),Opened::Root(open))=>return v.require_current(open,tree).map_err(|_|Fault::SessionState),
            (InternalPrepared::File(v),Opened::File(open))=>(open.smb_index(tree,&v.scope),v.index),
            (InternalPrepared::Writer(v),Opened::Writer(open))=>(open.smb_index(tree,&v.scope),v.index),
            _=>return Err(Fault::SessionState),
        };
        if actual.map_err(|_|Fault::SessionState)?!=expected{return Err(Fault::SessionState);}
        Ok(())
    }
}
#[must_use]
pub(in crate::smb) struct PendingFileStandard{original:RequestSlot,ids:WireOpenId,prepared:StandardPrepared}
enum StandardPrepared{
    Directory(PreparedDirectoryStandard),Reader(PreparedFileStandard),Writer(PreparedWriterStandard),
    Retained(PreparedRetainedStandard),
}
impl PendingFileStandard{
    pub(in crate::smb) fn sid(&self)->NonZeroU64{self.original.sid}
    pub(in crate::smb) fn matches(&self,m:&VerifiedMember<'_,'_>)->bool{
        m.ordinal()==0&&self.original.matches(m,self.sid())&&
            m.message().query_info_target().is_ok_and(|id|id==OpenReference::Explicit(self.ids))
    }
    pub(in crate::smb) fn payload(&self)->(u32,FileStandardFields){
        let (output,allocation,eof,delete_pending)=match &self.prepared{
            StandardPrepared::Directory(p)=>return (p.output(),FileStandardFields::Directory),
            StandardPrepared::Reader(p)=>p.values(),
            StandardPrepared::Writer(p)=>p.values(),
            StandardPrepared::Retained(p)=>p.values(),
        };
        (output,FileStandardFields::File{allocation,eof,delete_pending})
    }
    fn check(&self,c:&Connection)->Result<()>{
        let tree=self.original.root(c)?;
        let slot=c.open_table(self.sid())?.find(self.ids).filter(|s|s.tree()==self.original.tree).ok_or(Fault::SessionState)?;
        match (&self.prepared,slot.object()){
            (StandardPrepared::Directory(p),Opened::Directory(open))=>p.require_current(open,tree),
            (StandardPrepared::Reader(p),Opened::File(open))=>p.require_current(open,tree),
            (StandardPrepared::Writer(p),Opened::Writer(open))=>p.require_current(open,tree),
            (StandardPrepared::Retained(p),Opened::Retained(open))=>p.require_current(open,tree),
            _=>return Err(Fault::SessionState),
        }.map_err(|_|Fault::SessionState)
    }
}
/// One real normalized-name observation travels with the bytes describing
/// it until the original receipt and ledger are checked after transmission.
#[must_use]
pub(in crate::smb) struct PendingFileName{original:RequestSlot,ids:WireOpenId,prepared:NamePrepared}
enum NamePrepared{Reader(PreparedFileName),Writer(PreparedWriterName)}
impl PendingFileName{
    pub(in crate::smb) fn sid(&self)->NonZeroU64{self.original.sid}
    pub(in crate::smb) fn matches(&self,m:&VerifiedMember<'_,'_>)->bool{
        m.ordinal()==0&&self.original.matches(m,self.sid())&&
            m.message().query_info_target().is_ok_and(|id|id==OpenReference::Explicit(self.ids))
    }
    pub(in crate::smb) fn payload(&self)->(u32,&str){
        match &self.prepared{
            NamePrepared::Reader(p)=>(p.output(),p.path()),
            NamePrepared::Writer(p)=>(p.output(),p.path()),
        }
    }
    fn check(&self,c:&Connection)->Result<()>{
        let tree=self.original.root(c)?;
        let slot=c.open_table(self.sid())?.find(self.ids).filter(|s|s.tree()==self.original.tree).ok_or(Fault::SessionState)?;
        match (&self.prepared,slot.object()){
            (NamePrepared::Reader(p),Opened::File(open))=>p.require_current(open,tree),
            (NamePrepared::Writer(p),Opened::Writer(open))=>p.require_current(open,tree),
            _=>return Err(Fault::SessionState),
        }.map_err(|_|Fault::SessionState)
    }
}
#[must_use]
pub(in crate::smb) struct PendingAttributeTag{original:RequestSlot,ids:WireOpenId,prepared:PreparedAttributeTag}
impl PendingAttributeTag{
    pub(in crate::smb) fn sid(&self)->NonZeroU64{self.original.sid}
    pub(in crate::smb) fn matches(&self,m:&VerifiedMember<'_,'_>)->bool{
        m.ordinal()==0&&self.original.matches(m,self.sid())&&
            m.message().query_info_target().is_ok_and(|id|id==OpenReference::Explicit(self.ids))
    }
    pub(in crate::smb) fn values(&self)->(u32,u32,u32){self.prepared.values()}
    fn check(&self,c:&Connection)->Result<()>{
        let tree=self.original.root(c)?;
        let slot=c.open_table(self.sid())?.find(self.ids).filter(|s|s.tree()==self.original.tree).ok_or(Fault::SessionState)?;
        match slot.object(){
            Opened::Root(open)=>open.require_attribute_tag(tree,&self.prepared),
            Opened::Directory(open)=>open.require_attribute_tag(tree,&self.prepared),
            Opened::File(open)=>open.require_attribute_tag(tree,&self.prepared),
            Opened::Writer(open)=>open.require_attribute_tag(tree,&self.prepared),
            _=>return Err(Fault::SessionState),
        }.map_err(|_|Fault::SessionState)
    }
}
enum PendingInfo{
    Root(PendingRootAttributes),Size(PendingRootSize),Internal(PendingInternal),File(PendingFileStandard),
    AttributeTag(PendingAttributeTag),
}
impl PendingInfo{
    fn check(&self,c:&Connection)->Result<()>{
        match self{
            Self::Root(v)=>v.check(c),Self::Size(v)=>v.check(c),Self::Internal(v)=>v.check(c),
            Self::File(v)=>v.check(c),Self::AttributeTag(v)=>v.check(c),
        }
    }
}
impl SyncReply<'_>{
    fn send_info(self,pending:PendingInfo)->Result<()>{
        pending.check(self.tx.plan.owner)?;
        let SyncReply{tx,ids,revision,count}=self.write_original()?.0;
        tx.publish_with(|c|{current(c,ids,revision,count)?;pending.check(c)})
    }
    fn send_file_name(self,pending:PendingFileName)->Result<()>{
        pending.check(self.tx.plan.owner)?;
        let SyncReply{tx,ids,revision,count}=self.write_original()?.0;
        tx.publish_with(|c|{current(c,ids,revision,count)?;pending.check(c)})
    }
}
