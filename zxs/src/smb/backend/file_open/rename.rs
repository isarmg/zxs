//! Same-parent FILE rename through the actual read-like DELETE OPEN.
//! No path resolution of its source and no write-buffer side effects.
use super::{CloseCompletion,DELETE_ACCESS,FileOpen,Held,VerifiedMember};
use super::super::{Counts,Object,OpenCoordinator,Slot,path::{NameError,TreeName}};
use crate::{FileId,PoolId,fs::MAX_PATH_DEPTH,smb::{RustSessionWitness,connection::Fault,
    wire::{io_fields::OpenReference,message::{RELATED,Routing},path_fields::FieldError,set_info::RenameRequest}},
    system::reader::smb::{ConfirmedOrdinaryMove,first_work::{RustAdoptedMove,RustOrdinaryMove,RustOrdinaryRename,RustRetiredOrdinaryRename},runtime::RustRoot}};
use std::{io,sync::{Arc,Weak}};

#[derive(Debug)]
pub(in crate::smb) enum OrdinaryRenameError{
    WrongTree,AccessDenied,Name(NameError),SharingViolation,DeletePending,Session(Fault),Original(io::Error),
}

fn same_parent_leaf<'n,'p>(path:&'n TreeName,rooted:bool,parent:impl Iterator<Item=&'p str>)
    ->Result<&'n str,NameError>{
    ordinary_parent_leaf(path,rooted,parent,222) // ordinary FILE physical terminal budget
}
pub(in crate::smb::backend) fn ordinary_parent_leaf<'n,'p>(path:&'n TreeName,rooted:bool,
    parent:impl Iterator<Item=&'p str>,maximum:usize)->Result<&'n str,NameError>{
    let mut parts=path.components();
    let leaf=parts.next_back().ok_or(NameError::InvalidName)?;
    let first=parts.next();
    // A complete tree-relative spelling is merely another spelling of
    // this holder's own parent, not a name lookup or a destination grant.
    if (rooted||first.is_some())&&!first.into_iter().chain(parts).eq(parent){
        return Err(NameError::Field(FieldError::Unsupported));
    }
    if leaf.len()>maximum{return Err(NameError::TooLong);}
    Ok(leaf)
}

pub(crate) enum FileRenameTarget<'a>{
    SameParent(&'a str),
    OtherParent{label:&'a str,parents:Vec<&'a str>,name:&'a str},
}
#[derive(Debug,Eq,PartialEq)]
struct NativeMoveName<'a>{label:&'a str,parents:Vec<&'a str>,name:&'a str}
const NATIVE_MOVE_RELATIVE_BYTES:usize=4096;
fn file_target<'n,'p>(path:&'n TreeName,rooted:bool,replace:bool,standalone:bool,label:&str,
    parent:impl Iterator<Item=&'p str>)->Result<FileRenameTarget<'n>,NameError>{
    match same_parent_leaf(path,rooted,parent){
        Ok(leaf)=>return Ok(FileRenameTarget::SameParent(leaf)),
        Err(NameError::Field(FieldError::Unsupported)) if standalone&&!rooted&&!replace=>{},
        Err(e)=>return Err(e),
    }
    // Native's narrow explicit MOVE form, not general SMB path resolution.
    // The only permitted label is the source OPEN's own enrolled F.
    let NativeMoveName{label,parents,name}=native_move_name(path,|s|s==label)?;
    Ok(FileRenameTarget::OtherParent{label,parents,name})
}

fn native_move_name(path:&TreeName,accept_label:impl FnOnce(&str)->bool)->Result<NativeMoveName<'_>,NameError>{
    let mut parts=path.components();
    let name=parts.next_back().ok_or(NameError::InvalidName)?;
    let label=parts.next().filter(|s|accept_label(s)).ok_or(NameError::Field(FieldError::Unsupported))?;
    if name.len()>222{return Err(NameError::TooLong);}
    // The Native relative part excludes the F label. Its depth includes
    // the terminal FILE name and its byte budget includes separators.
    let (mut depth,mut bytes)=(1, name.len());
    for part in parts{
        if depth>=MAX_PATH_DEPTH{return Err(NameError::TooLong);}
        depth+=1;
        bytes=bytes.checked_add(1+part.len()).filter(|&n|n<=NATIVE_MOVE_RELATIVE_BYTES).ok_or(NameError::TooLong)?;
    }
    let mut parents=Vec::new();
    parents.try_reserve_exact(depth-1).map_err(|_|NameError::Resources)?;
    parents.extend(path.components().skip(1).take(depth-1));
    Ok(NativeMoveName{label,parents,name})
}

fn cross_filesystem_name<'a>(path:&'a TreeName,rooted:bool,replace:bool,source_label:&str)
    ->Result<Option<NativeMoveName<'a>>,NameError>{
    let mut parts=path.components();
    let first=parts.next().ok_or(NameError::InvalidName)?;
    if parts.next().is_none()||first==source_label{return Ok(None);}
    if rooted||replace{return Err(NameError::Field(FieldError::Unsupported));}
    // This is only spelling. The exact target label may be missing, from
    // R, not writable, or another registration on the same filesystem.
    native_move_name(path,|label|label!=source_label).map(Some)
}

fn one_original(row:&Counts)->Result<(),OrdinaryRenameError>{
    if row.deletion.is_some(){return Err(OrdinaryRenameError::DeletePending);}
    if row.live!=1||row.bound!=1||row.retained!=0||row.renaming.is_some(){
        return Err(OrdinaryRenameError::SharingViolation);
    }
    Ok(())
}

/// In the one real OPEN share table. Neither FILE nor directory peers may
/// start a competing view. No mutex or SQL lock stays held in this turn.
#[must_use]
pub(in crate::smb::backend) struct RenameTurn{table:Arc<OpenCoordinator>,object:Object,open:Weak<()>,token:Arc<()>,retired:bool}
impl RenameTurn{
    fn take(slot:&Slot)->Result<Self,OrdinaryRenameError>{
        if !matches!(slot.object,Object::File(..)){return Err(OrdinaryRenameError::AccessDenied);}
        Self::reserve(slot)
    }
    pub(in crate::smb::backend) fn for_directory(slot:&Slot)->Result<Self,OrdinaryRenameError>{
        if !matches!(slot.object,Object::Directory(..)){return Err(OrdinaryRenameError::AccessDenied);}
        Self::reserve(slot)
    }
    pub(in crate::smb::backend) fn represents(&self,object:Object)->bool{self.object==object&&!self.retired}
    fn reserve(slot:&Slot)->Result<Self,OrdinaryRenameError>{
        if slot.released||!slot.bound||slot.retained{
            return Err(OrdinaryRenameError::AccessDenied);
        }
        let mut table=slot.owner.table.lock()
            .map_err(|_|OrdinaryRenameError::Original(io::Error::other("unavailable original ordinary sharing state")))?;
        let row=table.objects.get_mut(&slot.object).ok_or(OrdinaryRenameError::AccessDenied)?;
        one_original(row)?;
        let token=Arc::new(());
        row.renaming=Some(Arc::downgrade(&token));
        Ok(Self{table:Arc::clone(&slot.owner),object:slot.object,open:Arc::downgrade(&slot.identity),token,retired:false})
    }
    fn exact(&self,row:&Counts)->bool{same_rename_row(row,&Arc::downgrade(&self.token))}
    fn count(&self,row:&Counts,n:usize)->bool{count_rename_row(row,&Arc::downgrade(&self.token),n)}
    fn same_open(&self,slot:&Slot)->bool{
        Arc::ptr_eq(&self.table,&slot.owner)&&self.object==slot.object&&!slot.released&&slot.bound&&!slot.retained&&
            self.open.ptr_eq(&Arc::downgrade(&slot.identity))
    }
    pub(in crate::smb::backend) fn require(&self,slot:&Slot)->io::Result<()>{
        if self.retired||!self.same_open(slot){return Err(io::ErrorKind::PermissionDenied.into());}
        let table=self.table.table.lock().map_err(|_|io::Error::other("unavailable original ordinary sharing state"))?;
        if !table.objects.get(&self.object).is_some_and(|row|self.count(row,1)){
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(())
    }
    /// Same explicit CLOSE after its original physical FILE was released.
    /// Retain this row in the global quota only as an immutable send fence.
    fn retire(&mut self,mut slot:Slot)->io::Result<()>{
        if self.retired||!self.same_open(&slot){return Err(io::ErrorKind::PermissionDenied.into());}
        let mut table=self.table.table.lock().map_err(|_|io::Error::other("unavailable original FILE sharing state"))?;
        let row=table.objects.get_mut(&self.object).ok_or(io::ErrorKind::PermissionDenied)?;
        if !self.count(row,1){return Err(io::ErrorKind::PermissionDenied.into());}
        row.remove(slot.vote);row.bound=0;
        slot.released=true;
        self.retired=true;
        Ok(())
    }
    fn require_retired(&self)->io::Result<()>{
        let table=self.table.table.lock().map_err(|_|io::Error::other("unavailable original FILE sharing state"))?;
        if !self.retired||!table.objects.get(&self.object).is_some_and(|row|self.count(row,0)){
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(())
    }
}
fn same_rename_row(row:&Counts,token:&Weak<()>)->bool{
    row.retained==0&&row.deletion.is_none()&&row.renaming.as_ref().is_some_and(|p|p.ptr_eq(token))
}
fn count_rename_row(row:&Counts,token:&Weak<()>,n:usize)->bool{
    same_rename_row(row,token)&&row.live==n&&row.bound==n
}
impl Drop for RenameTurn{
    fn drop(&mut self){
        if let Ok(mut table)=self.table.table.lock(){
            if self.retired{
                let owns=table.objects.get(&self.object).is_some_and(|row|self.count(row,0));
                if owns{table.objects.remove(&self.object);table.live-=1;}
            }else if let Some(row)=table.objects.get_mut(&self.object){
                if self.exact(row){row.renaming=None;}
            }
        }
    }
}

/// Only a same-share, signed SET plus the original FILE's sole held
/// slot can construct this short input to the common Core. No numeric
/// selector or reconstructed path stands in for either authority.
pub(crate) struct OrdinaryRenameAdmission<'a>{
    turn:&'a RenameTurn,scope:&'a RustSessionWitness,target:&'a FileRenameTarget<'a>,
}

/// No caller can create this from bare FILE/volume IDs or a Core root.
/// It borrows the actual standalone SET and its same original share turn.
pub(crate) struct OrdinaryMoveAdmission<'a>{
    turn:&'a RenameTurn,scope:RustSessionWitness,target:&'a NativeMoveName<'a>,
}
impl<'a> OrdinaryMoveAdmission<'a>{
    pub(crate) fn into_original(self,pool:PoolId,file:FileId)
        ->io::Result<(RustSessionWitness,&'a str,&'a [&'a str],&'a str)>{
        if !self.turn.represents(Object::File(pool,file))||!self.scope.is_set_info(){
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok((self.scope,self.target.label,&self.target.parents,self.target.name))
    }
}

/// Pending-owner input, never a storage execution or a reply. Release
/// the read-only Core proof before dropping its exclusive share turn.
#[must_use]
pub(in crate::smb) struct PreparedCrossFilesystemMove{
    binding:RustOrdinaryMove,turn:RenameTurn,
}
/// Only the original peer retains the real share turn. The worker receives
/// neither this owner nor the original mutable OPEN.
#[must_use]
pub(in crate::smb) struct WorkingMoveGuard{_turn:RenameTurn}
#[derive(Clone)] // only existing weak identities, no holder or FILE access
pub(in crate::smb) struct MoveOpenWitness{
    table:Weak<OpenCoordinator>,object:Object,open:Weak<()>,turn:Weak<()>,
}
impl MoveOpenWitness{
    pub(in crate::smb) fn require(&self)->io::Result<()>{
        use io::ErrorKind::{PermissionDenied,WouldBlock};
        use std::sync::TryLockError;
        if self.open.strong_count()==0||self.turn.strong_count()==0{return Err(PermissionDenied.into());}
        let table=self.table.upgrade().ok_or(PermissionDenied)?;
        let rows=table.table.try_lock().map_err(|e|io::Error::from(match e{
            TryLockError::WouldBlock=>WouldBlock,TryLockError::Poisoned(_)=>PermissionDenied,
        }))?;
        if !rows.objects.get(&self.object).is_some_and(|row|count_rename_row(row,&self.turn,1)){
            return Err(PermissionDenied.into());
        }
        Ok(())
    }
}
impl WorkingMoveGuard{
    /// No target/file I/O or permission to mutate. The paid original
    /// member and its still-published original OPEN must survive output.
    pub(in crate::smb) fn require_original_open(&self,open:&FileOpen,tree:&RustRoot,member:&RustSessionWitness)
        ->io::Result<()>{
        if !member.is_set_info(){return Err(io::ErrorKind::PermissionDenied.into());}
        self._turn.require_move_open(open,tree)
    }
    pub(in crate::smb) fn require_adopted(&self,open:&FileOpen,tree:&RustRoot,member:&RustSessionWitness,
        adopted:&RustAdoptedMove)->io::Result<()>{
        self.require_original_open(open,tree,member)?;
        open.held.original.require_adopted_move(tree,member,adopted)?;
        self.require_original_open(open,tree,member)
    }
    /// The private worker returns its *same* original witness. Check both
    /// its exact share round and the actual published original read OPEN
    /// before consuming the target already held by that worker.
    pub(in crate::smb) fn adopt(&self,open:&mut FileOpen,tree:&RustRoot,member:&RustSessionWitness,
        witness:MoveOpenWitness,target:ConfirmedOrdinaryMove)->io::Result<RustAdoptedMove>{
        let turn=&self._turn;
        let unknown=||io::Error::from(io::ErrorKind::ConnectionAborted);
        if turn.retired||!turn.same_open(&open.held._slot)||open.held.grant.0&DELETE_ACCESS==0||
            turn.object!=witness.object||!witness.table.ptr_eq(&Arc::downgrade(&turn.table))||
            !witness.open.ptr_eq(&turn.open)||!witness.turn.ptr_eq(&Arc::downgrade(&turn.token)){
            return Err(unknown());
        }
        witness.require().map_err(|_|unknown())?;
        open.held.original.adopt_ordinary_move(tree,member,target)
    }
}
impl RenameTurn{
    fn require_move_open(&self,open:&FileOpen,tree:&RustRoot)->io::Result<()>{
        use io::ErrorKind::{PermissionDenied,WouldBlock};
        use std::sync::TryLockError;
        if !open.held.original.belongs_to(tree)||open.held.grant.0&DELETE_ACCESS==0||
            self.retired||!self.same_open(&open.held._slot){return Err(PermissionDenied.into());}
        let table=self.table.table.try_lock().map_err(|e|io::Error::from(match e{
            TryLockError::WouldBlock=>WouldBlock,TryLockError::Poisoned(_)=>PermissionDenied,
        }))?;
        if !table.objects.get(&self.object).is_some_and(|row|self.count(row,1)){
            return Err(PermissionDenied.into());
        }
        Ok(())
    }
}
impl PreparedCrossFilesystemMove{
    /// Check the original open and move turn before canceling queued work.
    pub(in crate::smb) fn require_original_open(&self,open:&FileOpen,tree:&RustRoot,member:&RustSessionWitness)
        ->io::Result<()>{
        if !self.binding.is_original_member(member){return Err(io::ErrorKind::PermissionDenied.into());}
        self.turn.require_move_open(open,tree)
    }
    pub(in crate::smb) fn hand_to_worker(self)->(RustOrdinaryMove,WorkingMoveGuard,MoveOpenWitness){
        let Self{binding,turn,..}=self;
        let witness=MoveOpenWitness{table:Arc::downgrade(&turn.table),object:turn.object,open:turn.open.clone(),
            turn:Arc::downgrade(&turn.token)};
        (binding,WorkingMoveGuard{_turn:turn},witness)
    }
}
impl OrdinaryRenameAdmission<'_>{
    pub(crate) fn held(&self,pool:PoolId,file:FileId)->io::Result<(&RustSessionWitness,&FileRenameTarget<'_>)>{
        if self.turn.object!=Object::File(pool,file)||!self.scope.is_set_info(){
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok((self.scope,self.target))
    }
}

#[must_use]
pub(in crate::smb) struct PreparedOrdinaryRename{
    turn:RenameTurn,scope:RustSessionWitness,proof:RustOrdinaryRename,following:Option<usize>,
}
impl PreparedOrdinaryRename{
    pub(in crate::smb) fn require_current(&self,open:&FileOpen,tree:&RustRoot)->io::Result<()>{
        if open.held.grant.0&DELETE_ACCESS==0{return Err(io::ErrorKind::PermissionDenied.into());}
        self.turn.require(&open.held._slot)?;
        open.held.original.require_rename(tree,&self.scope,&self.proof)
    }
    /// The sender must pick an actual related CLOSE from the same verified
    /// frame and the immediately preceding original's own already-held FILE.
    /// Failure after the preceding durable rename is a terminal send fault.
    pub(in crate::smb) fn finish_related_close(
        self,open:FileOpen,tree:&RustRoot,close:&VerifiedMember<'_,'_>,
    )->io::Result<(CloseCompletion,RetiredOrdinaryRename)>{
        if self.following!=Some(close.ordinal())||
            !close.message().close_request().is_ok_and(|x|x.open==OpenReference::PreviousInCompound){
            return Err(io::ErrorKind::ConnectionAborted.into());
        }
        let unknown=|_|io::Error::from(io::ErrorKind::ConnectionAborted);
        let closing=close.rust_session_witness().map_err(|_|io::Error::from(io::ErrorKind::ConnectionAborted))?;
        if !closing.is_close()||!closing.same_session(&self.scope){return Err(io::ErrorKind::ConnectionAborted.into());}
        tree.ensure_current(&closing).map_err(unknown)?;
        self.require_current(&open,tree).map_err(unknown)?;
        let Self{mut turn,scope,proof,..}=self;
        let Held{original,_slot,..}=open.held;
        let proof=original.retire_rename(tree,&scope,proof).map_err(unknown)?;
        turn.retire(_slot).map_err(unknown)?;
        let retired=RetiredOrdinaryRename{turn,scope,closing,proof};
        let close=CloseCompletion::after_ordinary_rename(&retired);
        Ok((close,retired))
    }
}

/// This is not an OPEN or a way to acquire one. It proves the same durable
/// request and quit state until the exact related original is fully written.
#[must_use]
pub(in crate::smb) struct RetiredOrdinaryRename{
    turn:RenameTurn,scope:RustSessionWitness,closing:RustSessionWitness,proof:RustRetiredOrdinaryRename,
}
impl RetiredOrdinaryRename{
    pub(in crate::smb) fn require_current(&self,tree:&RustRoot)->io::Result<()>{
        self.turn.require_retired()?;
        tree.ensure_current(&self.closing)?;
        self.proof.require_current(tree,&self.scope)
    }
}

impl FileOpen{
    /// The connection must first look up the exact original two-half OPEN
    /// in the original TREE and invoke this on that actual read holder.
    pub(in crate::smb) fn prepare_cross_filesystem_move(&self,tree:&RustRoot,request:&RenameRequest<'_,'_>)
        ->Result<Option<PreparedCrossFilesystemMove>,OrdinaryRenameError>{
        let m=request.original().message();
        if request.original().ordinal()!=0||m.header.flags&RELATED!=0||!matches!(m.header.routing,Routing::Sync{..})||
            m.raw.get(20..24)!=Some(&[0;4][..])||!matches!(request.open(),OpenReference::Explicit(_)){
            return Err(OrdinaryRenameError::Name(NameError::Field(FieldError::Unsupported)));
        }
        let Some(target)=cross_filesystem_name(request.destination(),request.had_root_separator(),
            request.requested_replace(),self.held.original.original_folder_label()).map_err(OrdinaryRenameError::Name)?
            else{return Ok(None);};
        if !self.held.original.belongs_to(tree){return Err(OrdinaryRenameError::WrongTree);}
        if self.held.grant.0&DELETE_ACCESS==0{return Err(OrdinaryRenameError::AccessDenied);}
        let scope=request.original().rust_session_witness().map_err(OrdinaryRenameError::Session)?;
        let turn=RenameTurn::take(&self.held._slot)?;
        turn.require(&self.held._slot).map_err(OrdinaryRenameError::Original)?;
        let admission=OrdinaryMoveAdmission{turn:&turn,scope,target:&target};
        let binding=tree.bind_ordinary_move(&self.held.original,admission).map_err(OrdinaryRenameError::Original)?;
        Ok(Some(PreparedCrossFilesystemMove{binding,turn}))
    }

    pub(in crate::smb) fn rename(&mut self,tree:&RustRoot,request:&RenameRequest<'_,'_>)
        ->Result<PreparedOrdinaryRename,OrdinaryRenameError>{
        self.rename_checked(tree,request,false)
    }
    /// Only the already verified single-message SET_INFO lane calls this.
    /// The related three-command path still accepts same-parent names only.
    pub(in crate::smb) fn rename_standalone(&mut self,tree:&RustRoot,request:&RenameRequest<'_,'_>)
        ->Result<PreparedOrdinaryRename,OrdinaryRenameError>{
        if !matches!(request.open(),OpenReference::Explicit(_)){
            return Err(OrdinaryRenameError::Name(NameError::Field(FieldError::Unsupported)));
        }
        self.rename_checked(tree,request,true)
    }
    fn rename_checked(&mut self,tree:&RustRoot,request:&RenameRequest<'_,'_>,standalone:bool)
        ->Result<PreparedOrdinaryRename,OrdinaryRenameError>{
        if !self.held.original.belongs_to(tree){return Err(OrdinaryRenameError::WrongTree);}
        if self.held.grant.0&DELETE_ACCESS==0{return Err(OrdinaryRenameError::AccessDenied);}
        let scope=request.original().rust_session_witness().map_err(OrdinaryRenameError::Session)?;
        if !scope.is_set_info(){return Err(OrdinaryRenameError::AccessDenied);}
        let target=file_target(request.destination(),request.had_root_separator(),request.requested_replace(),
            standalone,self.held.original.original_folder_label(),
            self.held.original.original_parent_components()).map_err(OrdinaryRenameError::Name)?;
        // Neither lane may overwrite. The old same-parent lane can ignore
        // the advisory bit; the new lane requires the Native no-replace form.
        let turn=RenameTurn::take(&self.held._slot)?;
        let admission=OrdinaryRenameAdmission{turn:&turn,scope:&scope,target:&target};
        let proof=self.held.original.rename_original(tree,&admission).map_err(OrdinaryRenameError::Original)?;
        let following=if request.open()==OpenReference::PreviousInCompound{
            request.original().ordinal().checked_add(1)
        }else{None};
        Ok(PreparedOrdinaryRename{turn,scope,proof,following})
    }
}

#[cfg(test)]
#[path="../../../../tests/smb/backend_ordinary_rename.rs"]
mod tests;
