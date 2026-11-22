//! Original paid and signed standalone CREATE/CLOSE. Only CREATE resolves
//! an exact approved name. Overwrite selects the same FILE's private work draft;
//! close never reopens or searches by name.
use super::{CheckedBatch,Connection,Fault,Payload,Result,RustRoot,RustSessionWitness,SyncReply,
    TreeTable,UnsignedSmall,VerifiedMember,current};
use super::super::{SessionView,opens::{OpenCloseError,OpenSeat,Opened,OpenSlot,OpenTable}};
use super::super::super::Phase;
use crate::smb::{backend::{CloseCompletion,DIRECTORY_ATTRIBUTES,DirectoryCloseError,DirectoryOpenError,FileCloseError,FileOpenError,FILE_ATTRIBUTES,
    PreparedBirth,PreparedDirectoryBirth,PreparedNamedFile,RangeIoError,RootCloseError,RootOpenError,path::NameError},session::BatchRevision,
    wire::{create_fields::{CurrentName,Disposition,Error as CreateError,SpecialOpen},fixed_reply::RootFailure,io_fields::{IoFieldError,OpenReference,WireOpenId},
        io_reply::FileReply,message::{Message,RELATED,Routing},path_fields::FieldError}};
use std::{io,num::NonZeroU64,sync::Arc};

pub(in crate::smb) mod probe;
pub(in crate::smb) mod retained_restore;
const CREATE:u16=5;
const CLOSE:u16=6;
const DIRECTORY_FILE:u32=1;

/// Only the original signed, paid CREATE, with its actual tree and the
/// same owner keeping an undisclosed wire seat. Neither ID is exposed to
/// storage. The genuine FILE vote and physical parent are still required.
#[must_use]
pub(in crate::smb) struct BirthAdmission<'seat,'step,'raw>{
    _seat:&'seat OpenSeat,tree:&'seat RustRoot,member:VerifiedMember<'step,'raw>,
}
impl<'seat,'step,'raw> BirthAdmission<'seat,'step,'raw>{
    pub(in crate::smb) fn into_original(self)->(&'seat RustRoot,VerifiedMember<'step,'raw>){
        (self.tree,self.member)
    }
}

impl CheckedBatch<'_,'_>{
    pub(in crate::smb) fn try_send_open_close(&mut self)->Result<bool>{
        self.before_reply()?;
        let frame=self.original.frame;
        let [m]=frame.messages() else{return Ok(false);};
        let Routing::Sync{tree}=m.header.routing else{return Ok(false);};
        if m.header.flags&RELATED!=0||!matches!(m.header.command,CREATE|CLOSE){return Ok(false);}
        if self.original.payload(0)!=Some(Payload::NoVariableRule){return Err(Fault::SessionState);}
        let sid=self.ids.at(0).ok_or(Fault::SessionState)?;
        let original=RequestSlot{sid,tree,mid:m.header.message};
        match m.header.command {
            CREATE if m.create_fields().is_ok_and(|f|matches!(f.disposition(),
                Disposition::Create|Disposition::OpenIf|Disposition::Overwrite|Disposition::OverwriteIf))
                =>self.create_named(m,original)?,
            CREATE=>self.open_object(m,original)?,
            CLOSE=>self.close_object(m,original)?,
            _=>return Err(Fault::WrongReply),
        }
        Ok(true)
    }

    fn create_named(&mut self,m:&Message<'_>,request:RequestSlot)->Result<()>{
        let sid=request.sid;
        let modes=Arc::clone(&self.owner.origin.run.share_modes);
        let global=Arc::clone(&self.owner.origin.run.persistent);
        let pending={
            let (session,trees,opens)=self.owner.member_tables(sid,&self.revision)?;
            let member=VerifiedMember{message:m,ordinal:0,session};
            let Some(tree)=trees.root(request.tree)else{
                let denied=refused(&member,sid,RootFailure::NetworkNameDeleted)?;
                return self.root_refusal(denied);
            };
            let scope=member.rust_session_witness()?;
            let seat=match opens.seat(&global,request.tree){
                Ok(seat)=>seat,
                Err(Fault::Resources|Fault::Exhausted)=>{
                    let denied=refused(&member,sid,RootFailure::InsufficientResources)?;
                    return self.root_refusal(denied);
                },
                Err(e)=>return Err(e),
            };
            let directory=m.create_fields().map_err(|_|Fault::WrongReply)?.effective_options()&DIRECTORY_FILE!=0;
            let admission=BirthAdmission{_seat:&seat,tree,member};
            let selected=if directory{
                match modes.prepare_directory_birth(admission){
                    Ok(PreparedDirectoryBirth::Opened(dir))=>Ok((Opened::Directory(dir),CreateAction::Opened)),
                    Ok(PreparedDirectoryBirth::Created(dir))=>Ok((Opened::Directory(dir),CreateAction::Created)),
                    Err(e)=>Err(directory_open_failure(e)?),
                }
            }else{
                match modes.prepare_file_birth(admission){
                    Ok(PreparedBirth::Opened(file))=>Ok((Opened::Writer(file),CreateAction::Opened)),
                    Ok(PreparedBirth::Created(file))=>Ok((Opened::Writer(file),CreateAction::Created)),
                    Ok(PreparedBirth::Overwritten(file))=>Ok((Opened::Writer(file),CreateAction::Overwritten)),
                    Err(e)=>Err(file_open_failure(e)?),
                }
            };
            let (held,action)=match selected{
                Ok(v)=>v,
                Err(why)=>{
                    drop(seat);
                    let member=VerifiedMember{message:m,ordinal:0,session:self.owner.member_scope(sid,&self.revision)?};
                    let denied=refused(&member,sid,why)?;
                    return self.root_refusal(denied);
                },
            };
            // No further allocation of routing IDs after a real birth.
            // If this owner is lost, disconnect rather than sign failure.
            let slot=opens.materialize(seat,held)?;
            PendingOpen{request,scope,slot,action}
        };
        let member=VerifiedMember{message:m,ordinal:0,session:self.owner.member_scope(sid,&self.revision)?};
        let draft=UnsignedSmall::opened(&member,sid,&pending).map_err(|_|Fault::WrongReply)?;
        self.next=1;
        self.small_replies(std::iter::once(draft))?.send_open(pending)
    }

    fn open_object(&mut self,m:&Message<'_>,request:RequestSlot)->Result<()>{
        let sid=request.sid;
        let member=VerifiedMember{message:m,ordinal:0,session:self.owner.member_scope(sid,&self.revision)?};
        let Some(tree)=self.owner.tree_table(sid)?.root(request.tree) else{
            let denied=refused(&member,sid,RootFailure::NetworkNameDeleted)?;
            return self.root_refusal(denied);
        };
        let modes=&self.owner.origin.run.share_modes;
        let opened=match modes.prepare_root_open(tree,&member){
            Ok(root)=>Opened::Root(root),
            Err(RootOpenError::NonRoot)=>match modes.prepare_retained_open(tree,&member){
                Ok(Some(retained))=>Opened::Retained(retained),
                // Only a name outside the exact approved R branch reaches
                // ordinary selection. An R error never reinterprets its path.
                Ok(None)=>match modes.prepare_directory_open(tree,&member){
                    Ok(Some(dir))=>Opened::Directory(dir),
                    // None is actual ordinary FILE classification from the
                    // same Core plan, not a retry after failed directory IO.
                    Ok(None)=>match modes.prepare_file_open(tree,&member){
                        Ok(PreparedNamedFile::Read(file))=>Opened::File(file),
                        Ok(PreparedNamedFile::Write(writer))=>Opened::Writer(writer),
                        Err(e)=>{
                            let denied=refused(&member,sid,file_open_failure(e)?)?;
                            return self.root_refusal(denied);
                        },
                    },
                    Err(e)=>{
                        let denied=refused(&member,sid,directory_open_failure(e)?)?;
                        return self.root_refusal(denied);
                    },
                },
                Err(e)=>{
                    let denied=refused(&member,sid,directory_open_failure(e)?)?;
                    return self.root_refusal(denied);
                },
            },
            Err(error)=>{
                let denied=refused(&member,sid,open_failure(error)?)?;
                return self.root_refusal(denied);
            },
        };
        let scope=member.rust_session_witness()?;
        let global=Arc::clone(&self.owner.origin.run.persistent);
        let slot=match self.owner.open_table_mut(sid)?.reserve(&global,request.tree,opened){
            Ok(s)=>s,
            Err(Fault::Resources|Fault::Exhausted)=>{
                let member=VerifiedMember{message:m,ordinal:0,session:self.owner.member_scope(sid,&self.revision)?};
                let denied=refused(&member,sid,RootFailure::InsufficientResources)?;
                return self.root_refusal(denied);
            },
            Err(e)=>return Err(e),
        };
        let pending=PendingOpen::existing(request,scope,slot);
        let member=VerifiedMember{message:m,ordinal:0,session:self.owner.member_scope(sid,&self.revision)?};
        let draft=UnsignedSmall::opened(&member,sid,&pending).map_err(|_|Fault::WrongReply)?;
        self.next=1;
        self.small_replies(std::iter::once(draft))?.send_open(pending)
    }

    fn close_object(&mut self,m:&Message<'_>,request:RequestSlot)->Result<()>{
        let sid=request.sid;
        let (closed,draft)={
            let (session,trees,opens)=self.owner.member_tables(sid,&self.revision)?;
            let member=VerifiedMember{message:m,ordinal:0,session};
            let wire=match m.close_target(){
                Ok(OpenReference::Explicit(wire))=>wire,
                Err(IoFieldError::NoExplicitOpen)=>{
                    let denied=refused(&member,sid,RootFailure::FileClosed)?;
                    return self.root_refusal(denied);
                },
                _=>{
                    let denied=refused(&member,sid,RootFailure::InvalidParameter)?;
                    return self.root_refusal(denied);
                },
            };
            // §3.3.5.10: find the two halves in the original Session first;
            // then find and verify the requested, originally bound Tree.
            let Some(slot)=opens.find(wire) else{
                let denied=refused(&member,sid,RootFailure::FileClosed)?;
                return self.root_refusal(denied);
            };
            let Some(tree)=trees.root(request.tree).filter(|_|slot.tree()==request.tree) else{
                let denied=refused(&member,sid,RootFailure::NetworkNameDeleted)?;
                return self.root_refusal(denied);
            };
            let scope=member.rust_session_witness()?;
            let receipt=match opens.close(wire,request.tree,tree,&member).ok_or(Fault::SessionState)?{
                Ok(done)=>done,
                Err(error)=>{
                    let denied=refused(&member,sid,close_failure(error)?)?;
                    return self.root_refusal(denied);
                },
            };
            let closed=ClosedOpen::new(request,scope,receipt)?;
            let draft=UnsignedSmall::closed(&member,sid,&closed).map_err(|_|Fault::WrongReply)?;
            (closed,draft)
        };
        self.next=1;
        self.small_replies(std::iter::once(draft))?.send_closed(closed)
    }

    fn root_refusal(&mut self,draft:UnsignedSmall)->Result<()>{
        self.next=1;self.small_replies(std::iter::once(draft))?.send()
    }
}

fn refused(m:&VerifiedMember<'_,'_>,sid:NonZeroU64,error:RootFailure)->Result<UnsignedSmall>{
    UnsignedSmall::root_failure(m,sid,error).map_err(|_|Fault::WrongReply)
}
pub(super) fn original_failure(e:io::Error)->RootFailure{
    match e.kind(){
        io::ErrorKind::PermissionDenied=>RootFailure::AccessDenied,
        io::ErrorKind::OutOfMemory=>RootFailure::InsufficientResources,
        _=>RootFailure::ServerUnavailable,
    }
}
pub(super) fn open_failure(why:RootOpenError)->Result<RootFailure>{
    Ok(match why{
        RootOpenError::Fields(CreateError::Unsupported|CreateError::Field(FieldError::Unsupported))|
        RootOpenError::NonRoot|RootOpenError::ContextRequired|RootOpenError::UnsupportedOptions=>RootFailure::NotSupported,
        RootOpenError::Special(SpecialOpen::RootIsDirectory)=>RootFailure::FileIsDirectory,
        RootOpenError::Special(_)=>RootFailure::NotSupported,
        RootOpenError::Fields(_)=>RootFailure::InvalidParameter,
        RootOpenError::AccessDenied=>RootFailure::AccessDenied,
        RootOpenError::ShareViolation=>RootFailure::SharingViolation,
        RootOpenError::Resources=>RootFailure::InsufficientResources,
        RootOpenError::Session(e)=>return Err(e),
        RootOpenError::Original(e)=>original_failure(e),
        RootOpenError::WrongTree=>RootFailure::NetworkNameDeleted,
    })
}
fn file_open_failure(why:FileOpenError)->Result<RootFailure>{
    use RootFailure as W;
    Ok(match why {
        FileOpenError::Fields(CreateError::Unsupported|CreateError::Field(FieldError::Unsupported))|
        FileOpenError::Special(_)|FileOpenError::NotFile|FileOpenError::ContextRequired|
        FileOpenError::UnsupportedOptions=>W::NotSupported,
        FileOpenError::Fields(_)=>W::InvalidParameter,
        FileOpenError::Name(e)=>name_failure(e),
        FileOpenError::AccessDenied=>W::AccessDenied,
        FileOpenError::ShareViolation=>W::SharingViolation,
        FileOpenError::DeletePending=>W::DeletePending,
        FileOpenError::Resources=>W::InsufficientResources,
        FileOpenError::Range(e)=>match e{
            RangeIoError::Conflict=>W::FileLockConflict,
            RangeIoError::Resources=>W::InsufficientResources,
            RangeIoError::Unavailable=>W::ServerUnavailable,
        },
        FileOpenError::Session(e)=>return Err(e),
        FileOpenError::Original(e) if e.kind()==io::ErrorKind::ConnectionAborted=>return Err(Fault::WriteOutcomeUnknown),
        FileOpenError::Original(e)=>named_original_failure(e),
        FileOpenError::WrongTree=>W::NetworkNameDeleted,
    })
}
fn name_failure(why:NameError)->RootFailure{
    match why{
        NameError::Field(_)=>RootFailure::InvalidParameter,
        NameError::InvalidName=>RootFailure::ObjectNameInvalid,
        NameError::TooLong=>RootFailure::NameTooLong,
        NameError::Resources=>RootFailure::InsufficientResources,
    }
}
fn named_original_failure(e:io::Error)->RootFailure{
    use RootFailure as W;
    match e.kind(){
        io::ErrorKind::NotFound=>W::ObjectNameNotFound,
        io::ErrorKind::AlreadyExists=>W::ObjectNameCollision,
        io::ErrorKind::NotADirectory=>W::ObjectPathNotFound,
        io::ErrorKind::IsADirectory=>W::FileIsDirectory,
        io::ErrorKind::NotConnected=>W::VolumeDismounted,
        io::ErrorKind::WouldBlock=>W::FileNotAvailable,
        io::ErrorKind::Unsupported=>W::NotSupported,
        io::ErrorKind::StorageFull=>W::DiskFull,
        io::ErrorKind::ReadOnlyFilesystem=>W::MediaWriteProtected,
        _=>original_failure(e),
    }
}
pub(super) fn directory_open_failure(why:DirectoryOpenError)->Result<RootFailure>{
    use RootFailure as W;
    Ok(match why{
        DirectoryOpenError::Fields(CreateError::Unsupported|CreateError::Field(FieldError::Unsupported))|
        DirectoryOpenError::Special(_)|DirectoryOpenError::ContextRequired|DirectoryOpenError::UnsupportedOptions=>W::NotSupported,
        DirectoryOpenError::Fields(_)=>W::InvalidParameter,
        DirectoryOpenError::Name(e)=>name_failure(e),
        DirectoryOpenError::FileIsDirectory=>W::FileIsDirectory,
        DirectoryOpenError::NotADirectory=>W::NotADirectory,
        DirectoryOpenError::AccessDenied=>W::AccessDenied,
        DirectoryOpenError::ShareViolation=>W::SharingViolation,
        DirectoryOpenError::DeletePending=>W::DeletePending,
        DirectoryOpenError::Resources=>W::InsufficientResources,
        DirectoryOpenError::Session(e)=>return Err(e),
        DirectoryOpenError::Original(e) if e.kind()==io::ErrorKind::ConnectionAborted=>return Err(Fault::WriteOutcomeUnknown),
        DirectoryOpenError::Original(e)=>named_original_failure(e),
        DirectoryOpenError::WrongTree=>W::NetworkNameDeleted,
    })
}
fn close_failure(why:OpenCloseError)->Result<RootFailure>{
    let why=match why{
        OpenCloseError::Root(v)=>v,
        OpenCloseError::File(v)|OpenCloseError::Writer(v)=>return file_close_failure(v),
        OpenCloseError::Directory(v)=>return directory_close_failure(v),
    };
    Ok(match why{
        RootCloseError::Fields(_)=>RootFailure::InvalidParameter,
        RootCloseError::Session(e)=>return Err(e),
        RootCloseError::Original(e)=>original_failure(e),
        RootCloseError::WrongTree=>RootFailure::NetworkNameDeleted,
    })
}
fn file_close_failure(why:FileCloseError)->Result<RootFailure>{
    Ok(match why{
        FileCloseError::Fields(_)=>RootFailure::InvalidParameter,
        FileCloseError::Session(e)=>return Err(e),
        FileCloseError::Original(e)=>match e.kind(){
            io::ErrorKind::ConnectionAborted=>return Err(Fault::WriteOutcomeUnknown),
            io::ErrorKind::WouldBlock|io::ErrorKind::NotFound=>RootFailure::FileNotAvailable,
            io::ErrorKind::NotConnected=>RootFailure::VolumeDismounted,
            io::ErrorKind::StorageFull=>RootFailure::DiskFull,
            io::ErrorKind::ReadOnlyFilesystem=>RootFailure::MediaWriteProtected,
            _=>original_failure(e),
        },
        FileCloseError::WrongTree=>RootFailure::NetworkNameDeleted,
    })
}
fn directory_close_failure(why:DirectoryCloseError)->Result<RootFailure>{
    Ok(match why{
        DirectoryCloseError::Fields(_)=>RootFailure::InvalidParameter,
        DirectoryCloseError::Session(e)=>return Err(e),
        DirectoryCloseError::Original(e)=>original_failure(e),
        DirectoryCloseError::WrongTree=>RootFailure::NetworkNameDeleted,
    })
}
impl Connection {
    pub(super) fn open_table(&self,sid:NonZeroU64)->Result<&OpenTable>{
        let item=self.sessions.iter().find(|item|item.reg.claim.id==sid).ok_or(Fault::SessionState)?;
        let Phase::Valid(live)=&item.phase else{return Err(Fault::SessionState);};
        Ok(&live.opens)
    }
    fn open_table_mut(&mut self,sid:NonZeroU64)->Result<&mut OpenTable>{
        let item=self.sessions.iter_mut().find(|item|item.reg.claim.id==sid).ok_or(Fault::SessionState)?;
        let Phase::Valid(live)=&mut item.phase else{return Err(Fault::SessionState);};
        Ok(&mut live.opens)
    }
    pub(super) fn member_tables<'a>(&'a mut self,sid:NonZeroU64,revision:&'a BatchRevision)
        ->Result<(SessionView<'a>,&'a TreeTable,&'a mut OpenTable)>{
        let item=self.sessions.iter_mut().find(|item|item.reg.claim.id==sid).ok_or(Fault::SessionState)?;
        let Phase::Valid(live)=&mut item.phase else{return Err(Fault::SessionState);};
        if !live.auth.on_revision(revision){return Err(Fault::SessionState);}
        let scope=SessionView{reg:&item.reg,origin:&self.origin,revision,limits:&self.limits};
        scope.recheck()?;Ok((scope,&live.trees,&mut live.opens))
    }
}

pub(super) struct RequestSlot {pub(super) sid:NonZeroU64,pub(super) tree:u32,pub(super) mid:u64}
impl RequestSlot {
    pub(super) fn matches(&self,member:&VerifiedMember<'_,'_>,sid:NonZeroU64)->bool{
        let h=&member.message().header;
        self.sid==sid&&h.message==self.mid&&h.routing==(Routing::Sync{tree:self.tree})&&member.response_session_matches(sid)
    }
    pub(super) fn root<'a>(&self,c:&'a Connection)->Result<&'a RustRoot>{
        c.tree_table(self.sid)?.root(self.tree).ok_or(Fault::SessionState)
    }
}
/// No public constructor. Body serialization is paired to the same paid
/// member and to the original already-held object.
pub(in crate::smb) struct PendingOpen {request:RequestSlot,scope:RustSessionWitness,slot:OpenSlot,action:CreateAction}
#[repr(u32)]
#[derive(Clone,Copy)]
enum CreateAction{Opened=1,Created=2,Overwritten=3}
impl PendingOpen {
    pub(super) fn existing(request:RequestSlot,scope:RustSessionWitness,slot:OpenSlot)->Self{
        Self{request,scope,slot,action:CreateAction::Opened}
    }
    pub(in crate::smb) fn action(&self)->u32{self.action as u32}
    pub(in crate::smb) fn matches(&self,m:&VerifiedMember<'_,'_>,sid:NonZeroU64)->bool{self.request.matches(m,sid)}
    pub(in crate::smb) fn wire(&self)->WireOpenId{self.slot.wire()}
    pub(in crate::smb) fn response_sizes_and_attributes(&self)->(u64,u64,u32){
        match self.slot.object(){
            Opened::Root(_)|Opened::Directory(_)=>(0,0,DIRECTORY_ATTRIBUTES),
            Opened::File(f)=>(f.logical_allocation(),f.size(),FILE_ATTRIBUTES),
            Opened::Writer(f)=>(f.logical_allocation(),f.size(),FILE_ATTRIBUTES),
            Opened::Retained(r) if r.is_directory()=>(0,0,DIRECTORY_ATTRIBUTES),
            Opened::Retained(r)=>(r.logical_allocation(),r.size(),FILE_ATTRIBUTES),
        }
    }
    pub(in crate::smb) fn is_retained(&self)->bool{matches!(self.slot.object(),Opened::Retained(_))}
    pub(in crate::smb) fn is_read_file(&self)->bool{matches!(self.slot.object(),Opened::File(_))}
    pub(in crate::smb) fn maximal_access_answer(&self,name:CurrentName<'_>)->Option<u32>{
        match (self.slot.object(),name){
            (Opened::Root(r),CurrentName::Root)=>r.maximal_access_answer(),
            (Opened::File(f),CurrentName::Relative(_))=>f.maximal_access_answer(),
            (Opened::Writer(f),CurrentName::Relative(_))=>f.maximal_access_answer(),
            (Opened::Retained(r),CurrentName::Relative(_))=>r.maximal_access_answer(),
            _=>None,
        }
    }
    pub(in crate::smb) fn disk_id_answer(&self,name:CurrentName<'_>)->Option<(u64,u64)>{
        match (self.slot.object(),name){
            (Opened::Root(open),CurrentName::Root)=>open.disk_id_answer(),
            (Opened::File(open),CurrentName::Relative(_))=>open.disk_id_answer(),
            (Opened::Writer(open),CurrentName::Relative(_))=>open.disk_id_answer(),
            _=>None,
        }
    }
    pub(super) fn check(&self,c:&Connection)->Result<()>{
        if !c.open_table(self.request.sid)?.waiting(&self.slot){return Err(Fault::SessionState);}
        self.slot.object().require_current(self.request.root(c)?,&self.scope).map_err(|_|Fault::SessionState)
    }
    pub(super) fn slot(&self)->&OpenSlot{&self.slot}
    pub(super) fn slot_mut(&mut self)->&mut OpenSlot{&mut self.slot}
    pub(super) fn publish(self,c:&mut Connection)->Result<()>{
        c.open_table_mut(self.request.sid)?.publish(self.slot)
    }
}
/// Final response evidence after the exact old route and its hold retired.
/// Can't be used as a new I/O handle or tied to another tree or MID.
pub(in crate::smb) struct ClosedOpen {request:RequestSlot,scope:RustSessionWitness,body:[u8;60],receipt:CloseCompletion}
impl ClosedOpen {
    fn new(request:RequestSlot,scope:RustSessionWitness,receipt:CloseCompletion)->Result<Self>{
        let reply=FileReply::close(&receipt);
        let (body,data)=reply.parts();
        if reply.command()!=CLOSE||reply.status()!=0||!data.is_empty(){return Err(Fault::WrongReply);}
        let body=body.try_into().map_err(|_|Fault::WrongReply)?;
        Ok(Self{request,scope,body,receipt})
    }
    pub(in crate::smb) fn matches(&self,m:&VerifiedMember<'_,'_>,sid:NonZeroU64)->bool{self.request.matches(m,sid)}
    pub(in crate::smb) fn body(&self)->&[u8;60]{&self.body}
    fn check(&self,c:&Connection)->Result<()>{
        self.receipt.require_current(self.request.root(c)?,&self.scope).map_err(|_|Fault::SessionState)
    }
}
impl SyncReply<'_>{
    fn send_open(self,open:PendingOpen)->Result<()>{
        open.check(self.tx.plan.owner)?;
        let SyncReply{tx,ids,revision,count}=self.write_original()?.0;
        tx.publish_with(|c|{
            current(c,ids,revision,count)?;open.check(c)?;
            c.open_table_mut(open.request.sid)?.publish(open.slot)
        })
    }
    fn send_closed(self,closed:ClosedOpen)->Result<()>{
        closed.check(self.tx.plan.owner)?;
        let SyncReply{tx,ids,revision,count}=self.write_original()?.0;
        tx.publish_with(|c|{current(c,ids,revision,count)?;closed.check(c)})
    }
}

#[cfg(test)]
#[path="../../../../../../tests/smb/root_io.rs"]
mod tests;
