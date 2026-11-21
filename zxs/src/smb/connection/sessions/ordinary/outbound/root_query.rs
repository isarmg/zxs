//! A paid, signed standalone QUERY_DIRECTORY and the exact original
//! same folder's cursor advancement after the whole original signed write.
use super::{CheckedBatch,Connection,Fault,Payload,Result,SyncReply,UnsignedSmall,VerifiedMember,current,
    root_io::RequestSlot};
use crate::smb::{backend::{PreparedOrdinaryQuery,PreparedRetainedQuery,PreparedRootQuery,RootQueryError},session::BatchRevision,
    wire::{direct::MAX_FRAME_BYTES,io_fields::{IoFieldError,OpenReference,WireOpenId},message::{Message,RELATED,Routing},
        query_fields::QueryError,query_reply::{DIRECTORY_COMMAND,DirectoryFailure,DirectoryReply,UnsignedDirectory}}};
use std::{io,num::NonZeroU64};
use super::super::opens::Opened;
pub(in crate::smb) mod related;

impl CheckedBatch<'_,'_>{
    pub(in crate::smb) fn try_send_root_query(&mut self)->Result<bool>{
        self.before_reply()?;
        let frame=self.original.frame;
        let [m]=frame.messages() else{return Ok(false);};
        let Routing::Sync{tree:tid}=m.header.routing else{return Ok(false);};
        if m.header.flags&RELATED!=0||m.header.command!=DIRECTORY_COMMAND{return Ok(false);}
        let sid=self.ids.at(0).ok_or(Fault::SessionState)?;
        let original=RequestSlot{sid,tree:tid,mid:m.header.message};
        let payload=self.original.payload(0).ok_or(Fault::SessionState)?;
        let draft={
            let (session,trees,opens)=self.owner.member_tables(sid,&self.revision)?;
            // §3.3.5.18: Session, Tree, then the original OPEN in that
            // session. No columns or flags create a search before this.
            let Some(tree)=trees.root(tid) else{
                return self.directory_refusal(m,sid,DirectoryFailure::NetworkNameDeleted);
            };
            let wire=match m.directory_target(){
                Ok(OpenReference::Explicit(wire))=>wire,
                Err(IoFieldError::NoExplicitOpen)=>return self.directory_refusal(m,sid,DirectoryFailure::FileClosed),
                _=>return self.directory_refusal(m,sid,DirectoryFailure::InvalidParameter),
            };
            let Some(slot)=opens.find_mut(wire).filter(|slot|slot.tree()==tid) else{
                return self.directory_refusal(m,sid,DirectoryFailure::FileClosed);
            };
            match payload {
                Payload::Covered=>{},
                Payload::InvalidParameter=>return self.directory_refusal(m,sid,DirectoryFailure::InvalidParameter),
                _=>return Err(Fault::SessionState),
            }
            let member=VerifiedMember{message:m,ordinal:0,session};
            // One standalone SMB header + the 8 fixed bytes are already
            // reserved. The request's exact output cap remains independent.
            let room=(u64::from(member.max_transact())+256).min(MAX_FRAME_BYTES as u64) as usize;
            let room=room.checked_sub(72).ok_or(Fault::FrameSize)?;
            let prepared=match slot.object_mut(){
                Opened::Root(open)=>open.prepare_query(tree,&member,room).map(QueryPrepared::Root),
                Opened::Directory(open)=>open.prepare_query(tree,&member,room).map(QueryPrepared::Ordinary),
                Opened::Retained(open) if open.is_directory()=>open.prepare_query(tree,&member,room).map(QueryPrepared::Retained),
                Opened::File(_)|Opened::Writer(_)|Opened::Retained(_)=>return self.directory_refusal(m,sid,DirectoryFailure::InvalidParameter),
            };
            let prepared=match prepared{
                Ok(p)=>p,
                Err(e)=>return self.directory_refusal(m,sid,query_failure(e)?),
            };
            UnsignedDirectory::new(&member,PendingRootQuery{original,wire,prepared}).map_err(|_|Fault::WrongReply)?
        };
        self.send_directory(draft)?;Ok(true)
    }

    fn directory_refusal(&mut self,m:&Message<'_>,sid:NonZeroU64,why:DirectoryFailure)->Result<bool>{
        // Recheck the original signed scope, but never mint an execution
        // ticket for a member with a short/invalid payload credit charge.
        self.owner.member_scope(sid,&self.revision)?;
        let draft=UnsignedSmall::directory_failure(&DirectoryRefusal{message:m,sid,why}).map_err(|_|Fault::WrongReply)?;
        self.next=1;self.small_replies(std::iter::once(draft))?.send()?;Ok(true)
    }

    fn send_directory(&mut self,draft:UnsignedDirectory)->Result<()>{
        self.next=1;self.halted=true;
        let ids=&self.ids;let revision=&self.revision;
        let plan=self.owner.plan_debit(&mut self.original.debit)?;
        let payable=draft.credit(plan.credit_response(0).ok_or(Fault::NoReply)?);
        let sid=payable.sid();
        if ids.at(0)!=Some(sid){return Err(Fault::SessionState);}
        plan.owner.member_scope(sid,revision)?;
        let (_,bound)=plan.owner.valid_slot(sid).ok_or(Fault::SessionState)?;
        let (record,pending)=bound.sign_directory(revision,payable).map_err(|_|Fault::SessionState)?.into_send();
        let tx=plan.bind_record(record)?;
        SyncReply{tx,ids,revision,count:1}.send_root_query(pending)
    }
}

fn query_failure(e:RootQueryError)->Result<DirectoryFailure>{
    use DirectoryFailure as W;
    Ok(match e {
        RootQueryError::Session(e)=>return Err(e),
        RootQueryError::Pending=>return Err(Fault::SessionState), // no outstanding page in the synchronous owner
        RootQueryError::Fields(QueryError::InvalidInfoClass)=>W::InvalidInfoClass,
        RootQueryError::Fields(QueryError::NotSupported)=>W::NotSupported,
        RootQueryError::Fields(_)=>W::InvalidParameter,
        RootQueryError::AccessDenied=>W::AccessDenied,
        RootQueryError::InvalidPattern=>W::ObjectNameInvalid,
        RootQueryError::BufferTooSmall=>W::InfoLengthMismatch,
        RootQueryError::FirstRecordOverflow=>W::BufferOverflow,
        RootQueryError::WrongTree=>W::NetworkNameDeleted,
        RootQueryError::Resources=>W::Resources,
        RootQueryError::Original(e)=>match e.kind(){
            io::ErrorKind::PermissionDenied=>W::AccessDenied,
            io::ErrorKind::OutOfMemory=>W::Resources,
            io::ErrorKind::NotConnected=>W::VolumeDismounted,
            io::ErrorKind::WouldBlock=>W::FileNotAvailable,
            _=>W::ServerUnavailable,
        },
    })
}

/// A refusal belongs to the original paid MID and whole-frame CMAC.
/// No constructor takes a foreign packet or turns this into a member.
pub(in crate::smb) struct DirectoryRefusal<'a,'r>{message:&'a Message<'r>,sid:NonZeroU64,why:DirectoryFailure}
impl<'a,'r> DirectoryRefusal<'a,'r>{
    pub(in crate::smb) fn fields(&self)->(&'a Message<'r>,NonZeroU64,DirectoryFailure){(self.message,self.sid,self.why)}
}

enum QueryPrepared{Root(PreparedRootQuery),Ordinary(PreparedOrdinaryQuery),Retained(PreparedRetainedQuery)}
impl QueryPrepared{
    fn require_on(&self,open:&Opened,tree:&super::RustRoot)->Result<()>{
        match (self,open){
            (Self::Root(p),Opened::Root(open))=>p.require_current(open,tree),
            (Self::Ordinary(p),Opened::Directory(open))=>p.require_current(open,tree),
            (Self::Retained(p),Opened::Retained(open))=>p.require_current(open,tree),
            _=>return Err(Fault::SessionState),
        }.map_err(|_|Fault::SessionState)
    }
    // Called only by original sender completion on an original OPEN.
    fn apply_written(self,open:&mut Opened,tree:&super::RustRoot)->Result<()>{
        match (self,open){
            (Self::Root(p),Opened::Root(open))=>open.apply_written_query(tree,WrittenRootQuery(p)),
            (Self::Ordinary(p),Opened::Directory(open))=>open.apply_written_query(tree,WrittenOrdinaryQuery(p)),
            (Self::Retained(p),Opened::Retained(open))=>open.apply_written_query(tree,WrittenRetainedQuery(p)),
            _=>return Err(Fault::SessionState),
        }.map_err(|_|Fault::SessionState)
    }
}

pub(in crate::smb) struct PendingRootQuery{original:RequestSlot,wire:WireOpenId,prepared:QueryPrepared}
impl PendingRootQuery {
    pub(in crate::smb) fn sid(&self)->NonZeroU64{self.original.sid}
    pub(in crate::smb) fn matches(&self,m:&VerifiedMember<'_,'_>)->bool{self.original.matches(m,self.sid())}
    pub(in crate::smb) fn reply(&self)->&DirectoryReply{
        match &self.prepared{
            QueryPrepared::Root(p)=>p.reply(),QueryPrepared::Ordinary(p)=>p.reply(),QueryPrepared::Retained(p)=>p.reply(),
        }
    }
    fn check(&self,c:&Connection)->Result<()>{
        let tree=self.original.root(c)?;
        let open=c.open_table(self.sid())?.find(self.wire).filter(|slot|slot.tree()==self.original.tree)
            .ok_or(Fault::SessionState)?;
        self.prepared.require_on(open.object(),tree)
    }
    fn publish(self,c:&mut Connection,revision:&BatchRevision)->Result<()>{
        let (_session,trees,opens)=c.member_tables(self.sid(),revision)?;
        let tree=trees.root(self.original.tree).ok_or(Fault::SessionState)?;
        let open=opens.find_mut(self.wire).filter(|slot|slot.tree()==self.original.tree).ok_or(Fault::SessionState)?;
        // Only after this exact immutable signed write may the owner mint
        // the typed receipt and commit its same actual search.
        self.prepared.apply_written(open.object_mut(),tree)
    }
}

#[must_use]
pub(in crate::smb) struct WrittenRootQuery(PreparedRootQuery);
impl WrittenRootQuery {
    pub(in crate::smb) fn into_prepared(self)->PreparedRootQuery{self.0}
}
#[must_use]
pub(in crate::smb) struct WrittenOrdinaryQuery(PreparedOrdinaryQuery);
impl WrittenOrdinaryQuery {
    pub(in crate::smb) fn into_prepared(self)->PreparedOrdinaryQuery{self.0}
}
#[must_use]
pub(in crate::smb) struct WrittenRetainedQuery(PreparedRetainedQuery);
impl WrittenRetainedQuery {
    pub(in crate::smb) fn into_prepared(self)->PreparedRetainedQuery{self.0}
}
impl SyncReply<'_>{
    fn send_root_query(self,pending:PendingRootQuery)->Result<()>{
        pending.check(self.tx.plan.owner)?;
        let SyncReply{tx,ids,revision,count}=self.write_original()?.0;
        tx.publish_with(|c|{
            current(c,ids,revision,count)?;
            pending.publish(c,revision)
        })
    }
}
