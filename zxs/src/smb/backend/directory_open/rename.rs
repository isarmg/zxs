//! Same-parent relabel of an existing, actual directory DELETE OPEN.
//! No source pathname reopen, second authority, or replacement grant.
use super::{DELETE_ACCESS,DirectoryOpen,Object};
use super::super::file_open::{OrdinaryRenameError,RenameTurn,ordinary_parent_leaf};
use crate::{DirectoryId,PoolId,display,
    smb::{RustSessionWitness,wire::{io_fields::OpenReference,set_info::RenameRequest}},
    system::reader::smb::{directory::RustDirectoryRename,runtime::RustRoot}};
use std::io;

pub(crate) struct OrdinaryDirectoryRenameAdmission<'a>{
    turn:&'a RenameTurn,scope:&'a RustSessionWitness,target:&'a str,
}
impl OrdinaryDirectoryRenameAdmission<'_>{
    pub(crate) fn held(&self,pool:PoolId,id:DirectoryId)->io::Result<(&RustSessionWitness,&str)>{
        if !self.scope.is_set_info()||!self.turn.represents(Object::Directory(pool,id)){
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok((self.scope,self.target))
    }
}

#[must_use]
pub(in crate::smb) struct PreparedDirectoryRename{
    turn:RenameTurn,scope:RustSessionWitness,proof:RustDirectoryRename,
}
impl PreparedDirectoryRename{
    pub(in crate::smb) fn require_current(&self,open:&DirectoryOpen,tree:&RustRoot)->io::Result<()>{
        if open.held.grant.0&DELETE_ACCESS==0{return Err(io::ErrorKind::PermissionDenied.into());}
        self.turn.require(&open.held._slot)?;
        open.held.original.require_rename(tree,&self.scope,&self.proof)
    }
}

impl DirectoryOpen{
    pub(in crate::smb) fn rename_ordinary(&mut self,tree:&RustRoot,request:&RenameRequest<'_,'_>)
        ->Result<PreparedDirectoryRename,OrdinaryRenameError>{
        if !self.held.original.belongs_to(tree){return Err(OrdinaryRenameError::WrongTree);}
        if self.held.grant.0&DELETE_ACCESS==0||self.held.original.nested().is_none(){
            return Err(OrdinaryRenameError::AccessDenied);
        }
        let scope=request.original().rust_session_witness().map_err(OrdinaryRenameError::Session)?;
        if !matches!(request.open(),OpenReference::Explicit(_))||!scope.is_set_info(){
            return Err(OrdinaryRenameError::Original(io::ErrorKind::Unsupported.into()));
        }
        let target=ordinary_parent_leaf(request.destination(),request.had_root_separator(),
            self.held.original.original_parent_components(),display::MAX).map_err(OrdinaryRenameError::Name)?;
        // No apparent success for a request that this exact native engine
        // cannot carry. Same-name no-op is outside this first Rust lane.
        if self.held.original.original_leaf()==Some(target){
            return Err(OrdinaryRenameError::Original(io::ErrorKind::Unsupported.into()));
        }
        let turn=RenameTurn::for_directory(&self.held._slot)?;
        turn.require(&self.held._slot).map_err(OrdinaryRenameError::Original)?;
        let admission=OrdinaryDirectoryRenameAdmission{turn:&turn,scope:&scope,target};
        // ReplaceIfExists is not authority. Both SQLite reservation and
        // the original physical rename still require no replacement.
        let proof=self.held.original.rename_in_parent(tree,&admission).map_err(OrdinaryRenameError::Original)?;
        Ok(PreparedDirectoryRename{turn,scope,proof})
    }
}
