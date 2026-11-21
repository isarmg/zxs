//! One real directory CREATE immediately followed by its own QUERY_DIRECTORY.
//! No synthetic handle, other operation, or related-packet ID is admitted.
use super::{PendingRootQuery,QueryPrepared,query_failure};
use super::super::{CheckedBatch,Connection,Fault,Payload,Result,RustRoot,RustSessionWitness,SyncReply,
    UnsignedSmall,VerifiedMember,current,root_io::{PendingOpen,RequestSlot,directory_open_failure,open_failure}};
use super::super::super::{SessionView,opens::Opened};
use crate::smb::{
    backend::{OpenCoordinator,RootOpenError},
    wire::{create_fields::Disposition,direct::MAX_FRAME_BYTES,fixed_reply::RootFailure,
        io_fields::OpenReference,message::{Message,RELATED,Routing},
        query_reply::{DIRECTORY_COMMAND,DirectoryFailure,UnsignedDirectory}},
};
use std::{num::NonZeroU64,sync::Arc};

const CREATE:u16=5;
const FILE_DIRECTORY_FILE:u32=0x0000_0001;
const PARTS:usize=2;

/// Only constructed from a matching pair after paid whole-frame verification.
pub(in crate::smb) struct BrowsePart<'a,'r>{
    raw:&'a Message<'r>,sid:NonZeroU64,tid:u32,i:usize,
}
impl<'a,'r> BrowsePart<'a,'r>{
    pub(in crate::smb) fn fields(&self)->(&'a Message<'r>,NonZeroU64,u32,usize){
        (self.raw,self.sid,self.tid,self.i)
    }
    pub(in crate::smb) fn matches(&self,m:&VerifiedMember<'_,'_>)->bool{
        m.ordinal()==self.i&&std::ptr::eq(m.message(),self.raw)&&m.response_session_matches(self.sid)
    }
    fn member<'s>(&'s self,scope:&'s SessionView<'_>)->Result<VerifiedMember<'s,'r>>{
        scope.recheck()?;
        if scope.reg.claim.id!=self.sid{return Err(Fault::SessionState);}
        Ok(VerifiedMember{message:self.raw,ordinal:self.i,session:SessionView{
            reg:scope.reg,origin:scope.origin,revision:scope.revision,limits:scope.limits,
        }})
    }
}

impl PendingRootQuery{
    pub(in crate::smb) fn matches_browse(&self,part:&BrowsePart<'_,'_>,m:&VerifiedMember<'_,'_>)->bool{
        part.i==1&&part.matches(m)&&self.original.sid==part.sid&&self.original.tree==part.tid&&
            self.original.mid==part.raw.header.message
    }
}

impl CheckedBatch<'_,'_>{
    pub(in crate::smb) fn try_send_related_browse(&mut self)->Result<bool>{
        self.before_reply()?;
        let frame=self.original.frame;
        let [create,search]=frame.messages()else{return Ok(false);};
        let Routing::Sync{tree:tid}=create.header.routing else{return Ok(false);};
        if create.header.command!=CREATE||search.header.command!=DIRECTORY_COMMAND||
           create.header.flags&RELATED!=0||search.header.flags&RELATED==0||
           !matches!(search.header.routing,Routing::Sync{..})||
           !create.create_fields().is_ok_and(|f|f.disposition()==Disposition::Open&&
               f.effective_options()&FILE_DIRECTORY_FILE!=0)||
           !search.directory_target().is_ok_and(|id|id==OpenReference::PreviousInCompound)||
           self.original.payload(0)!=Some(Payload::NoVariableRule)||
           self.original.payload(1)!=Some(Payload::Covered){return Ok(false);}
        let sid=self.ids.at(0).ok_or(Fault::SessionState)?;
        if self.ids.at(1)!=Some(sid){return Err(Fault::SessionState);}
        let first=BrowsePart{raw:create,sid,tid,i:0};
        let last=BrowsePart{raw:search,sid,tid,i:1};
        let modes=Arc::clone(&self.owner.origin.run.share_modes);
        let global=Arc::clone(&self.owner.origin.run.persistent);
        let (draft,after,outcome)={
            let (scope,trees,opens)=self.owner.member_tables(sid,&self.revision)?;
            let Some(tree)=trees.root(tid)else{return self.browse_refuse_open(&first,&last,RootFailure::NetworkNameDeleted);};
            let member=first.member(&scope)?;
            let object=match folder(&modes,tree,&member)?{
                Ok(v)=>v,Err(e)=>return self.browse_refuse_open(&first,&last,e),
            };
            let created=member.rust_session_witness()?;
            let slot=match opens.reserve(&global,tid,object){
                Ok(v)=>v,Err(Fault::Resources|Fault::Exhausted)=>
                    return self.browse_refuse_open(&first,&last,RootFailure::InsufficientResources),
                Err(e)=>return Err(e),
            };
            let mut opening=PendingOpen::existing(RequestSlot{sid,tree:tid,mid:create.header.message},created,slot);
            let draft=UnsignedSmall::browse_opened(&first,&member,&opening).map_err(|_|Fault::WrongReply)?;
            let member=last.member(&scope)?;
            let authority=member.rust_session_witness()?;
            let room=(u64::from(member.max_transact())+256).min(MAX_FRAME_BYTES as u64) as usize;
            // The other member, fixed search reply and worst final padding
            // must all fit. The client's own OutputBufferLength still binds.
            let room=room.checked_sub(draft.smb_len()+72+7).ok_or(Fault::FrameSize)?;
            let prepared=match opening.slot_mut().object_mut(){
                Opened::Root(dir)=>dir.prepare_query(tree,&member,room).map(QueryPrepared::Root),
                Opened::Directory(dir)=>dir.prepare_query(tree,&member,room).map(QueryPrepared::Ordinary),
                Opened::Retained(dir) if dir.is_directory()=>dir.prepare_query(tree,&member,room).map(QueryPrepared::Retained),
                _=>return Err(Fault::SessionState),
            };
            let query=RequestSlot{sid,tree:tid,mid:search.header.message};
            let chosen=match prepared{
                Ok(prepared)=>{
                    let reply=PendingRootQuery{original:RequestSlot{sid,tree:tid,mid:query.mid},wire:opening.wire(),prepared};
                    BrowseReply::Page(UnsignedDirectory::new_related(&last,&member,reply).map_err(|_|Fault::WrongReply)?)
                },
                Err(e)=>BrowseReply::Refusal(UnsignedSmall::browse_query_failure(&last,Some(query_failure(e)?))
                    .map_err(|_|Fault::WrongReply)?),
            };
            (draft,PendingBrowse{opening,query,authority},chosen)
        };
        self.next=PARTS;
        match outcome{
            BrowseReply::Refusal(denied)=>self.small_replies([draft,denied].into_iter())?.send_browse(after,None)?,
            BrowseReply::Page(page)=>self.send_browse_record(draft,page,after)?,
        }
        Ok(true)
    }

    fn browse_refuse_open(&mut self,first:&BrowsePart<'_,'_>,last:&BrowsePart<'_,'_>,why:RootFailure)->Result<bool>{
        let denied=UnsignedSmall::browse_create_failure(first,why).map_err(|_|Fault::WrongReply)?;
        let no_open=UnsignedSmall::browse_query_failure(last,None).map_err(|_|Fault::WrongReply)?;
        self.next=PARTS;self.small_replies([denied,no_open].into_iter())?.send()?;Ok(true)
    }

    fn send_browse_record(&mut self,first:UnsignedSmall,page:UnsignedDirectory,after:PendingBrowse)->Result<()>{
        if self.next!=PARTS||self.original.frame.messages().len()!=PARTS{return Err(Fault::WrongReply);}
        self.halted=true;
        let ids=&self.ids;let revision=&self.revision;
        let plan=self.owner.plan_debit(&mut self.original.debit)?;
        let first=first.credit(plan.credit_response(0).ok_or(Fault::NoReply)?);
        let page=page.credit(plan.credit_response(1).ok_or(Fault::NoReply)?);
        let sid=first.sid();
        if page.sid()!=sid||ids.at(0)!=Some(sid)||ids.at(1)!=Some(sid){return Err(Fault::SessionState);}
        plan.owner.member_scope(sid,revision)?;
        let (_,bound)=plan.owner.valid_slot(sid).ok_or(Fault::SessionState)?;
        let open=bound.sign_small_reply(revision,first).map_err(|_|Fault::SessionState)?;
        let (page,proof)=bound.sign_directory(revision,page).map_err(|_|Fault::SessionState)?.into_send();
        let directory=page.get(4..).ok_or(Fault::WrongReply)?;
        let len=open.bytes().len().checked_add(directory.len()).ok_or(Fault::FrameSize)?;
        if len>MAX_FRAME_BYTES||len as u64>u64::from(plan.owner.limits.transact)+256{return Err(Fault::FrameSize);}
        let mut whole=Vec::new();whole.try_reserve_exact(4+len).map_err(|_|Fault::Resources)?;
        // The isolated second member's transport prefix is not signed;
        // drop it and write a single record for the original compound.
        whole.extend_from_slice(&(len as u32).to_be_bytes());
        whole.extend_from_slice(open.bytes());whole.extend_from_slice(directory);
        let tx=plan.bind_record(whole)?;
        SyncReply{tx,ids,revision,count:PARTS}.send_browse(after,Some(proof))
    }
}

fn folder(modes:&Arc<OpenCoordinator>,tree:&RustRoot,m:&VerifiedMember<'_,'_>)
    ->Result<std::result::Result<Opened,RootFailure>>{
    match modes.prepare_root_open(tree,m){
        Ok(root)=>Ok(Ok(Opened::Root(root))),
        Err(RootOpenError::NonRoot)=>match modes.prepare_retained_open(tree,m){
            Ok(Some(retained)) if retained.is_directory()=>Ok(Ok(Opened::Retained(retained))),
            Ok(Some(_))=>Ok(Err(RootFailure::NotADirectory)),
            Ok(None)=>match modes.prepare_directory_open(tree,m){
                Ok(Some(dir))=>Ok(Ok(Opened::Directory(dir))),
                Ok(None)=>Ok(Err(RootFailure::NotADirectory)),
                Err(e)=>Ok(Err(directory_open_failure(e)?)),
            },
            Err(e)=>Ok(Err(directory_open_failure(e)?)),
        },
        Err(e)=>Ok(Err(open_failure(e)?)),
    }
}
enum BrowseReply{Page(UnsignedDirectory),Refusal(UnsignedSmall)}

struct PendingBrowse{opening:PendingOpen,query:RequestSlot,authority:RustSessionWitness}
impl PendingBrowse{
    fn check(&self,c:&Connection,page:Option<&PendingRootQuery>)->Result<()>{
        self.opening.check(c)?;
        let tree=self.query.root(c)?;
        tree.ensure_current(&self.authority).map_err(|_|Fault::SessionState)?;
        if let Some(p)=page{
            if p.wire!=self.opening.wire()||p.original.mid!=self.query.mid||
               p.original.sid!=self.query.sid||p.original.tree!=self.query.tree{return Err(Fault::SessionState);}
            p.prepared.require_on(self.opening.slot().object(),tree)?;
        }
        Ok(())
    }
    fn publish(mut self,c:&mut Connection,page:Option<PendingRootQuery>)->Result<()>{
        self.check(c,page.as_ref())?;
        if let Some(p)=page{
            let tree=self.query.root(c)?;
            // The complete signed SMB record is already on its stream.
            // A failed late check leaves both this OPEN and its page unpublished.
            p.prepared.apply_written(self.opening.slot_mut().object_mut(),tree)?;
        }
        self.opening.publish(c)
    }
}
impl SyncReply<'_>{
    fn send_browse(self,done:PendingBrowse,page:Option<PendingRootQuery>)->Result<()>{
        done.check(self.tx.plan.owner,page.as_ref())?;
        let SyncReply{tx,ids,revision,count}=self.write_original()?.0;
        tx.publish_with(|c|{current(c,ids,revision,count)?;done.publish(c,page)})
    }
}
