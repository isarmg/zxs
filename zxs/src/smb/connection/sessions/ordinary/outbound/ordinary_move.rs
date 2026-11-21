//! No live dispatcher. The qualifying short borrow can transfer the single
//! original debt and signer revision only into its owning peer's capture.
//! There is no interim, final reply or credit publication in this module.
use super::{CheckedBatch,Connection,Fault,Payload,VerifiedMember,root_io::RequestSlot};
use super::super::opens::Opened;
use crate::smb::{backend::{OrdinaryRenameError,PreparedCrossFilesystemMove},
    connection::{MoveDebt,Received,Result as PeerResult,accepted::incoming::MoveCapture},
    session::{self,BatchRevision,VerifiedIds},wire::{io_fields::{IoFieldError,OpenReference,WireOpenId},message::{Compound,RELATED,Routing},
        set_info::{RenameError,RenameRequest}}};

#[derive(Debug)]
pub(in crate::smb) enum MoveQualificationError{
    Connection(Fault),Malformed,FileClosed,Fields(RenameError),Original(OrdinaryRenameError),
}

#[must_use]
pub(in crate::smb) struct PreparedOriginalMove{
    request:RequestSlot,open:WireOpenId,_held:PreparedCrossFilesystemMove,
}

/// No clone, raw signer or detached debt accessor. Dropping its still-owed
/// debit marks only the original connection terminal before releasing the
/// FILE proposal. Successful finalization must be added with the real
/// original cancel/worker/rebind path, not as a generic acknowledgement.
#[must_use]
pub(in crate::smb::connection) struct MoveEscrow{
    debt:MoveDebt,ids:VerifiedIds,revision:BatchRevision,prepared:PreparedOriginalMove,
}
impl MoveEscrow{
    pub(in crate::smb::connection) fn require_original(&self,owner:&Connection,frame:&Compound<'_>)->PeerResult<()>{
        let [raw]=frame.messages()else{return Err(Fault::WrongBatch);};
        self.debt.require(owner,raw)?;
        let sid=self.ids.at(0).ok_or(Fault::SessionState)?;
        let checked=session::verify_published(frame,&self.revision,|id|owner.published_bound(id))
            .map_err(|_|Fault::SessionState)?;
        if checked.at(0)!=Some(sid){return Err(Fault::SessionState);}
        let scope=owner.member_scope(sid,&self.revision)?;
        let member=VerifiedMember{message:raw,ordinal:0,session:scope};
        if !self.prepared.request.matches(&member,sid)||
            !raw.set_info_target().is_ok_and(|id|id==OpenReference::Explicit(self.prepared.open)){
            return Err(Fault::WrongBatch);
        }
        Ok(())
    }
}

impl CheckedBatch<'_,'_>{
    pub(in crate::smb::connection) fn into_move_escrow(mut self,capture:MoveCapture<'_,'_>)->PeerResult<MoveEscrow>{
        if !capture.matches(self.original.frame){return Err(Fault::WrongBatch);}
        let prepared=self.qualify_ordinary_cross_filesystem_move().map_err(|e|match e{
            MoveQualificationError::Connection(e)=>e,
            _=>Fault::WrongReply,
        })?.ok_or(Fault::WrongReply)?;
        let Self{owner,original:Received{frame,debit},ids,revision,..}=self;
        let [raw]=frame.messages()else{return Err(Fault::WrongBatch);};
        let debt=owner.retain_move_debt(debit,raw)?;
        Ok(MoveEscrow{debt,ids,revision,prepared})
    }
    /// Deliberately not called in the synchronous turn. No `next`, credit,
    /// copy or receipt is changed here. The owning connection still owes
    /// and must retain the actual original frame, not this plan alone.
    pub(in crate::smb) fn qualify_ordinary_cross_filesystem_move(&mut self)
        ->Result<Option<PreparedOriginalMove>,MoveQualificationError>{
        use MoveQualificationError as E;
        self.before_reply().map_err(E::Connection)?;
        let [raw]=self.original.frame.messages()else{return Ok(None);};
        let Routing::Sync{tree:tid}=raw.header.routing else{return Ok(None);};
        if raw.header.flags&RELATED!=0||!raw.is_basic_rename_request(){return Ok(None);}
        let sid=self.ids.at(0).ok_or(E::Connection(Fault::SessionState))?;
        let (session,trees,opens)=self.owner.member_tables(sid,&self.revision).map_err(E::Connection)?;
        let tree=trees.root(tid).ok_or(E::Original(OrdinaryRenameError::WrongTree))?;
        let ids=match raw.set_info_target(){
            Ok(OpenReference::Explicit(ids))=>ids,
            Err(IoFieldError::NoExplicitOpen)=>return Err(E::FileClosed),
            _=>return Err(E::Malformed),
        };
        let slot=opens.find(ids).filter(|slot|slot.tree()==tid).ok_or(E::FileClosed)?;
        let Opened::File(open)=slot.object()else{return Ok(None);};
        match self.original.payload(0){
            Some(Payload::Covered)=>{},
            Some(Payload::InvalidParameter)=>return Err(E::Malformed),
            _=>return Err(E::Connection(Fault::SessionState)),
        }
        let member=VerifiedMember{message:raw,ordinal:0,session};
        let request=RenameRequest::parse(member).map_err(|(_,why)|E::Fields(why))?;
        if request.open()!=OpenReference::Explicit(ids){return Err(E::Connection(Fault::WrongReply));}
        let held=open.prepare_cross_filesystem_move(tree,&request).map_err(E::Original)?;
        Ok(held.map(|_held|PreparedOriginalMove{request:RequestSlot{sid,tree:tid,mid:raw.header.message},open:ids,_held}))
    }
}
