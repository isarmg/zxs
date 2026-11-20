//! From a paid frame on its original connection to short, ordered, signed
//! session-member borrows. Not a Tree, Open, execution result or reply.
pub(in crate::smb) mod echo;
use super::{Phase,Registration};
use crate::smb::{
    connection::{Connection,Fault,Origin,Payload,Received,Result},
    session::{self,BatchRevision,BoundSession,VerifiedIds},
    wire::message::{Message,RELATED},
};
use std::{num::NonZeroU64,sync::{Arc,atomic::Ordering},time::Instant};

#[must_use]
pub(in crate::smb) enum Admission<'owner,'raw> {
    Ready(CheckedBatch<'owner,'raw>),
    AnotherConnection(Received<'raw>), // nothing touched on the mistaken owner
    Other(Received<'raw>),            // setup, cancel or an unsupported command
    Refused(Refused<'raw>),           // a future error writer still owes THIS frame
    TransportGone,
}
pub(in crate::smb) struct Refused<'raw> {original:Received<'raw>,ordinal:usize,reason:session::Fault}
impl Refused<'_> {
    pub(in crate::smb) fn failure(&self)->(usize,session::Fault) {(self.ordinal,self.reason)}
}

#[must_use]
pub(in crate::smb) struct CheckedBatch<'owner,'raw> {
    owner:&'owner mut Connection,original:Received<'raw>,ids:VerifiedIds,revision:BatchRevision,
    next:usize,credit_failed:bool,halted:bool,
}
#[must_use]
pub(in crate::smb) enum Step<'step,'raw> {
    Verified(VerifiedMember<'step,'raw>),
    InvalidCharge{ordinal:usize}, // legal MID was spent, no executable member
    AfterFailedCredit{ordinal:usize}, // never infer preceding Tree/Open or an NTSTATUS here
}
/// This borrow cannot outlive the parent batch or overlap its next step.
/// Even this view proves only the session, NOT a Tree, Open or access mask.
#[must_use]
pub(in crate::smb) struct VerifiedMember<'step,'raw> {
    message:&'step Message<'raw>,ordinal:usize,session:SessionView<'step>,
}
impl<'step,'raw> VerifiedMember<'step,'raw> {
    pub(in crate::smb) fn message(&self)->&Message<'raw> {self.message}
    pub(in crate::smb) fn ordinal(&self)->usize {self.ordinal}
    pub(in crate::smb) fn session(&self)->&SessionView<'step> {&self.session}
}
/// Not Clone, no constructor from an ID, no key or long-lived account token.
pub(in crate::smb) struct SessionView<'step> {
    reg:&'step Registration,origin:&'step Arc<Origin>,revision:&'step BatchRevision,
}
impl SessionView<'_> {
    /// Call again at use/commit. No registry or credential lock survives this
    /// function, in particular none may be held across Core storage or network.
    pub(in crate::smb) fn recheck(&self)->Result<()> {
        if self.origin.closed.load(Ordering::Acquire) {return Err(Fault::Closed);}
        let s=self.origin.run.verifier.as_ref().ok_or(Fault::NoVerifier)?;
        if self.reg.claim.valid.get().is_none() || !self.revision.current(s) {return Err(Fault::SessionState);}
        match self.reg.current(self.origin,Instant::now()) {
            Ok(true)=>Ok(()),Ok(false)=>Err(Fault::SessionState),
            Err(e)=>{self.origin.closed.store(true,Ordering::Release);Err(e)},
        }
    }
}
impl Connection {
    fn valid_slot(&self,sid:NonZeroU64)->Option<(&Registration,&BoundSession)> {
        let item=self.sessions.iter().find(|v|v.reg.claim.id==sid)?;
        match &item.phase {Phase::Valid(bound)=>Some((&item.reg,bound)),_=>None}
    }
    fn member_scope<'a>(&'a self,sid:NonZeroU64,revision:&'a BatchRevision)->Result<SessionView<'a>> {
        let (reg,bound)=self.valid_slot(sid).ok_or(Fault::SessionState)?;
        if !bound.on_revision(revision) {return Err(Fault::SessionState);}
        let view=SessionView{reg,origin:&self.origin,revision};view.recheck()?;Ok(view)
    }
}
impl<'raw> Received<'raw> {
    fn unsigned_live_tree(&self,i:usize,c:&Connection,revision:&BatchRevision)->bool {
        let parts=self.frame.messages();
        let Some(m)=parts.get(i) else {return false;};
        if m.header.command!=3 || m.header.flags&crate::smb::wire::message::SIGNED!=0 {return false;}
        // verify_published already checked every preceding signature up to i.
        // In a related chain the preceding non-related member supplied the
        // effective SID; never interpret literal all-ones/another SID as a new owner.
        let Some(seed)=parts[..=i].iter().rev().find(|p|p.header.flags&RELATED==0) else {return false;};
        let Some(sid)=NonZeroU64::new(seed.header.session).filter(|id|id.get()!=u64::MAX) else {return false;};
        let Some((reg,bound))=c.valid_slot(sid) else {return false;};
        if !bound.on_revision(revision) || reg.claim.valid.get().is_none() {return false;}
        match reg.current(&c.origin,Instant::now()) {
            Ok(is_current)=>is_current,
            Err(_)=>{c.origin.closed.store(true,Ordering::Release);false}
        }
    }
    /// Preflight without callbacks into business code. All original MIDs
    /// have already been debited. Verify each exact segment, including its
    /// own padding, even for a paid member with an insufficient payload charge.
    pub(in crate::smb) fn preflight<'owner>(self,owner:&'owner mut Connection)->Admission<'owner,'raw> {
        if !Arc::ptr_eq(&self.debit.origin,&owner.origin) {return Admission::AnotherConnection(self);}
        if owner.alive().is_err() {owner.disconnect();return Admission::TransportGone;}
        if !self.debit.owed || self.frame.messages().iter().any(|m|!matches!(m.header.command,2..=11|13..=18)) {
            return Admission::Other(self);
        }
        let Some(revision)=owner.origin.run.verifier.as_ref().and_then(|s|s.batch_revision()) else {
            return Admission::Refused(Refused{original:self,ordinal:0,reason:session::Fault::UnknownSession});
        };
        let c:&Connection=&*owner;
        let signed=session::verify_published(self.frame,&revision,|id|{
            let (reg,bound)=c.valid_slot(id)?;
            match reg.current(&c.origin,Instant::now()) {
                Ok(true)=>Some(bound),Ok(false)=>None,
                Err(_)=>{c.origin.closed.store(true,Ordering::Release);None},
            }
        });
        if c.alive().is_err() {owner.disconnect();return Admission::TransportGone;}
        let ids=match signed {
            Ok(ids)=>ids,
            Err((ordinal,reason))=>{
                // MS-SMB2 3.3.5.7: on this plaintext-only 3.1.1 transport
                // a naked TREE_CONNECT for our real non-guest session is a
                // mandatory disconnect, not a reply followed by continued use.
                // This special terminal disposition never authenticates a byte.
                if reason==session::Fault::SignatureRequired && self.unsigned_live_tree(ordinal,c,&revision) {
                    owner.disconnect();return Admission::TransportGone;
                }
                if c.alive().is_err() {owner.disconnect();return Admission::TransportGone;}
                return Admission::Refused(Refused{original:self,ordinal,reason});
            }
        };
        for i in 0..self.frame.messages().len() {
            let valid=ids.at(i).is_some_and(|sid|c.member_scope(sid,&revision).is_ok());
            if c.alive().is_err() {owner.disconnect();return Admission::TransportGone;}
            if !valid {return Admission::Refused(Refused{original:self,ordinal:i,reason:session::Fault::UnknownSession});}
        }
        Admission::Ready(CheckedBatch{owner,original:self,ids,revision,next:0,credit_failed:false,halted:false})
    }
}
impl<'owner,'raw> CheckedBatch<'owner,'raw> {
    /// Original order, at most one short session-member borrow at a time.
    /// A future command dispatcher must still consume the *adjacent real
    /// outcome* for related Tree/Open IDs; none is synthesized here.
    pub(in crate::smb) fn next(&mut self)->Result<Option<Step<'_,'raw>>> {
        if self.halted {return Err(Fault::SessionState);}
        let i=self.next;
        let Some(m)=self.original.frame.messages().get(i) else {return Ok(None);};
        let Some(id)=self.ids.at(i) else {self.halted=true;return Err(Fault::SessionState);};
        let scope=match self.owner.member_scope(id,&self.revision) {
            Ok(s)=>s,Err(e)=>{self.halted=true;return Err(e)},
        };
        self.next+=1;
        let related=m.header.flags&RELATED!=0;
        if !related {self.credit_failed=false;}
        match self.original.payload(i) {
            Some(Payload::InvalidParameter)=>{
                self.credit_failed=true;Ok(Some(Step::InvalidCharge{ordinal:i}))
            }
            Some(Payload::Covered|Payload::NoVariableRule) if self.credit_failed=>
                Ok(Some(Step::AfterFailedCredit{ordinal:i})),
            Some(Payload::Covered|Payload::NoVariableRule)=>
                Ok(Some(Step::Verified(VerifiedMember{message:m,ordinal:i,session:scope}))),
            _=>{self.halted=true;Err(Fault::SessionState)}
        }
    }
}

#[cfg(test)]
#[path = "../../../test/smb/ordinary.rs"]
pub(in crate::smb) mod tests;
