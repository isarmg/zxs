//! Paid signed FILE chains: actual ordinary rename or retained restore,
//! and an ordinary FILE's mark-for-delete. All use an explicit CLOSE.
use super::*;
use super::super::file_write::{disposition_failure,set_failure};
use super::super::restore::{ordinary_rename_failure,parse_failure,restore_failure};
use crate::smb::{
    backend::{PreparedOrdinaryRename,PreparedRetainedRestore,RetiredOrdinaryRename,RetiredRetainedRestore,path::TreeName},
    wire::{create_fields::Disposition,fixed_reply::{RestoreFailure,WriteFailure},message::Compound,
        set_info::{Change,FileSetRequest,RenameRequest}},
};

const PARTS:usize=3;
const FILE_DIRECTORY_FILE:u32=0x0000_0001;
const FILE_DELETE_ON_CLOSE:u32=0x0000_1000;
const DELETE:u32=0x0001_0000;
const READ_AND_DELETE:u32=0x0011_00a1;
#[derive(Clone,Copy)]
enum Action{Rename,Disposition}
impl Action{
    fn selects(self,mid:&Message<'_>)->bool{
        match self{Self::Rename=>mid.is_basic_rename_request(),Self::Disposition=>mid.is_basic_disposition_request()}
    }
}
pub(in crate::smb) struct FileActionPart<'a,'r>{
    raw:&'a Message<'r>,sid:NonZeroU64,tree:u32,ordinal:usize,action:Action,
}
impl<'a,'r> FileActionPart<'a,'r>{
    pub(in crate::smb) fn fields(&self)->(&'a Message<'r>,NonZeroU64,u32,usize){
        (self.raw,self.sid,self.tree,self.ordinal)
    }
    pub(in crate::smb) fn is_rename(&self)->bool{matches!(self.action,Action::Rename)}
    pub(in crate::smb) fn selects_set(&self)->bool{self.action.selects(self.raw)}
    pub(in crate::smb) fn matches(&self,member:&VerifiedMember<'_,'_>)->bool{
        self.ordinal==member.ordinal()&&std::ptr::eq(self.raw,member.message())&&member.response_session_matches(self.sid)
    }
    fn member<'s>(&'s self,scope:&'s SessionView<'_>)->Result<VerifiedMember<'s,'r>>{
        scope.recheck()?;
        if scope.reg.claim.id!=self.sid{return Err(Fault::SessionState);}
        Ok(VerifiedMember{message:self.raw,ordinal:self.ordinal,session:SessionView{
            reg:scope.reg,origin:scope.origin,revision:scope.revision,limits:scope.limits,
        }})
    }
}

impl CheckedBatch<'_,'_>{
    fn file_action_parts<'b,'r>(&self,frame:&'b Compound<'r>,action:Action)
        ->Result<Option<[FileActionPart<'b,'r>;PARTS]>>{
        let [start,set,finish]=frame.messages()else{return Ok(None);};
        let Routing::Sync{tree:tid}=start.header.routing else{return Ok(None);};
        if start.header.command!=CREATE||!action.selects(set)||finish.header.command!=CLOSE||
           start.header.flags&RELATED!=0||set.header.flags&RELATED==0||finish.header.flags&RELATED==0||
           !matches!(set.header.routing,Routing::Sync{..})||!matches!(finish.header.routing,Routing::Sync{..})||
           !start.create_name_fields().is_ok_and(|v|!v.name.is_empty())||
           !finish.close_request().is_ok_and(|c|c.open==OpenReference::PreviousInCompound){
            return Ok(None);
        }
        if self.original.payload(0)!=Some(Payload::NoVariableRule)||
           self.original.payload(1)!=Some(Payload::Covered)||
           self.original.payload(2)!=Some(Payload::NoVariableRule){return Ok(None);}
        let sid=self.ids.at(0).ok_or(Fault::SessionState)?;
        if self.ids.at(1)!=Some(sid)||self.ids.at(2)!=Some(sid){return Err(Fault::SessionState);}
        let raw=[start,set,finish];
        Ok(Some(std::array::from_fn(|ordinal|FileActionPart{raw:raw[ordinal],sid,tree:tid,ordinal,action})))
    }
    pub(in crate::smb) fn try_send_related_rename(&mut self)->Result<bool>{
        self.before_reply()?;
        let frame=self.original.frame;
        let Some(parts)=self.file_action_parts(frame,Action::Rename)?else{return Ok(false);};
        let (start,sid,tid,_)=parts[0].fields();
        let share_modes=Arc::clone(&self.owner.origin.run.share_modes);
        let global=Arc::clone(&self.owner.origin.run.persistent);
        let (drafts,tail)={
            let (scope,trees,opens)=self.owner.member_tables(sid,&self.revision)?;
            let Some(tree)=trees.root(tid)else{return self.related_create_refusal(&parts,RootFailure::NetworkNameDeleted);};
            let create=parts[0].member(&scope)?;
            let opened=match share_modes.prepare_retained_open(tree,&create){
                Ok(Some(open))=>Opened::Retained(open),
                // None is the non-R name class, never fallback after an
                // actual R selection/authority error.
                Ok(None)=>{
                    let fields=match start.create_fields(){
                        Ok(v)=>v,Err(_)=>return self.related_create_refusal(&parts,RootFailure::InvalidParameter),
                    };
                    let access=fields.desired_access();
                    if fields.disposition()!=Disposition::Open||
                       fields.effective_options()&(FILE_DIRECTORY_FILE|FILE_DELETE_ON_CLOSE)!=0||
                       access&DELETE==0||access&!READ_AND_DELETE!=0{return Ok(false);}
                    // No new disk-ID disclosure after the held FILE is
                    // retired by this compound. Standalone CREATE rechecks it.
                    if fields.current_intent().is_ok_and(|intent|intent.disk_id()){
                        return self.related_create_refusal(&parts,RootFailure::NotSupported);
                    }
                    match share_modes.prepare_directory_open(tree,&create){
                        Ok(None)=>{}, // confirmed ordinary FILE kind
                        Ok(Some(_))=>return Ok(false), // never rename a directory here
                        Err(e)=>return self.related_create_refusal(&parts,directory_open_failure(e)?),
                    }
                    match share_modes.prepare_file_open(tree,&create){
                        Ok(PreparedNamedFile::Read(file))=>Opened::File(file),
                        Ok(PreparedNamedFile::Write(_))=>return Err(Fault::SessionState),
                        Err(e)=>return self.related_create_refusal(&parts,file_open_failure(e)?),
                    }
                },
                Err(e)=>return self.related_create_refusal(&parts,directory_open_failure(e)?),
            };
            let created=create.rust_session_witness()?;
            let slot=match opens.reserve(&global,tid,opened){
                Ok(v)=>v,
                Err(Fault::Resources|Fault::Exhausted)=>return self.related_create_refusal(&parts,RootFailure::InsufficientResources),
                Err(e)=>return Err(e),
            };
            let mut pending=PendingOpen::existing(RequestSlot{sid,tree:tid,mid:start.header.message},created,slot);
            let first=UnsignedSmall::rename_opened(&parts[0],&create,&pending).map_err(|_|Fault::WrongReply)?;
            let set=parts[1].member(&scope)?;
            let close=parts[2].member(&scope)?;
            let original_set=set.rust_session_witness()?;
            let renamed=match RenameRequest::parse(set){
                Ok(request)=>{
                    if request.open()!=OpenReference::PreviousInCompound{return Err(Fault::WrongReply);}
                    match pending.slot_mut().object_mut(){
                        Opened::Retained(open)=>match open.restore(tree,&request){
                            Ok(actual)=>Ok((UnsignedSmall::restore_related_done(&parts[1],&request,&actual)
                                .map_err(|_|Fault::WrongReply)?,RenameDone::Restore(actual))),
                            Err(e)=>Err(restore_failure(e)?),
                        },
                        Opened::File(open)=>match open.rename(tree,&request){
                            Ok(actual)=>Ok((UnsignedSmall::ordinary_rename_related_done(&parts[1],&request,&actual)
                                .map_err(|_|Fault::WrongReply)?,RenameDone::File(actual))),
                            Err(e)=>Err(ordinary_rename_failure(e)?),
                        },
                        _=>return Err(Fault::SessionState),
                    }
                },
                Err((_,e))=>Err(parse_failure(e)?),
            };
            match renamed{
                Ok((second,done))=>{
                    let PendingOpen{request,scope,slot,..}=pending;
                    let (completion,retired)=match done{
                        RenameDone::Restore(actual)=>{
                            let (closed,retired)=opens.retire_retained_restore(slot,tree,actual,&close)?;
                            (closed,RetiredRename::Restore(retired))
                        },
                        RenameDone::File(actual)=>{
                            let (closed,retired)=opens.retire_ordinary_rename(slot,tree,actual,&close)?;
                            (closed,RetiredRename::File(retired))
                        },
                    };
                    let third=UnsignedSmall::file_action_closed(&parts[2],&close,&completion).map_err(|_|Fault::WrongReply)?;
                    ([first,second,third],Tail::Closed(ClosedRename{request,scope,retired}))
                },
                Err(error)=>{
                    let failed=UnsignedSmall::rename_related_failure(&parts[1],error).map_err(|_|Fault::WrongReply)?;
                    let blocked=UnsignedSmall::rename_close_blocked(&parts[2],error).map_err(|_|Fault::WrongReply)?;
                    ([first,failed,blocked],Tail::Blocked(BlockedFileAction{open:pending,set:original_set}))
                },
            }
        };
        self.next=PARTS;
        let reply=self.small_replies(drafts.into_iter())?;
        match tail{
            Tail::Closed(v)=>reply.send_closed_rename(v)?,
            Tail::Blocked(v)=>reply.send_blocked_file_action(v)?,
        }
        Ok(true)
    }

    pub(in crate::smb) fn try_send_related_disposition(&mut self)->Result<bool>{
        self.before_reply()?;
        let frame=self.original.frame;
        let Some(parts)=self.file_action_parts(frame,Action::Disposition)?else{return Ok(false);};
        let (start,sid,tid,_)=parts[0].fields();
        let share_modes=Arc::clone(&self.owner.origin.run.share_modes);
        let global=Arc::clone(&self.owner.origin.run.persistent);
        let (drafts,tail)={
            let (scope,trees,opens)=self.owner.member_tables(sid,&self.revision)?;
            let Some(tree)=trees.root(tid)else{return self.related_create_refusal(&parts,RootFailure::NetworkNameDeleted);};
            let create=parts[0].member(&scope)?;
            let fields=match start.create_fields(){
                Ok(v)=>v,Err(_)=>return self.related_create_refusal(&parts,RootFailure::InvalidParameter),
            };
            // Only a normal existing FILE opened to mark it, not a new
            // FILE, writer, directory or CREATE FILE_DELETE_ON_CLOSE.
            // The applicable client's exclusive request is not weakened.
            let desired=fields.desired_access();
            if fields.disposition()!=Disposition::Open||fields.effective_options()&0x1000!=0||
               fields.share_access()!=0||desired&0x0001_0000==0||desired&!0x0011_00a1!=0{
                return self.related_create_refusal(&parts,RootFailure::NotSupported);
            }
            let raw=match fields.current_intent(){
                Ok(intent) if !intent.disk_id()=>match intent.name(){
                    CurrentName::Relative(raw)=>raw,
                    _=>return self.related_create_refusal(&parts,RootFailure::NotSupported),
                },
                _=>return self.related_create_refusal(&parts,RootFailure::NotSupported),
            };
            let name=match TreeName::create_relative(raw){
                Ok(v)=>v,Err(e)=>return self.related_create_refusal(&parts,name_failure(e)),
            };
            if name.components().next().is_none_or(|label|tree.names_retained_role(label)){
                return self.related_create_refusal(&parts,RootFailure::AccessDenied);
            }
            match share_modes.prepare_directory_open(tree,&create){
                Ok(None)=>{}, // only actual Core FILE kind enters this lane
                Ok(Some(_))=>return self.related_create_refusal(&parts,RootFailure::NotSupported),
                Err(e)=>return self.related_create_refusal(&parts,directory_open_failure(e)?),
            }
            let file=match share_modes.prepare_file_open(tree,&create){
                Ok(PreparedNamedFile::Read(file))=>file,
                Ok(PreparedNamedFile::Write(_))=>return Err(Fault::SessionState),
                Err(e)=>return self.related_create_refusal(&parts,file_open_failure(e)?),
            };
            let created=create.rust_session_witness()?;
            let slot=match opens.reserve(&global,tid,Opened::File(file)){
                Ok(v)=>v,
                Err(Fault::Resources|Fault::Exhausted)=>return self.related_create_refusal(&parts,RootFailure::InsufficientResources),
                Err(e)=>return Err(e),
            };
            let pending=PendingOpen::existing(RequestSlot{sid,tree:tid,mid:start.header.message},created,slot);
            pending.slot.object().require_current(tree,&pending.scope).map_err(|_|Fault::SessionState)?;
            let first=UnsignedSmall::disposition_opened(&parts[0],&create,&pending).map_err(|_|Fault::WrongReply)?;
            let Opened::File(open)=pending.slot.object()else{return Err(Fault::SessionState);};
            let set=parts[1].member(&scope)?;
            let close=parts[2].member(&scope)?;
            let original_set=set.rust_session_witness()?;
            let marked=match FileSetRequest::parse(set){
                Ok(request)=>{
                    if request.open()!=OpenReference::PreviousInCompound{return Err(Fault::WrongReply);}
                    if request.change()!=Change::MarkForDelete||request.input_length()!=1{
                        Err(WriteFailure::InvalidParameter)
                    }else{
                        match open.set_disposition(tree,&request){
                            Ok(mark)=>{
                                let second=UnsignedSmall::disposition_related_done(&parts[1],&request,&mark)
                                    .map_err(|_|Fault::WrongReply)?;
                                Ok((second,mark))
                            },
                            Err(e)=>Err(disposition_failure(e)?),
                        }
                    }
                },
                Err((_,e))=>Err(set_failure(e)?),
            };
            match marked{
                Ok((second,mark))=>{
                    let closing=close.rust_session_witness()?;
                    let PendingOpen{request,scope,slot,..}=pending;
                    let result=opens.close_marked_file(slot,tree,mark,&close)?;
                    let (third,confirmed)=match result{
                        Ok(receipt)=>{
                            let done=UnsignedSmall::file_action_closed(&parts[2],&close,&receipt)
                                .map_err(|_|Fault::WrongReply)?;
                            (done,Some(receipt))
                        },
                        Err(e)=>{
                            let real=UnsignedSmall::disposition_close_failure(&parts[2],file_close_failure(e)?)
                                .map_err(|_|Fault::WrongReply)?;
                            (real,None)
                        },
                    };
                    ([first,second,third],DispositionTail::Closed(ClosedDisposition{
                        request,create:scope,set:original_set,close:closing,confirmed,
                    }))
                },
                Err(error)=>{
                    let second=UnsignedSmall::disposition_related_failure(&parts[1],error).map_err(|_|Fault::WrongReply)?;
                    let third=UnsignedSmall::disposition_close_blocked(&parts[2],error).map_err(|_|Fault::WrongReply)?;
                    ([first,second,third],DispositionTail::Blocked(BlockedFileAction{open:pending,set:original_set}))
                },
            }
        };
        self.next=PARTS;
        let reply=self.small_replies(drafts.into_iter())?;
        match tail{
            DispositionTail::Closed(v)=>reply.send_closed_disposition(v)?,
            DispositionTail::Blocked(v)=>reply.send_blocked_file_action(v)?,
        }
        Ok(true)
    }
    fn related_create_refusal(&mut self,parts:&[FileActionPart<'_,'_>;PARTS],why:RootFailure)->Result<bool>{
        let drafts=[
            UnsignedSmall::file_action_create_refusal(&parts[0],why).map_err(|_|Fault::WrongReply)?,
            UnsignedSmall::file_action_without_open(&parts[1]).map_err(|_|Fault::WrongReply)?,
            UnsignedSmall::file_action_without_open(&parts[2]).map_err(|_|Fault::WrongReply)?,
        ];
        self.next=PARTS;self.small_replies(drafts.into_iter())?.send()?;Ok(true)
    }
}

enum RenameDone{Restore(PreparedRetainedRestore),File(PreparedOrdinaryRename)}
enum RetiredRename{Restore(RetiredRetainedRestore),File(RetiredOrdinaryRename)}
enum Tail{Closed(ClosedRename),Blocked(BlockedFileAction)}
enum DispositionTail{Closed(ClosedDisposition),Blocked(BlockedFileAction)}
struct ClosedDisposition{
    request:RequestSlot,create:RustSessionWitness,set:RustSessionWitness,close:RustSessionWitness,
    confirmed:Option<CloseCompletion>,
}
impl ClosedDisposition{
    fn check(&self,c:&Connection)->Result<()>{
        let tree=self.request.root(c)?;
        tree.ensure_current(&self.create).map_err(|_|Fault::SessionState)?;
        tree.ensure_current(&self.set).map_err(|_|Fault::SessionState)?;
        if let Some(receipt)=&self.confirmed{
            receipt.require_current(tree,&self.close).map_err(|_|Fault::SessionState)
        }else{
            tree.ensure_current(&self.close).map_err(|_|Fault::SessionState)
        }
    }
}
struct ClosedRename{request:RequestSlot,scope:RustSessionWitness,retired:RetiredRename}
impl ClosedRename{
    fn check(&self,c:&Connection)->Result<()>{
        let tree=self.request.root(c)?;
        tree.ensure_current(&self.scope).map_err(|_|Fault::SessionState)?;
        match &self.retired{
            RetiredRename::Restore(v)=>v.require_current(tree),
            RetiredRename::File(v)=>v.require_current(tree),
        }.map_err(|_|Fault::SessionState)
    }
}
struct BlockedFileAction{open:PendingOpen,set:RustSessionWitness}
impl BlockedFileAction{
    fn check(&self,c:&Connection)->Result<()>{
        self.open.check(c)?;
        self.open.request.root(c)?.ensure_current(&self.set).map_err(|_|Fault::SessionState)
    }
}
impl SyncReply<'_>{
    fn send_closed_disposition(self,done:ClosedDisposition)->Result<()>{
        done.check(self.tx.plan.owner)?;
        let SyncReply{tx,ids,revision,count}=self.write_original()?.0;
        tx.publish_with(|c|{current(c,ids,revision,count)?;done.check(c)})
    }
    fn send_closed_rename(self,done:ClosedRename)->Result<()>{
        done.check(self.tx.plan.owner)?;
        let SyncReply{tx,ids,revision,count}=self.write_original()?.0;
        tx.publish_with(|c|{current(c,ids,revision,count)?;done.check(c)})
    }
    fn send_blocked_file_action(self,failed:BlockedFileAction)->Result<()>{
        // The real CREATE was successful, but SET and dependent CLOSE
        // were not. Only advertise this OPEN if it is genuinely still
        // owned and can be closed by a later request on this same session.
        failed.check(self.tx.plan.owner)?;
        let SyncReply{tx,ids,revision,count}=self.write_original()?.0;
        tx.publish_with(|c|{
            current(c,ids,revision,count)?;failed.check(c)?;
            c.open_table_mut(failed.open.request.sid)?.publish(failed.open.slot)
        })
    }
}
