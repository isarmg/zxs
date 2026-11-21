//! Bounded related probes on the actual logical root. A failed query may
//! leave exactly that real OPEN; a successful CLOSE always consumes it.
use super::*;
use crate::smb::{
    backend::{PreparedRootFsSize,RetiredRootFs,RetiredRootFsSize},
    wire::{fixed_reply::FsInfoFailure,query_fields::{FileClass,FsClass,InfoClass}},
};

use super::super::root_attributes::root_failure;

const QUERY_INFO:u16=16;
const PARTS:usize=3;
enum Query{Attributes,Size,FileAll}

/// A particular original member of this specific verified related chain.
/// The effective identifiers come from its first member, not later bytes.
pub(in crate::smb) struct ProbePart<'a,'r>{
    raw:&'a Message<'r>,sid:NonZeroU64,tree:u32,ordinal:usize,count:usize,
}
impl<'a,'r> ProbePart<'a,'r>{
    pub(in crate::smb) fn fields(&self)->(&'a Message<'r>,NonZeroU64,u32,usize){
        (self.raw,self.sid,self.tree,self.ordinal)
    }
    pub(in crate::smb) fn matches(&self,member:&VerifiedMember<'_,'_>)->bool{
        self.ordinal==member.ordinal()&&std::ptr::eq(self.raw,member.message())&&member.response_session_matches(self.sid)
    }
    pub(in crate::smb) fn reply_shape(&self)->Option<(u16,bool)>{
        match (self.count,self.ordinal){
            (2|3,0)=>Some((CREATE,true)),
            (3,1)=>Some((QUERY_INFO,true)),
            (2,1)|(3,2)=>Some((CLOSE,false)),
            _=>None,
        }
    }
    // Reborrow the actual original Session and exact same already-paid
    // segment. The only producer is the specific checked chain below.
    fn member<'s>(&'s self,scope:&'s SessionView<'_>)->Result<VerifiedMember<'s,'r>>{
        scope.recheck()?;
        if scope.reg.claim.id!=self.sid{return Err(Fault::SessionState);}
        Ok(VerifiedMember{message:self.raw,ordinal:self.ordinal,session:SessionView{
            reg:scope.reg,origin:scope.origin,revision:scope.revision,limits:scope.limits,
        }})
    }
}

impl CheckedBatch<'_,'_>{
    pub(in crate::smb) fn try_send_root_probe(&mut self)->Result<bool>{
        self.before_reply()?;
        let frame=self.original.frame;
        let [start,query,finish]=frame.messages() else{return Ok(false);};
        let Routing::Sync{tree:tid}=start.header.routing else{return Ok(false);};
        if start.header.command!=CREATE||query.header.command!=QUERY_INFO||finish.header.command!=CLOSE||
           start.header.flags&RELATED!=0||query.header.flags&RELATED==0||finish.header.flags&RELATED==0||
           !matches!(query.header.routing,Routing::Sync{..})||!matches!(finish.header.routing,Routing::Sync{..})||
           !start.create_name_fields().is_ok_and(|v|v.name.is_empty()){
            return Ok(false);
        }
        // A bounded synchronous probe. No partial creation/side effect
        // for an undercharged, wrong-class or malformed subrequest.
        if self.original.payload(0)!=Some(Payload::NoVariableRule)||
           self.original.payload(1)!=Some(Payload::Covered)||
           self.original.payload(2)!=Some(Payload::NoVariableRule)||
           !finish.close_request().is_ok_and(|c|c.open==OpenReference::PreviousInCompound){
            return Ok(false);
        }
        let family=match query.query_info(self.owner.limits.transact){
            Ok(q) if q.class==InfoClass::Filesystem(FsClass::Size)=>Query::Size,
            Ok(q) if q.class==InfoClass::Filesystem(FsClass::Attribute)&&q.output_bytes>=12=>Query::Attributes,
            Ok(q) if q.class==InfoClass::File(FileClass::All)=>Query::FileAll,
            _=>return Ok(false),
        };
        let sid=self.ids.at(0).ok_or(Fault::SessionState)?;
        if self.ids.at(1)!=Some(sid)||self.ids.at(2)!=Some(sid){return Err(Fault::SessionState);}
        let parts=[
            ProbePart{raw:start,sid,tree:tid,ordinal:0,count:PARTS},
            ProbePart{raw:query,sid,tree:tid,ordinal:1,count:PARTS},
            ProbePart{raw:finish,sid,tree:tid,ordinal:2,count:PARTS},
        ];
        let share_modes=Arc::clone(&self.owner.origin.run.share_modes);
        let global=Arc::clone(&self.owner.origin.run.persistent);
        let (drafts,terminal)={
            let (scope,trees,opens)=self.owner.member_tables(sid,&self.revision)?;
            let Some(tree)=trees.root(tid)else{return self.probe_refusal(&parts,RootFailure::NetworkNameDeleted);};
            let create=parts[0].member(&scope)?;
            let opened=match share_modes.prepare_root_open(tree,&create){
                Ok(v)=>v,Err(e)=>return self.probe_refusal(&parts,open_failure(e)?),
            };
            let stamp=create.rust_session_witness()?;
            let slot=match opens.reserve(&global,tid,Opened::Root(opened)){
                Ok(v)=>v,
                Err(Fault::Resources|Fault::Exhausted)=>return self.probe_refusal(&parts,RootFailure::InsufficientResources),
                Err(e)=>return Err(e),
            };
            let pending=PendingOpen::existing(RequestSlot{sid,tree:tid,mid:start.header.message},stamp,slot);
            let first=UnsignedSmall::probe_opened(&parts[0],&create,&pending).map_err(|_|Fault::WrongReply)?;
            let Opened::Root(open)=pending.slot.object()else{return Err(Fault::SessionState);};
            let query=parts[1].member(&scope)?;
            // No session/Tree/FileId literal in this member is consulted
            // as a selector. Only the immediately preceding real OPEN.
            let close=parts[2].member(&scope)?;
            let (second,third,terminal)=match family{
                Query::Attributes=>{
                    let output=open.prepare_fs_attributes(tree,&query).map_err(|_|Fault::SessionState)?;
                    let second=UnsignedSmall::probe_fs_attributes(&parts[1],&query,&output).map_err(|_|Fault::WrongReply)?;
                    let PendingOpen{request,scope:created,slot,..}=pending;
                    let (receipt,retired)=opens.retire_root_probe(slot,tree,output,&close)?;
                    let third=UnsignedSmall::probe_closed(&parts[2],&close,receipt).map_err(|_|Fault::WrongReply)?;
                    (second,third,Tail::Closed(CompletedProbe{request,scope:created,retired:Retired::Attributes(retired)}))
                },
                Query::Size=>match open.prepare_fs_size(tree,&query){
                    Ok(size)=>{
                        let (second,failure)=UnsignedSmall::probe_fs_size(&parts[1],&query,&size).map_err(|_|Fault::WrongReply)?;
                        if let Some(status)=failure{
                            let third=refuse_close(&parts[2],status)?;
                            (second,third,Tail::Blocked(BlockedProbe{open:pending,proof:Blocked::Size(size)}))
                        }else{
                            let PendingOpen{request,scope:created,slot,..}=pending;
                            let (receipt,retired)=opens.retire_root_size_probe(slot,tree,size,&close)?;
                            let third=UnsignedSmall::probe_closed(&parts[2],&close,receipt).map_err(|_|Fault::WrongReply)?;
                            (second,third,Tail::Closed(CompletedProbe{request,scope:created,retired:Retired::Size(retired)}))
                        }
                    },
                    Err(e)=>{
                        let status=root_failure(e)?;
                        let second=UnsignedSmall::probe_size_refusal(&parts[1],&query,status).map_err(|_|Fault::WrongReply)?;
                        let third=refuse_close(&parts[2],status)?;
                        let scope=query.rust_session_witness()?;
                        (second,third,Tail::Blocked(BlockedProbe{open:pending,proof:Blocked::OriginalFailure(scope)}))
                    },
                },
                Query::FileAll=>{
                    let witnessed=query.rust_session_witness()?;
                    open.require_current(tree,&witnessed).map_err(|_|Fault::SessionState)?;
                    // The virtual root cannot supply truthful Basic times
                    // and thus cannot produce a FileAll body. Respect the
                    // original READ_ATTRIBUTES precedence nevertheless.
                    let status=if open.allows_read_attributes(){FsInfoFailure::NotSupported}
                               else{FsInfoFailure::AccessDenied};
                    let second=UnsignedSmall::probe_all_refusal(&parts[1],&query,status).map_err(|_|Fault::WrongReply)?;
                    let third=refuse_close(&parts[2],status)?;
                    (second,third,Tail::Blocked(BlockedProbe{open:pending,proof:Blocked::OriginalFailure(witnessed)}))
                },
            };
            ([first,second,third],terminal)
        };
        self.next=PARTS;
        let reply=self.small_replies(drafts.into_iter())?;
        match terminal{
            Tail::Closed(done)=>reply.send_probe(done)?,
            Tail::Blocked(blocked)=>reply.send_blocked_probe(blocked)?,
        }
        Ok(true)
    }
    pub(in crate::smb) fn try_send_root_open_close_probe(&mut self)->Result<bool>{
        self.before_reply()?;
        let frame=self.original.frame;
        let [start,finish]=frame.messages()else{return Ok(false);};
        let Routing::Sync{tree:tid}=start.header.routing else{return Ok(false);};
        if start.header.command!=CREATE||finish.header.command!=CLOSE||
           start.header.flags&RELATED!=0||finish.header.flags&RELATED==0||
           !matches!(finish.header.routing,Routing::Sync{..})||
           !start.create_name_fields().is_ok_and(|v|v.name.is_empty())||
           !finish.close_request().is_ok_and(|c|c.open==OpenReference::PreviousInCompound)||
           self.original.payload(0)!=Some(Payload::NoVariableRule)||
           self.original.payload(1)!=Some(Payload::NoVariableRule){return Ok(false);}
        let sid=self.ids.at(0).ok_or(Fault::SessionState)?;
        if self.ids.at(1)!=Some(sid){return Err(Fault::SessionState);}
        let parts=[ProbePart{raw:start,sid,tree:tid,ordinal:0,count:2},ProbePart{raw:finish,sid,tree:tid,ordinal:1,count:2}];
        let share_modes=Arc::clone(&self.owner.origin.run.share_modes);
        let global=Arc::clone(&self.owner.origin.run.persistent);
        let (drafts,done)={
            let (scope,trees,opens)=self.owner.member_tables(sid,&self.revision)?;
            let Some(tree)=trees.root(tid)else{return self.probe_pair_refusal(&parts,RootFailure::NetworkNameDeleted);};
            let create=parts[0].member(&scope)?;
            let root=match share_modes.prepare_root_open(tree,&create){
                Ok(v)=>v,Err(e)=>return self.probe_pair_refusal(&parts,open_failure(e)?),
            };
            let stamp=create.rust_session_witness()?;
            let slot=match opens.reserve(&global,tid,Opened::Root(root)){
                Ok(v)=>v,
                Err(Fault::Resources|Fault::Exhausted)=>return self.probe_pair_refusal(&parts,RootFailure::InsufficientResources),
                Err(e)=>return Err(e),
            };
            let pending=PendingOpen::existing(RequestSlot{sid,tree:tid,mid:start.header.message},stamp,slot);
            pending.slot.object().require_current(tree,&pending.scope).map_err(|_|Fault::SessionState)?;
            let id=pending.disk_id_answer(CurrentName::Root);
            let first=UnsignedSmall::probe_opened(&parts[0],&create,&pending).map_err(|_|Fault::WrongReply)?;
            let close=parts[1].member(&scope)?;
            let closing=close.rust_session_witness()?;
            let PendingOpen{request,scope:created,slot,..}=pending;
            let receipt=opens.retire_root_pair(slot,tree,&close)?;
            let last=UnsignedSmall::probe_closed(&parts[1],&close,receipt).map_err(|_|Fault::WrongReply)?;
            ([first,last],CompletedProbe{request,scope:created,retired:Retired::Pair{closing,id}})
        };
        self.next=2;self.small_replies(drafts.into_iter())?.send_probe(done)?;Ok(true)
    }
    fn probe_pair_refusal(&mut self,parts:&[ProbePart<'_,'_>;2],failure:RootFailure)->Result<bool>{
        let drafts=[
            UnsignedSmall::probe_create_refusal(&parts[0],failure).map_err(|_|Fault::WrongReply)?,
            UnsignedSmall::probe_without_open(&parts[1]).map_err(|_|Fault::WrongReply)?,
        ];
        self.next=2;self.small_replies(drafts.into_iter())?.send()?;Ok(true)
    }
    fn probe_refusal(&mut self,parts:&[ProbePart<'_,'_>;PARTS],failure:RootFailure)->Result<bool>{
        // CREATE saw no real OPEN. The two later members cannot inherit a
        // FileId, even if the packet supplied non-sentinel wire numbers.
        let drafts=[
            UnsignedSmall::probe_create_refusal(&parts[0],failure).map_err(|_|Fault::WrongReply)?,
            UnsignedSmall::probe_without_open(&parts[1]).map_err(|_|Fault::WrongReply)?,
            UnsignedSmall::probe_without_open(&parts[2]).map_err(|_|Fault::WrongReply)?,
        ];
        self.next=PARTS;self.small_replies(drafts.into_iter())?.send()?;Ok(true)
    }
}
fn refuse_close(part:&ProbePart<'_,'_>,why:FsInfoFailure)->Result<UnsignedSmall>{
    UnsignedSmall::probe_close_blocked(part,why).map_err(|_|Fault::WrongReply)
}
enum Retired{Attributes(RetiredRootFs),Size(RetiredRootFsSize),Pair{closing:RustSessionWitness,id:Option<(u64,u64)>}}
enum Blocked{Size(PreparedRootFsSize),OriginalFailure(RustSessionWitness)}
enum Tail{Closed(CompletedProbe),Blocked(BlockedProbe)}

struct CompletedProbe{request:RequestSlot,scope:RustSessionWitness,retired:Retired}
impl CompletedProbe{
    fn check(&self,c:&Connection)->Result<()>{
        let tree=self.request.root(c)?;
        tree.ensure_current(&self.scope).map_err(|_|Fault::SessionState)?;
        match &self.retired{
            Retired::Attributes(v)=>v.require_current(tree),Retired::Size(v)=>v.require_current(tree),
            Retired::Pair{closing,id}=>{
                if let Some(expected)=id{
                    if tree.logical_root_disk_id(&self.scope).map_err(|_|Fault::SessionState)?!=*expected{
                        return Err(Fault::SessionState);
                    }
                }
                tree.ensure_current(closing)
            },
        }.map_err(|_|Fault::SessionState)
    }
}
struct BlockedProbe{open:PendingOpen,proof:Blocked}
impl BlockedProbe{
    fn check(&self,c:&Connection)->Result<()>{
        self.open.check(c)?;
        let tree=self.open.request.root(c)?;
        let Opened::Root(open)=self.open.slot.object()else{return Err(Fault::SessionState);};
        match &self.proof{
            Blocked::Size(p)=>p.require_current(open,tree),
            Blocked::OriginalFailure(w)=>tree.ensure_current(w),
        }.map_err(|_|Fault::SessionState)
    }
}
impl SyncReply<'_>{
    fn send_probe(self,done:CompletedProbe)->Result<()>{
        done.check(self.tx.plan.owner)?;
        let SyncReply{tx,ids,revision,count}=self.write_original()?.0;
        tx.publish_with(|c|{current(c,ids,revision,count)?;done.check(c)})
    }
    fn send_blocked_probe(self,failed:BlockedProbe)->Result<()>{
        // §3.3.5.2.7.2: the rejected QUERY still has the actual FileId,
        // and the following CLOSE carries its failure; CLOSE did not run.
        // Publish that real, still-open hold with its original successful
        // CREATE. Never advertise an ID which was secretly closed.
        failed.check(self.tx.plan.owner)?;
        let SyncReply{tx,ids,revision,count}=self.write_original()?.0;
        tx.publish_with(|c|{
            current(c,ids,revision,count)?;failed.check(c)?;
            c.open_table_mut(failed.open.request.sid)?.publish(failed.open.slot)
        })
    }
}
