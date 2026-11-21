//! Currency of the exact live protocol session and its actual startup.
//! No key, client-supplied SID, numeric constructor or storage route.
use super::VerifiedMember;
use super::super::Claim;
use crate::{smb::{InheritedRun,connection::{Fault,Origin,Result}},system::reader::smb::runtime::QualifiedRustInstall};
use std::{io,sync::{Arc,TryLockError,Weak,atomic::Ordering}};

/// Borrowed session rechecks may seed an owned tree with this weak mark.
/// It neither keeps a session open nor authorizes another command. Every
/// operation must derive its own mark from its original verified member.
#[must_use]
pub(crate) struct RustSessionWitness {
    source:Weak<InheritedRun>,
    origin:Weak<Origin>,
    claim:Weak<Claim>,
    command:u16,
}
impl VerifiedMember<'_,'_> {
    pub(in crate::smb) fn rust_session_witness(&self)->Result<RustSessionWitness>{
        let view=self.session();
        view.recheck()?;
        let service=view.origin.run.verifier.as_ref().ok_or(Fault::NoVerifier)?;
        let source=view.revision.original_start(service).ok_or(Fault::NoVerifier)?;
        let marked=view.reg.claim.valid.get().and_then(|v|v.original_start(service)).ok_or(Fault::SessionState)?;
        if !Arc::ptr_eq(&source,&marked){return Err(Fault::SessionState);}
        Ok(RustSessionWitness{source:Arc::downgrade(&source),origin:Arc::downgrade(view.origin),
            claim:Arc::downgrade(&view.reg.claim),command:self.message().header.command})
    }
}
impl RustSessionWitness {
    pub(crate) fn is_tree_connect(&self)->bool{self.command==3}
    pub(crate) fn is_create(&self)->bool{self.command==5}
    pub(crate) fn is_close(&self)->bool{self.command==6}
    pub(crate) fn is_flush(&self)->bool{self.command==7}
    pub(crate) fn is_read(&self)->bool{self.command==8}
    pub(crate) fn is_write(&self)->bool{self.command==9}
    pub(crate) fn is_query_directory(&self)->bool{self.command==14}
    pub(crate) fn is_query_info(&self)->bool{self.command==16}
    pub(crate) fn is_set_info(&self)->bool{self.command==17}
    /// Identity comparison only: even two expired references can be the
    /// same session. Do not substitute this for either current check.
    pub(crate) fn same_session(&self,other:&Self)->bool{
        self.source.ptr_eq(&other.source)&&self.origin.ptr_eq(&other.origin)&&self.claim.ptr_eq(&other.claim)
    }
    /// Progress only for the actual original READ: no credential lock,
    /// SQLite, file/socket data I/O or claim of client TCP liveness.
    /// The original admission and full pre/late proofs still authorize.
    pub(crate) fn require_read_progress(&self,install:&QualifiedRustInstall)->io::Result<()>{
        use io::ErrorKind::{Interrupted,PermissionDenied};
        if !self.is_read(){return Err(PermissionDenied.into());}
        let (Some(start),Some(origin),Some(claim))=(self.source.upgrade(),self.origin.upgrade(),self.claim.upgrade())
            else{return Err(PermissionDenied.into());};
        if !claim.origin.ptr_eq(&self.origin)||claim.valid.get().is_none()||!install.is_original_start(&start){
            return Err(PermissionDenied.into());
        }
        let still=||!origin.closed.load(Ordering::Acquire)&&!crate::smb::launch::stopping();
        if !still(){return Err(Interrupted.into());}
        start.require_live_source()?;
        if !still(){return Err(Interrupted.into());}
        Ok(())
    }
    /// Cheap liveness for an already authenticated FILE birth/STORE turn.
    /// The actual engine still validates its Core epoch/setting/parent
    /// inside the same transaction. No waiting, SQL, disk, socket I/O or
    /// new authorization can be performed through this scope.
    pub(crate) fn require_write_turn_inside(&self,install:&QualifiedRustInstall)->io::Result<()> {
        use io::ErrorKind::{PermissionDenied,WouldBlock};
        if !matches!(self.command,5|6|7|9|17){return Err(PermissionDenied.into());}
        let (Some(source),Some(origin),Some(claim))=(self.source.upgrade(),self.origin.upgrade(),self.claim.upgrade())
            else{return Err(PermissionDenied.into());};
        if !claim.origin.ptr_eq(&self.origin)||origin.closed.load(Ordering::Acquire)||
            !install.is_original_start(&source){return Err(PermissionDenied.into());}
        let service=origin.run.verifier.as_ref().ok_or(PermissionDenied)?;
        let table=origin.run.index.try_lock().map_err(|err|io::Error::from(match err {
            TryLockError::WouldBlock=>WouldBlock,
            TryLockError::Poisoned(_)=>PermissionDenied,
        }))?;
        if !table.live.iter().any(|entry|entry.ptr_eq(&self.claim)){
            return Err(PermissionDenied.into());
        }
        claim.valid.get().ok_or(PermissionDenied)?.require_held_revision(service,&source)?;
        if origin.closed.load(Ordering::Acquire){return Err(PermissionDenied.into());}
        Ok(())
    }
    /// Short in-memory live check of the same original session on the
    /// selected Core installation. Actual Core epoch, media and object
    /// checks remain necessary and none of these locks survive this call.
    pub(crate) fn is_current_on(&self,install:&QualifiedRustInstall)->bool{
        let (Some(source),Some(origin),Some(claim))=(self.source.upgrade(),self.origin.upgrade(),self.claim.upgrade())
            else{return false;};
        if !claim.origin.ptr_eq(&self.origin)||origin.closed.load(Ordering::Acquire)||
            !install.is_original_start(&source){return false;}
        let Some(service)=origin.run.verifier.as_ref()else{return false;};
        let Some(current)=claim.valid.get().and_then(|m|m.original_start(service))else{return false;};
        if !Arc::ptr_eq(&source,&current){return false;}
        let registered=origin.run.index.lock().is_ok_and(|table|table.live.iter().any(|m|m.ptr_eq(&self.claim)));
        registered&&!origin.closed.load(Ordering::Acquire)&&source.require_live_source().is_ok()
    }
}
