//! One synchronous receive turn on the original negotiated duplex. Neither
//! the caller nor a fallback can supply a frame, mutable connection or socket.
use super::{Peer,parsed};
use crate::smb::{
    connection::{Connection,Fault,Origin,Received,Result,sessions::{Resume,ordinary::{Admission,MoveEscrow}},setup::{Start,Why}},
    session::envelope::Failure,wire::{direct::MAX_FRAME_BYTES,message::{CANCEL,Compound},setup_reply},
};
use std::{sync::{Arc,Weak,atomic::Ordering},time::Instant};

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::smb) enum Turn {Idle,SelectionWritten,ChallengeWritten,FinalWritten,ErrorWritten,EchoWritten,TreeRefused,CancelNoTarget,Routed}
/// A real future handler must either settle its own original receipt or stop.
/// In particular an unsigned error reply is NOT a substitute for checking a
/// signed reauth, binding or an unrelated protected session.
#[must_use]
pub(in crate::smb) enum Outstanding<'c,'r> {
    Ordinary(Admission<'c,'r>),
    SpecialSetup(SpecialSetup<'r>),
}
pub(in crate::smb) struct SpecialSetup<'r> { _original:Received<'r> }

#[derive(Clone,Copy)]
enum Issuer { Protected, #[cfg(test)] FixedChallenge }

/// Created only by receiving one complete record from this owner's
/// original stream. No caller can trade a signed body or Direct TCP prefix
/// onto different bytes, reissue a paid debit, or substitute a sink.
#[must_use]
struct CapturedRecord{origin:Weak<Origin>,bytes:Vec<u8>}
impl CapturedRecord{
    fn from_stream(owner:&mut Connection,limit:usize)->Result<Option<Self>>{
        let got=owner.stream.as_mut().ok_or(Fault::Transport)?.poll_record(limit)?;
        Ok(got.map(|bytes|Self{origin:Arc::downgrade(&owner.origin),bytes}))
    }
    fn require_owner(&self,owner:&Connection)->Result<()>{
        if !self.origin.ptr_eq(&Arc::downgrade(&owner.origin)){
            // A consumed frame can't silently vanish on its true origin.
            // The mistaken recipient does not inherit or spend its credit.
            if let Some(source)=self.origin.upgrade(){source.closed.store(true,Ordering::Release);}
            return Err(Fault::WrongBatch);
        }
        owner.alive()
    }
    fn frame_on<'a>(&'a self,owner:&Connection)->Result<Compound<'a>>{
        self.require_owner(owner)?;
        parsed(&self.bytes)
    }
}

/// Only the peer's actual immutable input, while still borrowed, can
/// export an owned escrow from the qualified synchronous batch.
pub(in crate::smb::connection) struct MoveCapture<'frame,'wire>{frame:&'frame Compound<'wire>}
impl MoveCapture<'_,'_>{
    pub(in crate::smb::connection) fn matches(&self,frame:&Compound<'_>)->bool{std::ptr::eq(self.frame,frame)}
}

/// The one real connection owner holds the immutable input allocation,
/// charged debt, original signer revision and bound FILE proposal together.
/// No full Connection, SQLite borrow or cryptographic key crosses a worker
/// boundary. Drop still owes the original terminal reply and closes that
/// origin; nothing in this slice may declare or settle its success.
#[must_use]
pub(super) struct PendingOriginalMove{held:MoveEscrow,input:CapturedRecord}
impl PendingOriginalMove{
    fn prepare(input:CapturedRecord,owner:&mut Connection)->Result<Self>{
        let held={
            let frame=input.frame_on(owner)?;
            let [raw]=frame.messages()else{return Err(Fault::WrongReply);};
            if !raw.is_basic_rename_request()||owner.in_flight!=0{return Err(Fault::WrongReply);}
            let capture=MoveCapture{frame:&frame};
            let paid=owner.receive(&frame)?;
            let admitted=paid.preflight(owner);
            match admitted{
                Admission::Ready(batch)=>batch.into_move_escrow(capture)?,
                Admission::TransportGone=>return Err(Fault::Closed),
                _=>return Err(Fault::SessionState),
            }
        };
        let pending=Self{held,input};
        pending.require_original(owner)?;
        Ok(pending)
    }
    fn require_original(&self,owner:&Connection)->Result<()>{
        let frame=self.input.frame_on(owner)?;
        self.held.require_original(owner,&frame)
    }
}

impl Peer {
    /// Not called by the real receive loop yet. That may change only once
    /// socket/worker events, authenticated cancellation, held target rebind
    /// and the original reply/credit terminal transition are implemented.
    fn park_original_move(&mut self,input:CapturedRecord)->Result<()>{
        if let Err(e)=input.require_owner(&self.connection){
            if e!=Fault::WrongBatch{self.disconnect_original();}
            return Err(e);
        }
        let prepare=if self.original_move.is_none(){PendingOriginalMove::prepare(input,&mut self.connection)}
            else{Err(Fault::WrongBatch)};
        match prepare{
            Ok(pending)=>{self.original_move=Some(pending);Ok(())},
            Err(e)=>{self.disconnect_original();Err(e)},
        }
    }
    fn disconnect_original(&mut self){
        // Invalidate the connection and its local tables before letting
        // go of the parked original's FILE guard. Never issue a credit,
        // implicit CLOSE/SAVE, or a second result on this path.
        self.connection.disconnect();
        drop(self.original_move.take());
    }
    pub(in crate::smb) fn poll(&mut self,
        other:impl for<'c,'r> FnOnce(Outstanding<'c,'r>)->Result<()>)->Result<Turn> {
        self.poll_with(Issuer::Protected,other)
    }
    fn housekeeping(&mut self)->Result<()> {
        self.connection.alive()?;let now=Instant::now();
        if self.connection.reap_and_has_valid(now) {self.first_login_by=None;}
        if self.first_login_by.is_some_and(|end|now>=end) {return Err(Fault::SessionState);}Ok(())
    }
    fn poll_with(&mut self,issuer:Issuer,other:impl for<'c,'r> FnOnce(Outstanding<'c,'r>)->Result<()>)->Result<Turn> {
        if self.original_move.is_some(){self.disconnect_original();return Err(Fault::Closed);}
        let result=self.once(issuer,other);
        // Never skip a paid frame by dropping/forgetting an outstanding
        // operation. This owner is strictly synchronous, with no pending async I/O.
        if result.is_err() || self.connection.alive().is_err() || self.connection.in_flight!=0 {
            self.disconnect_original();return Err(result.err().unwrap_or(Fault::Closed));
        }
        result
    }
    fn once(&mut self,issuer:Issuer,other:impl for<'c,'r> FnOnce(Outstanding<'c,'r>)->Result<()>)->Result<Turn> {
        self.housekeeping()?;
        let c=&mut self.connection;
        let limit=(u64::from(c.limits.transact)+256).min(MAX_FRAME_BYTES as u64) as usize;
        let Some(record)=CapturedRecord::from_stream(c,limit)? else {
            self.housekeeping()?;return Ok(Turn::Idle);
        };
        self.housekeeping()?;
        let c=&mut self.connection;
        let frame=record.frame_on(c)?;let parts=frame.messages();let paid=c.receive(&frame)?;
        if parts.iter().any(|m|m.header.command==CANCEL) {
            // Do not route a mixed frame, even if another member was paid.
            paid.discard_idle_cancel(c)?;
            return Ok(Turn::CancelNoTarget);
        }
        if parts.iter().any(|m|m.header.command==1) {
            let special=match parts {
                [m]=>!setup_reply::plain_header(m) || (m.raw.get(64..66)==Some(&25u16.to_le_bytes()[..]) &&
                    (m.raw.get(66).is_some_and(|flags|*flags&1!=0) ||
                     m.raw.get(80..88).is_some_and(|v|v!=&[0u8;8][..]))),
                _=>true,
            };
            if special {other(Outstanding::SpecialSetup(SpecialSetup{_original:paid}))?;return Ok(Turn::Routed);}
        }
        match paid.start_setup() {
            Start::Type1(mut x)=>{
                if !c.has_protected_provider() {return denied(c,x.original,Reason::NoCredentials);}
                let result=match issuer {Issuer::Protected=>x.challenge(c).and_then(|tx|tx.send()),
                    #[cfg(test)] Issuer::FixedChallenge=>x.fixture_send(c)};
                after_prepare(c,x.original,result,Turn::ChallengeWritten)
            }
            Start::Select(mut x)=>{
                if !c.has_protected_provider() {return denied(c,x.original,Reason::NoCredentials);}
                let result=x.prepare(c).and_then(|tx|tx.send());
                after_prepare(c,x.original,result,Turn::SelectionWritten)
            }
            Start::Existing(x)=>match x.continue_setup(c) {
                Resume::Type1(mut x)=>{
                    let result=match issuer {Issuer::Protected=>x.challenge(c).and_then(|tx|tx.send()),
                        #[cfg(test)] Issuer::FixedChallenge=>x.fixture_send(c)};
                    after_prepare(c,x.original,result,Turn::ChallengeWritten)
                }
                Resume::Proven(mut x)=>{
                    let result=x.prepare(c).and_then(|tx|tx.send());
                    let turn=after_prepare(c,x.original,result,Turn::FinalWritten)?;
                    if turn==Turn::FinalWritten {self.first_login_by=None;}Ok(turn)
                }
                Resume::NoSession(x)=>denied(c,x.original,Reason::UnknownSession),
                Resume::InvalidToken(x)=>denied(c,x.original,Reason::InvalidToken),
                Resume::ProofRejected(x)=>denied(c,x.original,Reason::LogonFailure),
                Resume::EstablishedSetupUnsupported(x)=>{
                    other(Outstanding::SpecialSetup(SpecialSetup{_original:x.original}))?;Ok(Turn::Routed)
                }
                Resume::AnotherConnection(_)=>Err(Fault::WrongBatch),Resume::TransportGone=>Err(Fault::Closed),
            },
            Start::Refused(x)=>{
                let why=match x.reason() {
                    Why::Mechanism(Failure::Stage)=>Reason::Parameters,
                    Why::Mechanism(Failure::Malformed|Failure::Mechanism)=>Reason::InvalidToken,
                    Why::Mechanism(Failure::Unsupported)=>Reason::Unsupported,
                    Why::Mechanism(Failure::Limit)=>Reason::Resources,Why::SetupCompound=>return Err(Fault::SetupPhase),
                };denied(c,x.original,why)
            }
            Start::Other(x)=>{
                let echo=parts.iter().all(|m|m.header.command==13);
                match x.preflight(c) {
                    Admission::Ready(mut batch) if echo=>{
                        let tx=batch.echoes()?.ok_or(Fault::SessionState)?;tx.send()?;Ok(Turn::EchoWritten)
                    }
                    Admission::Ready(mut batch)=>{
                        if batch.try_send_tree_refusal()? {return Ok(Turn::TreeRefused);}
                        other(Outstanding::Ordinary(Admission::Ready(batch)))?;Ok(Turn::Routed)
                    }
                    Admission::TransportGone=>Err(Fault::Closed),
                    work=>{other(Outstanding::Ordinary(work))?;Ok(Turn::Routed)}
                }
            }
            Start::TransportGone=>Err(Fault::Closed),
        }
    }
}

// A known pre-write refusal still owns its original debit. A failed or
// uncertain bound sender already retired the stream: never send a second record.
fn after_prepare(c:&mut Connection,original:Received<'_>,result:Result<()>,success:Turn)->Result<Turn> {
    let Err(error)=result else {return if original.debit.owed {Err(Fault::WrongReply)} else {Ok(success)};};
    if c.alive().is_err() || c.stream.is_none() || !original.debit.owed {return Err(error);}
    let reason=match error {
        Fault::SessionLimit|Fault::Resources=>Reason::Resources,Fault::NoVerifier=>Reason::NoCredentials,
        Fault::ClientDialect=>Reason::UnknownSession,Fault::SetupPhase=>Reason::LogonFailure,
        _=>return Err(error),
    };denied(c,original,reason)
}

#[derive(Clone,Copy)]
#[repr(u32)]
enum Reason {UnknownSession=0xc0000203,InvalidToken=0x80090308,LogonFailure=0xc000006d,
    NoCredentials=0x8009030e,Parameters=0xc000000d,Unsupported=0xc00000bb,Resources=0xc000009a}
/// Closed error-only codec for one unsigned, standalone new/in-progress
/// attempt. No KEY, arbitrary NTSTATUS, session issuance or state transition.
/// Other established sessions and their file identities are never modified.
fn denied(c:&mut Connection,mut original:Received<'_>,why:Reason)->Result<Turn> {
    let [m]=original.frame.messages() else {return Err(Fault::SetupPhase);};
    if !setup_reply::plain_header(m) {return Err(Fault::SetupPhase);}
    let plan=c.plan_debit(&mut original.debit)?;
    let grant=plan.credit_response(0).ok_or(Fault::NoReply)?;
    let mut smb=Vec::new();smb.try_reserve_exact(72).map_err(|_|Fault::Resources)?;
    smb.extend_from_slice(m.raw.get(..64).ok_or(Fault::SetupPhase)?);
    smb[8..12].copy_from_slice(&(why as u32).to_le_bytes());smb[14..16].copy_from_slice(&grant.to_le_bytes());
    smb[16]|=1;smb[20..24].fill(0);smb.extend_from_slice(&[9,0,0,0,0,0,0,0]);
    plan.bind_owned(smb)?.send_setup((),|c,_,_|c.alive(),|c,_,_|c.alive())?;Ok(Turn::ErrorWritten)
}

#[cfg(test)]
#[path="../../../test/smb/accepted_turn.rs"]
mod tests;
