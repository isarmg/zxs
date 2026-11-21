//! Explicit termination on the original verified connection. A resource
//! retirement is not a file CLOSE command, FLUSH or recycle transaction.
use super::{CheckedBatch,Connection,Fault,Payload,Result,RustSessionWitness,SyncReply,UnsignedSmall,VerifiedMember,
    current,root_io::RequestSlot};
use super::super::super::{Claim,Phase};
use crate::smb::{session::BatchRevision,wire::{fixed_reply::{ExitFailure,LOGOFF,TREE_DISCONNECT},message::{RELATED,Routing}}};
use std::{num::NonZeroU64,sync::Arc,time::Instant};

enum Which { Session(Arc<Claim>), Tree(RustSessionWitness) }
#[must_use]
pub(in crate::smb) struct ExitPlan{original:RequestSlot,kind:Which}
impl ExitPlan {
    pub(in crate::smb) fn sid(&self)->NonZeroU64{self.original.sid}
    pub(in crate::smb) fn matches(&self,member:&VerifiedMember<'_,'_>)->bool{
        let target=match &self.kind{
            Which::Session(original)=>member.message().header.command==LOGOFF&&Arc::ptr_eq(original,&member.session().reg.claim),
            Which::Tree(_)=>member.message().header.command==TREE_DISCONNECT,
        };
        target&&self.original.matches(member,self.sid())
    }
}

impl CheckedBatch<'_,'_>{
    pub(in crate::smb) fn try_send_exit(&mut self)->Result<bool>{
        self.before_reply()?;
        let frame=self.original.frame;
        let [m]=frame.messages() else{return Ok(false);};
        let Routing::Sync{tree}=m.header.routing else{return Ok(false);};
        if m.header.flags&RELATED!=0||!matches!(m.header.command,LOGOFF|TREE_DISCONNECT){return Ok(false);}
        if self.original.payload(0)!=Some(Payload::NoVariableRule){return Err(Fault::SessionState);}
        let sid=self.ids.at(0).ok_or(Fault::SessionState)?;
        let member=VerifiedMember{message:m,ordinal:0,session:self.owner.member_scope(sid,&self.revision)?};
        if m.raw.len()<68||m.raw[64..66]!=[4,0]{
            let failure=UnsignedSmall::exit_failure(&member,sid,ExitFailure::InvalidParameter).map_err(|_|Fault::WrongReply)?;
            return self.exit_refusal(failure);
        }
        // Neither command interprets the reserved word, or LOGOFF's
        // irrelevant request TreeId. Respect TreeDisconnect's actual TID.
        let kind=if m.header.command==LOGOFF {Which::Session(Arc::clone(&member.session().reg.claim))}else{
            let Some(root)=self.owner.tree_table(sid)?.root(tree) else{
                let failure=UnsignedSmall::exit_failure(&member,sid,ExitFailure::NetworkNameDeleted).map_err(|_|Fault::WrongReply)?;
                return self.exit_refusal(failure);
            };
            let scope=member.rust_session_witness()?;
            root.ensure_current(&scope).map_err(|_|Fault::SessionState)?;
            Which::Tree(scope)
        };
        let leave=ExitPlan{original:RequestSlot{sid,tree,mid:m.header.message},kind};
        let draft=UnsignedSmall::leaving(&member,&leave).map_err(|_|Fault::WrongReply)?;
        self.next=1;
        self.small_replies(std::iter::once(draft))?.send_exit(leave)?;Ok(true)
    }
    fn exit_refusal(&mut self,draft:UnsignedSmall)->Result<bool>{
        self.next=1;self.small_replies(std::iter::once(draft))?.send()?;Ok(true)
    }
}

impl SyncReply<'_>{
    fn send_exit(self,plan:ExitPlan)->Result<()>{
        let sid=plan.original.sid;
        if self.count!=1||self.ids.at(0)!=Some(sid){return Err(Fault::WrongReply);}
        match plan.kind{
            Which::Tree(scope)=>{
                current(self.tx.plan.owner,self.ids,self.revision,1)?;
                self.tx.plan.owner.retire_tree(sid,plan.original.tree,&scope)?;
                self.write_original()?.publish_ledger()
            },
            Which::Session(claim)=>{
                let SyncReply{tx,ids,revision,count}=self;
                if tx.start!=4{return Err(Fault::WrongReply);}
                current(tx.plan.owner,ids,revision,count)?;
                let ending=tx.plan.owner.retire_session(claim,revision)?;
                let mut stream=tx.plan.owner.stream.take().ok_or(Fault::Transport)?;
                let c:&Connection=&*tx.plan.owner;
                ending.current(c,revision)?;
                stream.write_record(tx.actual.as_ref(),||ending.current(c,revision))?;
                ending.current(c,revision)?;
                tx.plan.owner.stream=Some(stream);
                tx.publish_with(|c|ending.current(c,revision))
            },
        }
    }
}
impl Connection {
    fn retire_tree(&mut self,sid:NonZeroU64,tid:u32,scope:&RustSessionWitness)->Result<()>{
        let item=self.sessions.iter_mut().find(|v|v.reg.claim.id==sid).ok_or(Fault::SessionState)?;
        let Phase::Valid(live)=&mut item.phase else{return Err(Fault::SessionState);};
        let root=live.trees.root(tid).ok_or(Fault::SessionState)?;
        root.ensure_current(scope).map_err(|_|Fault::SessionState)?;
        live.opens.retire_tree(tid)?;
        live.trees.retire(tid)
    }
    fn retire_session(&mut self,claim:Arc<Claim>,revision:&BatchRevision)->Result<Retired>{
        self.member_scope(claim.id,revision)?;
        let i=self.sessions.iter().position(|v|Arc::ptr_eq(&v.reg.claim,&claim)).ok_or(Fault::SessionState)?;
        let closed=self.sessions.swap_remove(i); // no ordinary local lookup from this point
        let Phase::Valid(live)=closed.phase else{return Err(Fault::SessionState);};
        drop(live); // original opens before trees; no business I/O on Drop
        drop(closed.reg); // only this global session registration
        let retired=Retired(claim);retired.current(self,revision)?;Ok(retired)
    }
}
/// No `BoundSession`, keys, privileges or route. The only right is to
/// finish the original already signed record on its own connection while
/// its original credential is still current. Never re-register this Claim.
struct Retired(Arc<Claim>);
impl Retired {
    fn current(&self,c:&Connection,revision:&BatchRevision)->Result<()>{
        c.alive()?;
        let service=c.origin.run.verifier.as_ref().ok_or(Fault::NoVerifier)?;
        if !self.0.origin.ptr_eq(&Arc::downgrade(&c.origin))||
            !self.0.active(Instant::now())||!revision.current(service)||
            c.sessions.iter().any(|item|item.reg.claim.id==self.0.id){return Err(Fault::SessionState);}
        let index=c.origin.run.index.lock().map_err(|_|Fault::SessionState)?;
        if index.live.iter().any(|item|item.ptr_eq(&Arc::downgrade(&self.0))){return Err(Fault::SessionState);}
        Ok(())
    }
}
