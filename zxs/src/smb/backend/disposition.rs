//! Delete state lives in the same true FILE share table. No storage work in
//! a mutex: only the elected last normal close can take the checked intent
//! after dropping its actual old writer/read resources.
use super::{Counts,Held,Object,OpenCoordinator,Slot,file_open::FileOpen,writable_file_open::WritableFileOpen};
use crate::{smb::{RustSessionWitness,connection::Fault,wire::set_info::{Change,FileSetRequest}},
    system::reader::smb::{ConfirmedFileDelete,RustFileDelete,runtime::RustRoot}};
use std::{io,sync::{Arc,Weak}};

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(super) enum Rule{Disposition,CloseOption}

pub(super) enum DeleteState{
    Marked{owner:Weak<()>,rule:Rule,plan:Arc<RustFileDelete>},
    Armed(Arc<RustFileDelete>),
    Retiring(Weak<RustFileDelete>),
}
impl DeleteState{
    pub(super) fn is_unarmed_on(&self,owner:&Arc<()>)->bool{
        matches!(self,Self::Marked{owner:who,..} if who.ptr_eq(&Arc::downgrade(owner)))
    }
    fn plan(&self)->Option<&Arc<RustFileDelete>>{
        match self{Self::Marked{plan,..}|Self::Armed(plan)=>Some(plan),Self::Retiring(_)=>None}
    }
}

/// The mark's relationship to the CLOSE currently holding the table.
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
enum OnClose{Unmarked,Own,Other,Armed,Retiring}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
enum Departure{Release,Arm,Move}
fn normal_departure(live:usize,bound:usize,mark:OnClose)->io::Result<Departure>{
    use OnClose::*;
    let invalid=||io::Error::from(io::ErrorKind::PermissionDenied);
    if bound==0||live<bound{return Err(invalid());}
    if mark==Unmarked{return Ok(Departure::Release);}
    if live!=bound||mark==Retiring{return Err(invalid());}
    match (bound,mark){
        (1,Own|Armed)=>Ok(Departure::Move),
        (1,Other)=>Err(invalid()),
        (_,Own)=>Ok(Departure::Arm),
        _=>Ok(Departure::Release),
    }
}

/// Just the resulting original table state for a signed reply. It cannot
/// keep an OPEN or active queued work alive if that OPEN was abandoned.
#[must_use]
pub(super) struct Snapshot{owner:Weak<()>,plan:Option<Weak<RustFileDelete>>}
impl Snapshot{
    pub(super) fn verify(&self,slot:&Slot)->io::Result<Option<Arc<RustFileDelete>>>{
        if !self.owner.ptr_eq(&Arc::downgrade(&slot.identity)){return Err(io::ErrorKind::PermissionDenied.into());}
        slot.with_counts(|row|{
            match (&self.plan,&row.deletion){
                (None,None)=>Ok(None),
                (Some(expected),Some(active))if active.is_unarmed_on(&slot.identity)=>{
                    let current=active.plan().ok_or(io::ErrorKind::PermissionDenied)?;
                    if !expected.ptr_eq(&Arc::downgrade(current)){return Err(io::ErrorKind::PermissionDenied.into());}
                    Ok(Some(Arc::clone(current)))
                },
                _=>Err(io::ErrorKind::PermissionDenied.into()),
            }
        })
    }
}

impl Slot{
    fn with_counts<T>(&self,edit:impl FnOnce(&mut Counts)->io::Result<T>)->io::Result<T>{
        if !self.bound||self.released{return Err(io::ErrorKind::PermissionDenied.into());}
        let mut table=self.owner.table.lock().map_err(|_|io::Error::other("unavailable original FILE share table"))?;
        edit(table.objects.get_mut(&self.object).ok_or(io::ErrorKind::PermissionDenied)?)
    }
    pub(super) fn pending_delete(&self)->io::Result<bool>{
        self.with_counts(|row|Ok(row.deletion.is_some()))
    }
    pub(super) fn require_publishable_file(&self,tree:&RustRoot,scope:&RustSessionWitness)->io::Result<()>{
        let plan=self.with_counts(|row|match &row.deletion{
            None=>Ok(None),
            Some(mark @ DeleteState::Marked{rule:Rule::CloseOption,plan,..})
                if mark.is_unarmed_on(&self.identity)=>Ok(Some(Arc::clone(plan))),
            _=>Err(io::ErrorKind::WouldBlock.into()),
        })?;
        if let Some(plan)=plan{plan.require_mark(tree,scope)?;}
        Ok(())
    }
    pub(super) fn mark_delete(&self,plan:RustFileDelete,rule:Rule)->io::Result<Snapshot>{
        let (pool,file)=plan.key();
        if self.object!=Object::File(pool,file){return Err(io::ErrorKind::PermissionDenied.into());}
        self.with_counts(|row|{
            if row.renaming.is_some(){return Err(io::ErrorKind::WouldBlock.into());}
            if let Some(already)=&row.deletion{
                if !already.is_unarmed_on(&self.identity){return Err(io::ErrorKind::WouldBlock.into());}
                // Repeating the same owner's positive setting is idempotent;
                // never replace its original checked plan or drop a close option.
                if let DeleteState::Marked{rule:Rule::Disposition,..}=already{
                    if rule==Rule::CloseOption{return Err(io::ErrorKind::PermissionDenied.into());}
                }
                return Ok(Snapshot{owner:Arc::downgrade(&self.identity),plan:already.plan().map(Arc::downgrade)});
            }
            // A provisional share reservation is not yet an OPEN. Do not
            // mark across a FILE acquisition that still might be published.
            // A legacy R read left over after a restore participates in
            // sharing, but its CLOSE cannot authorize a fresh ordinary
            // move of its successor. Wait for such old reads to retire.
            if row.retained!=0||row.bound!=row.live||row.denies[2]>usize::from(self.vote.denies&super::D!=0){
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let plan=Arc::new(plan);
            let receipt=Snapshot{owner:Arc::downgrade(&self.identity),plan:Some(Arc::downgrade(&plan))};
            row.deletion=Some(DeleteState::Marked{owner:Arc::downgrade(&self.identity),rule,plan});
            Ok(receipt)
        })
    }
    pub(super) fn clear_disposition(&self)->io::Result<Snapshot>{
        self.with_counts(|row|{
            if row.renaming.is_some(){return Err(io::ErrorKind::WouldBlock.into());}
            if let Some(state)=&row.deletion {
                if !matches!(state,DeleteState::Marked{rule:Rule::Disposition,..})||
                    !state.is_unarmed_on(&self.identity){return Err(io::ErrorKind::PermissionDenied.into());}
            }
            row.deletion=None;
            Ok(Snapshot{owner:Arc::downgrade(&self.identity),plan:None})
        })
    }
    /// Call after dropping this original storage object, only if explicit
    /// CLOSE succeeded (including its actual dirty FLUSH). Counting and
    /// arming/electing are one critical section: two concurrent closes
    /// cannot both think the other is last.
    pub(super) fn finish_normal(mut self)->io::Result<Option<LastClose>>{
        if !matches!(self.object,Object::File(..))||self.retained||!self.bound||self.released{
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let owner=Arc::clone(&self.owner);
        let mut table=owner.table.lock().map_err(|_|io::Error::other("unavailable original FILE share table"))?;
        let row=table.objects.get_mut(&self.object).ok_or(io::ErrorKind::PermissionDenied)?;
        if row.renaming.is_some(){return Err(io::ErrorKind::WouldBlock.into());}
        let mark=match &row.deletion{
            None=>OnClose::Unmarked,
            Some(m @ DeleteState::Marked{..})=>if m.is_unarmed_on(&self.identity){OnClose::Own}else{OnClose::Other},
            Some(DeleteState::Armed(_))=>OnClose::Armed,
            Some(DeleteState::Retiring(_))=>OnClose::Retiring,
        };
        let step=normal_departure(row.live,row.bound,mark)?;
        let plan=if step==Departure::Release{None}else{
            let authority=row.deletion.as_ref().and_then(DeleteState::plan).ok_or(io::ErrorKind::PermissionDenied)?;
            Some(Arc::clone(authority))
        };
        row.remove(self.vote,&self.identity);
        row.bound-=1;
        self.released=true;
        if let Some(plan)=plan{
            if step==Departure::Move{
                // Keep the existing seat as a barrier until one original
                // transfer returns, even if the original F is now offline.
                row.deletion=Some(DeleteState::Retiring(Arc::downgrade(&plan)));
                drop(table);
                return Ok(Some(LastClose{owner,object:self.object,plan}));
            }
            row.deletion=Some(DeleteState::Armed(plan));
        }
        let empty=row.live==0;
        if empty{table.objects.remove(&self.object);}
        table.live-=1;
        Ok(None)
    }
}

#[must_use]
pub(super) struct LastClose{owner:Arc<OpenCoordinator>,object:Object,plan:Arc<RustFileDelete>}
impl LastClose{
    pub(super) fn move_to_recycle(self,tree:&RustRoot,scope:&RustSessionWitness)->io::Result<ClosedDeletion>{
        let actual=self.plan.normal_last_close(tree,scope)?;
        Ok(ClosedDeletion{plan:Arc::clone(&self.plan),actual})
    }
}
impl Drop for LastClose{
    fn drop(&mut self){
        if let Ok(mut table)=self.owner.table.lock(){
            let exact=table.objects.get(&self.object).is_some_and(|row|
                row.live==0&&row.bound==0&&matches!(&row.deletion,
                    Some(DeleteState::Retiring(mark)) if mark.ptr_eq(&Arc::downgrade(&self.plan))));
            if exact {table.objects.remove(&self.object);table.live-=1;}
        }
    }
}

/// A completion has no FILE hold and must use the same original CLOSE.
#[must_use]
pub(super) struct ClosedDeletion{plan:Arc<RustFileDelete>,actual:ConfirmedFileDelete}
impl ClosedDeletion{
    pub(super) fn require_current(&self,tree:&RustRoot,scope:&RustSessionWitness)->io::Result<()>{
        self.plan.require_confirmation(tree,scope,&self.actual)
    }
}

pub(super) const DELETE_ACCESS:u32=0x0001_0000;
pub(super) const DELETE_ON_CLOSE:u32=0x0000_1000;

#[derive(Debug)]
pub(in crate::smb) enum DispositionError{
    NotSupported,AccessDenied,WrongTree,Session(Fault),Original(io::Error),
}
#[must_use]
pub(in crate::smb) struct PreparedDisposition{state:Snapshot,scope:RustSessionWitness}
impl PreparedDisposition{
    pub(in crate::smb) fn require_file(&self,open:&FileOpen,tree:&RustRoot)->io::Result<()>{
        open.require_disposition(tree,&self.scope,&self.state)
    }
    pub(in crate::smb) fn require_writer(&self,open:&WritableFileOpen,tree:&RustRoot)->io::Result<()>{
        open.require_disposition(tree,&self.scope,&self.state)
    }
}
impl<H> Held<H>{
    pub(super) fn set_disposition(&self,tree:&RustRoot,request:&FileSetRequest<'_,'_>,
        current:impl FnOnce(&H,&RustSessionWitness)->io::Result<()>,
        plan:impl FnOnce(&H,&RustSessionWitness)->io::Result<RustFileDelete>,
    )->Result<PreparedDisposition,DispositionError>{
        if self.grant.0&DELETE_ACCESS==0{return Err(DispositionError::AccessDenied);}
        let scope=request.original().rust_session_witness().map_err(DispositionError::Session)?;
        if !scope.is_set_info(){return Err(DispositionError::NotSupported);}
        tree.ensure_current(&scope).map_err(DispositionError::Original)?;
        current(&self.original,&scope).map_err(DispositionError::Original)?;
        let state=match request.change(){
            Change::MarkForDelete=>self._slot.mark_delete(plan(&self.original,&scope).map_err(DispositionError::Original)?,Rule::Disposition),
            Change::ClearDisposition=>self._slot.clear_disposition(),
            _=>return Err(DispositionError::NotSupported),
        }.map_err(DispositionError::Original)?;
        Ok(PreparedDisposition{state,scope})
    }
    pub(super) fn require_disposition(&self,tree:&RustRoot,scope:&RustSessionWitness,snapshot:&Snapshot)->io::Result<()>{
        if self.grant.0&DELETE_ACCESS==0{return Err(io::ErrorKind::PermissionDenied.into());}
        if let Some(exact)=snapshot.verify(&self._slot)?{exact.require_mark(tree,scope)?;}
        Ok(())
    }
    pub(super) fn enroll_close_option(&self,plan:RustFileDelete)->io::Result<()>{
        if self.grant.0&DELETE_ACCESS==0{return Err(io::ErrorKind::PermissionDenied.into());}
        self._slot.mark_delete(plan,Rule::CloseOption).map(drop)
    }
}

#[cfg(test)]
#[path="../../../tests/smb/backend_disposition.rs"]
mod tests;
