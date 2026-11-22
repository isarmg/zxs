//! Owns exactly one asynchronous storage worker for this paid peer turn.
//! Only the peer can adopt the result, sign or settle the original debt.
use super::{Arc,Connection,Fault,MoveFlight,Origin,OriginalMoveInput,Peer,PendingOriginalMove,Result,Weak,
    move_control::{ControlInput,ControlTurn,MoveControl}};
use crate::{smb::{RustSessionWitness,backend::MoveOpenWitness,connection::{
    sessions::ordinary::{CheckWrittenMove,MoveWritten,WorkingMoveEscrow},transport::OriginalPeerProbe}},
    system::reader::smb::{CheckedMoveTarget,ConfirmedOrdinaryMove,MoveTargetKey,first_work::RustAdoptedMove,runtime::QualifiedRustInstall}};
use std::{io,os::fd::{AsFd,AsRawFd,FromRawFd,OwnedFd},
    sync::{atomic::{AtomicU8,Ordering},mpsc::{self,Receiver,SyncSender,TryRecvError}},thread::{self,JoinHandle}};

const WORKING:u8=0;
const CANCELLED:u8=1;
const LOST:u8=2;
const SELECTING:u8=3;
const FINISHED:u8=4;

struct Gate(AtomicU8);
impl Gate{
    fn new()->Self{Self(AtomicU8::new(WORKING))}
    fn flip(&self,to:u8)->std::result::Result<u8,u8>{
        self.0.compare_exchange(WORKING,to,Ordering::AcqRel,Ordering::Acquire)
    }
    fn require(&self,state:u8)->io::Result<()>{
        if self.0.load(Ordering::Acquire)==state{Ok(())}
        else{Err(io::ErrorKind::Interrupted.into())}
    }
    fn cancel(&self)->ControlTurn{
        match self.flip(CANCELLED){
            Ok(_)|Err(CANCELLED)=>ControlTurn::CancelRequested,
            Err(_)=>ControlTurn::TooLate,
        }
    }
    fn abandon(&self){let _=self.flip(LOST);}
    fn select(&self)->io::Result<()>{
        self.flip(SELECTING).map(drop).map_err(|_|io::ErrorKind::Interrupted.into())
    }
    fn finish(&self)->u8{self.flip(FINISHED).unwrap_or_else(|last|last)}
}

#[must_use]
pub(crate) struct OriginalMoveFence<'a>{
    gate:&'a Gate,origin:&'a Weak<Origin>,file:&'a MoveOpenWitness,peer:&'a OriginalPeerProbe,
}
impl OriginalMoveFence<'_>{
    fn require_original_alive(&self)->io::Result<()>{
        if crate::smb::launch::stopping()||
            self.origin.upgrade().is_none_or(|o|o.closed.load(Ordering::Acquire)){
            Err(io::ErrorKind::Interrupted.into())
        }else{Ok(())}
    }
    fn check_progress_at(&self,state:u8)->io::Result<()>{
        self.gate.require(state)?;
        self.require_original_alive()?;
        self.file.require()?;
        self.gate.require(state)
    }
    pub(crate) fn check_progress(&self)->io::Result<()>{self.check_progress_at(WORKING)}
    fn check_member_at(&self,state:u8,member:&RustSessionWitness,installed:&QualifiedRustInstall)->io::Result<()>{
        self.check_progress_at(state)?;
        member.require_write_turn_inside(installed)?;
        // The credential check above does not sample the global TERM latch.
        self.require_original_alive()?;
        self.gate.require(state)
    }
    pub(crate) fn check_member(&self,member:&RustSessionWitness,installed:&QualifiedRustInstall)->io::Result<()>{
        self.check_member_at(WORKING,member,installed)
    }
    /// After actual selection but while this worker verifies/holds target.
    /// A late signed CANCEL cannot convert the placement to cancellation.
    pub(crate) fn check_selected_member(&self,member:&RustSessionWitness,installed:&QualifiedRustInstall)->io::Result<()>{
        self.check_member_at(SELECTING,member,installed)
    }
    /// Inside the common Core's final before-COMMIT hook only. The worker
    /// cannot read TCP. After the membership/OPEN check, take one actual
    /// zero-wait observation of the same socket before the only choice.
    /// Success is not an acknowledgement or a promise it stays connected.
    pub(crate) fn before_placement(&self,member:&RustSessionWitness,installed:&QualifiedRustInstall)->io::Result<()>{
        self.check_member(member,installed)?;
        if self.peer.observe().is_err(){
            self.gate.abandon();
            return Err(io::ErrorKind::Interrupted.into());
        }
        self.require_original_alive()?;
        self.gate.select()
    }
}

/// These outcomes are private observations, not client statuses. Even an
/// observed veto need not settle an earlier reservation or remove a stage.
struct WorkerTarget{held:ConfirmedOrdinaryMove,file:MoveOpenWitness}
enum WorkerOutcome{Confirmed(WorkerTarget),CancelledBeforeSelection,Lost,Stopped(io::ErrorKind),Unknown}
impl WorkerOutcome{
    fn from_core(state:u8,result:io::Result<WorkerTarget>)->Self{
        match (state,result){
            (SELECTING,Ok(original))=>Self::Confirmed(original),
            (_,Err(e)) if e.kind()==io::ErrorKind::ConnectionAborted=>Self::Unknown,
            (CANCELLED,Err(_))=>Self::CancelledBeforeSelection,
            (LOST,Err(_))=>Self::Lost,
            (WORKING,Err(error))=>Self::Stopped(error.kind()),
            // Any error after entering selection or unexpected success
            // without it is not a definite negative; no automatic replay.
            _=>Self::Unknown,
        }
    }
}

/// Cannot be constructed from a CANCEL packet, a caller flag or an
/// expected target. Issued only for the actual original worker's final
/// cancellation report, not while its selection decision can still race.
#[must_use]
pub(in crate::smb::connection) struct WorkerCancelled<'a>(&'a OriginalMoveInput);
impl<'a> WorkerCancelled<'a>{
    pub(in crate::smb::connection) fn original_input(&self)->&'a OriginalMoveInput{self.0}
}

enum CheckTarget{Before(MoveTargetKey),After(CheckWrittenMove)}
enum WorkerEvent{Initial(WorkerOutcome),Before(CheckedMoveTarget),After(CheckWrittenMove,CheckedMoveTarget),Unknown}
enum OwnerCompletion{
    Waiting,Worker(WorkerOutcome),Before(RustAdoptedMove),After(RustAdoptedMove,MoveWritten),Settled,
}
// Only a readiness hint, never the receipt or a TCP capability.
// The same capacity-one channel remains the authoritative worker result.
struct CompletionWake(Arc<OwnedFd>);
struct WakePoster(Arc<OwnedFd>);
impl CompletionWake{
    fn pair()->Result<(Self,WakePoster)>{
        // SAFETY: fixed Linux event counter, initially zero and nonblocking.
        let fd=unsafe{libc::eventfd(0,libc::EFD_CLOEXEC|libc::EFD_NONBLOCK)};
        if fd<0{return Err(Fault::Resources);}
        // SAFETY: successful eventfd creates one new fd owned here.
        let fd=Arc::new(unsafe{OwnedFd::from_raw_fd(fd)});
        Ok((Self(Arc::clone(&fd)),WakePoster(fd)))
    }
    fn clear(&self){
        // SAFETY: one bounded read on our nonblocking event counter only.
        let mut value=[0u8;8];
        let _=unsafe{libc::read(self.0.as_raw_fd(),value.as_mut_ptr().cast(),value.len())};
    }
}
impl WakePoster{
    fn notify(&self){
        // SAFETY: our same private counter fd and exactly one native
        // eventfd value. Never wait/retry; the bounded poll fallback
        // also observes a missing hint or panic through the real channel.
        let one=1u64.to_ne_bytes();
        let _=unsafe{libc::write(self.0.as_raw_fd(),one.as_ptr().cast(),one.len())};
    }
}
#[must_use]
pub(in crate::smb::connection::accepted) struct WorkingOriginalMove{
    thread:Option<JoinHandle<()>>,gate:Arc<Gate>,result:Receiver<WorkerEvent>,done:OwnerCompletion,wake:CompletionWake,
    checks:Option<SyncSender<CheckTarget>>,
    // Retain actual shared FILE turn and original unpaid debt through join.
    original:WorkingMoveEscrow,input:OriginalMoveInput,control:MoveControl,
}
impl WorkingOriginalMove{
    fn begin(c:&mut Connection,pending:PendingOriginalMove)->Result<Self>{
        let stream=c.stream.as_ref().ok_or(Fault::Transport)?;
        stream.require_pending_peer()?;
        let peer=stream.original_peer_probe()?;
        let (wake,post)=CompletionWake::pair()?;
        let PendingOriginalMove{mut held,input,control}=pending;
        // Copy can take an unknown duration. The verified original request
        // receives one complete interim before the worker is ever started.
        // Failure after that closes this original, without a mock result.
        held.send_interim(c,&input)?;
        let (original,work,file)=held.assign_worker(c,&input)?;
        let origin=Arc::downgrade(&c.origin);
        let gate=Arc::new(Gate::new());
        let active=Arc::clone(&gate);
        let (send,result)=mpsc::sync_channel(1);
        let (checks,requested)=mpsc::sync_channel(1);
        // Only the real bound operation, weak original liveness and its
        // share witness enter with a non-owning, poll-only TCP observation.
        // No stream access, signing key, SQLite or Rc view.
        let thread=thread::Builder::new().spawn(move||{
            let publish=|event|{
                if send.try_send(event).is_err(){return false;}
                post.notify();true
            };
            let fence=OriginalMoveFence{gate:&active,origin:&origin,file:&file,peer:&peer};
            let mut verifier=None;
            let done=work.run(&fence).map(|(held,check)|{
                verifier=Some(check);WorkerTarget{held,file:file.clone()}
            });
            let initial=WorkerOutcome::from_core(active.finish(),done);
            let ready=matches!(&initial,WorkerOutcome::Confirmed(_));
            if !publish(WorkerEvent::Initial(initial))||!ready{return;}
            let Some(verifier)=verifier else{let _=publish(WorkerEvent::Unknown);return;};
            // Retain the exact target and independent S. No SQL or root
            // namespace lock spans either wait; a closed sender ends it.
            let Ok(CheckTarget::Before(key))=requested.recv()else{return;};
            let Ok(before)=verifier.verify(&fence,&key)else{let _=publish(WorkerEvent::Unknown);return;};
            if !publish(WorkerEvent::Before(before)){return;}
            let Ok(CheckTarget::After(written))=requested.recv()else{return;};
            let Ok(after)=verifier.verify(&fence,written.target())else{let _=publish(WorkerEvent::Unknown);return;};
            let _=publish(WorkerEvent::After(written,after));
        }).map_err(|_|Fault::Resources)?;
        Ok(Self{thread:Some(thread),gate,result,done:OwnerCompletion::Waiting,wake,checks:Some(checks),original,input,control})
    }
    pub(super) fn control_once(&mut self,c:&mut Connection)->Result<ControlTurn>{
        self.wake.clear();
        let turn=self.control_step(c)?;
        if turn!=ControlTurn::Waiting||!self.awaits_worker(){return Ok(turn);}
        self.control.wait_for_worker_or_input(c,self.wake.0.as_fd())?;
        self.wake.clear();
        // At most two receive quanta/observations per turn; even a full
        // socket can't starve the real channel, and the hint is not proof.
        self.control_step(c)
    }
    fn control_step(&mut self,c:&mut Connection)->Result<ControlTurn>{
        self.original.require_session(c)?;
        let mut turn=match self.control.receive(c)?{
            ControlInput::Waiting=>ControlTurn::Waiting,
            ControlInput::OrdinaryHeld=>ControlTurn::OrdinaryHeld,
            ControlInput::Cancel(r)=>{
                if self.original.matches_cancel(c,&self.input,&r)?{self.gate.cancel()}
                else{ControlTurn::NoTarget}
            },
        };
        // Read at most one original input step, then observe only this
        // worker. A ready result cannot be starved by ordinary input.
        if self.awaits_worker(){
            match self.result.try_recv(){
                Ok(ready)=>{turn=self.worker_report(c,ready)?;},
                Err(TryRecvError::Empty)=>{},
                Err(TryRecvError::Disconnected)=>return Err(Fault::WriteOutcomeUnknown),
            }
        }
        match self.done{
            OwnerCompletion::Worker(WorkerOutcome::CancelledBeforeSelection)=>{
                self.original.send_worker_cancelled(c,WorkerCancelled(&self.input))?;
                Ok(ControlTurn::Cancelled)
            }
            OwnerCompletion::Worker(WorkerOutcome::Lost)=>Err(Fault::Closed),
            OwnerCompletion::Worker(WorkerOutcome::Unknown)=>Err(Fault::WriteOutcomeUnknown),
            // No phase-specific public refusal has been proved for this
            // storage failure. Never guess a final body or replay.
            OwnerCompletion::Worker(WorkerOutcome::Stopped(_))=>Err(Fault::NoReply),
            _=>Ok(turn),
        }
    }
    fn awaits_worker(&self)->bool{
        matches!(self.done,OwnerCompletion::Waiting|OwnerCompletion::Before(_)|OwnerCompletion::After(..))
    }
    fn ask(&self,check:CheckTarget)->Result<()>{
        self.checks.as_ref().ok_or(Fault::WriteOutcomeUnknown)?.try_send(check).map_err(|_|Fault::WriteOutcomeUnknown)
    }
    fn worker_report(&mut self,c:&mut Connection,event:WorkerEvent)->Result<ControlTurn>{
        use OwnerCompletion as Phase;
        let unknown=|_|Fault::WriteOutcomeUnknown;
        match (std::mem::replace(&mut self.done,Phase::Settled),event){
            (Phase::Waiting,WorkerEvent::Initial(result))=>{
                self.done=Phase::Worker(result);Ok(ControlTurn::WorkerFinished)
            },
            (Phase::Before(adopted),WorkerEvent::Before(early))=>{
                let sent=self.original.write_confirmed(c,&self.input,&adopted,early).map_err(unknown)?;
                self.ask(CheckTarget::After(sent.for_readback()))?;
                self.done=Phase::After(adopted,sent);
                Ok(ControlTurn::Waiting)
            },
            (Phase::After(adopted,sent),WorkerEvent::After(report,late))=>{
                self.original.settle_confirmed(c,&self.input,&adopted,sent,report,late).map_err(unknown)?;
                Ok(ControlTurn::MoveSettled)
            },
            _=>Err(Fault::WriteOutcomeUnknown),
        }
    }
    fn adopt_ready(&mut self,c:&mut Connection)->Result<bool>{
        use OwnerCompletion::Worker;
        let unknown=|_|Fault::WriteOutcomeUnknown;
        match self.done{
            Worker(WorkerOutcome::Unknown)=>return Err(Fault::WriteOutcomeUnknown),
            Worker(WorkerOutcome::Confirmed(_))=>{},
            _=>return Ok(false),
        }
        c.stream.as_ref().ok_or(Fault::WriteOutcomeUnknown)?.require_pending_peer().map_err(unknown)?;
        let Worker(WorkerOutcome::Confirmed(WorkerTarget{held,file}))=
            std::mem::replace(&mut self.done,OwnerCompletion::Waiting)
            else{return Err(Fault::WriteOutcomeUnknown);};
        let rebound=self.original.adopt_destination(c,&self.input,file,held).map_err(unknown)?;
        c.stream.as_ref().ok_or(Fault::WriteOutcomeUnknown)?.require_pending_peer().map_err(unknown)?;
        self.ask(CheckTarget::Before(rebound.target().clone()))?;
        self.done=OwnerCompletion::Before(rebound);
        // Still owns the original debit/input/share turn; no reply yet.
        Ok(true)
    }
    pub(super) fn abandon(&self){self.gate.abandon();}
    pub(super) fn drain(&mut self){
        drop(self.checks.take()); // release either parked worker before join
        if let Some(thread)=self.thread.take(){let _=thread.join();}
    }
    pub(super) fn take_control_after_terminal(&mut self)->MoveControl{
        self.drain();
        std::mem::take(&mut self.control)
    }
}
impl Drop for WorkingOriginalMove{
    fn drop(&mut self){self.abandon();self.drain();}
}

impl Peer{
    pub(super) fn adopt_original_move(&mut self)->Result<bool>{
        let Some(MoveFlight::Working(working))=&mut self.original_move else{return Err(Fault::WrongBatch);};
        let result=working.adopt_ready(&mut self.connection);
        if result.is_err(){self.disconnect_original();}
        result
    }
    pub(super) fn start_original_move_worker(&mut self)->Result<()>{
        let pending=match self.original_move.take(){
            Some(MoveFlight::Unstarted(parked))=>parked,
            other=>{self.original_move=other;return Err(Fault::WrongBatch);},
        };
        match WorkingOriginalMove::begin(&mut self.connection,pending){
            Ok(active)=>{self.original_move=Some(MoveFlight::Working(active));Ok(())},
            Err(e)=>{self.disconnect_original();Err(e)},
        }
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/attached/smb/connection/accepted/incoming/move_worker/tests.rs"]
mod tests;
