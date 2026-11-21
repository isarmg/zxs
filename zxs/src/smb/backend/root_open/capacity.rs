//! A size query binds the same authorized root OPEN and the actual selected
//! Core volume observation. There is no by-name rescan or fake zero volume.
use super::{CloseCompletion,RootFsError,RootOpen,RustRoot,RustSessionWitness,VerifiedMember};
use crate::{smb::wire::query_fields::CapacityFormat,system::reader::smb::runtime::{CapacityUnits,RustRootCapacity}};
use std::{io,rc::{Rc,Weak}};

#[must_use]
pub(in crate::smb) struct PreparedRootFsSize{
    // A short request does not sample any disk. None with enough room is
    // instead a positive qualification carrying a not-ready observation.
    observation:Option<RustRootCapacity>,
    scope:RustSessionWitness,
    open:Weak<()>,
    format:CapacityFormat,
    output:u32,
}
impl PreparedRootFsSize{
    pub(in crate::smb) fn values(&self)->(CapacityFormat,u32,Option<CapacityUnits>){
        (self.format,self.output,self.observation.as_ref().and_then(RustRootCapacity::allocation_units))
    }
    pub(in crate::smb) fn require_current(&self,open:&RootOpen,tree:&RustRoot)->io::Result<()>{
        if !self.open.ptr_eq(&Rc::downgrade(&open.held.original.identity))||
           !tree.matches_stamp(&open.held.original.same_tree){return Err(io::ErrorKind::PermissionDenied.into());}
        self.require_tree(tree)
    }
    pub(in crate::smb) fn finish_related_close(self,open:RootOpen,tree:&RustRoot,close:&VerifiedMember<'_,'_>)
        ->io::Result<(CloseCompletion,RetiredRootFsSize)>{
        self.require_current(&open,tree)?;
        let last=close.rust_session_witness().map_err(|_|io::ErrorKind::PermissionDenied)?;
        if !last.is_close(){return Err(io::ErrorKind::PermissionDenied.into());}
        let Some(source)=self.observation.filter(|m|m.allocation_units().is_some()) else{
            return Err(io::ErrorKind::PermissionDenied.into());
        };
        let done=open.close(tree,close).map_err(|_|io::ErrorKind::PermissionDenied)?;
        Ok((done,RetiredRootFsSize{source,query:self.scope,close:last}))
    }
    fn require_tree(&self,tree:&RustRoot)->io::Result<()>{
        if let Some(m)=&self.observation{tree.require_capacity(&self.scope,m)}
        else{tree.ensure_current(&self.scope)}
    }
}
#[must_use]
pub(in crate::smb) struct RetiredRootFsSize{
    source:RustRootCapacity,query:RustSessionWitness,close:RustSessionWitness,
}
impl RetiredRootFsSize{
    pub(in crate::smb) fn require_current(&self,tree:&RustRoot)->io::Result<()>{
        tree.ensure_current(&self.close)?;
        tree.require_capacity(&self.query,&self.source)
    }
}
impl RootOpen{
    pub(in crate::smb) fn prepare_fs_size(&self,tree:&RustRoot,member:&VerifiedMember<'_,'_>)
        ->Result<PreparedRootFsSize,RootFsError>{
        let scope=member.rust_session_witness().map_err(RootFsError::Session)?;
        if !scope.is_query_info()||!tree.matches_stamp(&self.held.original.same_tree){return Err(RootFsError::WrongTree);}
        let query=member.message().query_info(member.max_transact()).map_err(RootFsError::Fields)?;
        let format=query.class.capacity_format().ok_or(RootFsError::NotSupported)?;
        let observation=if query.output_bytes<format.bytes(){
            // The fixed buffer error still requires the original current
            // tree/OPEN, but cannot trigger fstatvfs or hold a media lane.
            tree.ensure_current(&scope).map_err(RootFsError::Original)?;
            None
        }else{Some(tree.sample_capacity(&scope,format==CapacityFormat::FullSize).map_err(RootFsError::Original)?)};
        Ok(PreparedRootFsSize{observation,scope,open:Rc::downgrade(&self.held.original.identity),format,output:query.output_bytes})
    }
}
