//! One synchronous receive turn on the original negotiated duplex. Neither
//! the caller nor a fallback can supply a frame, mutable connection or socket.
use super::{Peer,parsed};
use crate::smb::{
    connection::{Connection,Fault,Received,Result,sessions::{Resume,ordinary::Admission},setup::{Start,Why}},
    session::envelope::Failure,wire::{direct::MAX_FRAME_BYTES,setup_reply},
};
use std::time::Instant;

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::smb) enum Turn {Idle,SelectionWritten,ChallengeWritten,FinalWritten,ErrorWritten,EchoWritten,Routed}
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
impl Peer {
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
        let result=self.once(issuer,other);
        // Never skip a paid frame by dropping/forgetting an outstanding
        // operation. This owner is strictly synchronous, with no pending async I/O.
        if result.is_err() || self.connection.alive().is_err() || self.connection.in_flight!=0 {
            self.connection.disconnect();return Err(result.err().unwrap_or(Fault::Closed));
        }
        result
    }
    fn once(&mut self,issuer:Issuer,other:impl for<'c,'r> FnOnce(Outstanding<'c,'r>)->Result<()>)->Result<Turn> {
        self.housekeeping()?;
        let c=&mut self.connection;
        let limit=(u64::from(c.limits.transact)+256).min(MAX_FRAME_BYTES as u64) as usize;
        let Some(record)=c.stream.as_mut().ok_or(Fault::Transport)?.poll_record(limit)? else {
            self.housekeeping()?;return Ok(Turn::Idle);
        };
        self.housekeeping()?;
        let c=&mut self.connection;
        let frame=parsed(&record)?;let parts=frame.messages();let paid=c.receive(&frame)?;
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
