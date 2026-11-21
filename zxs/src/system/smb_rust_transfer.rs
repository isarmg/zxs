//! A real R-to-F restore on the existing ZXS kind-12 transfer. Does not
//! make R writable, replace a target or reissue a FILE, working round or Node.
use super::{Cut,DirectoryRoute,Handoff,Moved,OrdinaryRoot,Place,Route,Setting,TransferAt,TransferCut};
use crate::{FileId,OperationId,PoolId,Role,display,fs::invalid,identity::AccountId,smb::RustSessionWitness,
    system::reader::smb::{RustRetainedFile,RustStagedWriter,denied,first_work::RustReadFile,runtime::RustRoot}};
use std::{io,rc::Weak};

#[derive(Clone,Copy,PartialEq,Eq)]
struct Authority{account:AccountId,op:OperationId,epoch:i64,setting:Setting}
impl From<(AccountId,OperationId,i64,Setting)> for Authority{
    fn from((account,op,epoch,setting):(AccountId,OperationId,i64,Setting))->Self{
        Self{account,op,epoch,setting}
    }
}
struct Target{label:String,parent:DirectoryRoute,authority:Authority}

#[must_use]
pub(crate) struct RustRetainedRestore{
    actual:Moved,request:Handoff,target:Target,source:Weak<()>,
}
impl RustRoot{
    pub(crate) fn restore_retained(&self,held:&RustRetainedFile,scope:&RustSessionWitness,
        target_label:&str,parents:&[&str],name:&str,
    )->io::Result<RustRetainedRestore>{
        if !scope.is_set_info(){return Err(denied());}
        display::component(name.as_bytes(),222)?;
        let from=held.original_restore_source(self,scope)?;
        let source=self.ordinary_reader(scope)?;
        let (authority,parent)=source.at(|db,c|{
            let auth:Authority=self.ordinary_transfer_at(&TransferCut{source,db,catalog:&c},target_label)?.into();
            let parent=DirectoryRoute::by_names(db,&c,auth.setting.volume,parents)?;
            Ok((auth,parent))
        })?;
        let target=Target{label:bounded_string(target_label)?,parent,authority};
        let request=Handoff::RestoreBound{source:from.clone(),target:bounded_string(name)?};
        // The existing original transfer determines a retry from the exact
        // kind-12 SQL reservation. No MOVE of an earlier DeleteOp or target.
        let op=OperationId::new()?;
        let scope_live=||self.original_write_live(scope); // poll/try only, no nested SQLite from a write cut
        let move_after=held.restore_identity();
        let actual=target.with_transfer(self,scope,|at|
            at.transfer_inner(&request,op,None,scope_live,false,|_:Cut|Ok(())))?;
        // Do not re-open the original R name after the transfer (it no
        // longer exists). The sender will verify the exact durable receipt.
        Ok(RustRetainedRestore{actual,request,target,source:move_after})
    }
}

fn bounded_string(s:&str)->io::Result<String>{
    let mut out=String::new();
    out.try_reserve_exact(s.len()).map_err(|_|io::ErrorKind::OutOfMemory)?;
    out.push_str(s);Ok(out)
}

impl Target{
    fn with_transfer<T>(&self,tree:&RustRoot,scope:&RustSessionWitness,
        action:impl FnOnce(TransferAt<'_>)->io::Result<T>,
    )->io::Result<T>{
        let source=tree.ordinary_reader(scope)?;
        let require=|cut:&TransferCut<'_>|{
            let now:Authority=tree.ordinary_transfer_at(cut,&self.label)?.into();
            if now!=self.authority{return Err(denied());}
            let (src,db,c)=cut.parts();
            if !std::ptr::eq(src,source)||DirectoryRoute::at(db,c,now.setting.volume,self.parent.parent())?!=self.parent{
                return Err(invalid("different original restore target ancestry"));
            }
            Ok(())
        };
        let who=self.authority;
        action(TransferAt{source,setting:who.setting,parent:self.parent.parent(),destination:None,
            actor:who.account,auth:who.op,epoch:who.epoch,require:&require})
    }
}
impl RustRetainedRestore{
    pub(crate) fn require_current(&self,tree:&RustRoot,held:&RustRetainedFile,scope:&RustSessionWitness)->io::Result<()>{
        if !held.same_restore_origin(tree,&self.source,&self.actual.from){return Err(denied());}
        self.require_receipt(tree,scope)
    }
    /// Only an actual restore result has this exact original record.
    /// The protocol owner can keep it after a true related CLOSE; it is
    /// neither another operation nor a way to look up the now absent R.
    pub(crate) fn require_receipt(&self,tree:&RustRoot,scope:&RustSessionWitness)->io::Result<()>{
        if !scope.is_set_info(){return Err(denied());}
        self.target.with_transfer(tree,scope,|at|at.source.at(|db,c|{
            let expected=Route::receipt(&at,db,&c,&self.request,self.actual.op)?;
            if expected.as_ref()!=Some(&self.actual){return Err(denied());}
            Ok(())
        }))?;
        tree.ordinary_reader(scope).map(drop)
    }
}

// Stable, non-secret Core coordinates of the already qualified normal
// delete intent. Never a general SQL mutation, epoch issuer, pathname
// lookup or writable FILE handle. The existing share table owns intent
// lifetime and supplies exactly the last normal close.
#[must_use]
pub(crate) struct RustFileDelete{pool:PoolId,from:Place,target:Target}

/// Original historical kind-12 success, with no OPEN or mutating authority.
#[must_use]
pub(crate) struct ConfirmedFileDelete(Moved);
impl RustRoot{
    pub(crate) fn plan_read_file_delete(&self,held:&RustReadFile,scope:&RustSessionWitness)->io::Result<RustFileDelete>{
        if !(scope.is_create()||scope.is_set_info()){return Err(denied());}
        held.require_current(self,scope)?;
        let (place,label,parent,setting)=held.deletion_coordinates();
        self.prepare_file_delete(scope,place,label,parent,setting)
    }
    pub(crate) fn plan_writer_delete(&self,held:&RustStagedWriter,scope:&RustSessionWitness)->io::Result<RustFileDelete>{
        if !(scope.is_create()||scope.is_set_info()){return Err(denied());}
        held.require_current(self,scope)?;
        let writer=held.actual_writer();
        self.prepare_file_delete(scope,writer.original_place(),writer.label(),writer.route(),writer.setting())
    }
    fn prepare_file_delete(&self,scope:&RustSessionWitness,place:&Place,label:&str,route:&DirectoryRoute,
        setting:Setting)->io::Result<RustFileDelete>{
        let source=self.ordinary_reader(scope)?;
        if place.role!=Role::File||place.volume!=setting.volume||place.parent!=route.parent(){return Err(denied());}
        let authority=source.at(|db,c|{
            let auth:Authority=self.ordinary_transfer_at(&TransferCut{source,db,catalog:&c},label)?.into();
            if auth.setting!=setting{return Err(denied());}
            Ok(auth)
        })?;
        let target=Target{authority,label:bounded_string(label)?,parent:route.clone()};
        let plan=RustFileDelete{pool:self.pool(),from:place.clone(),target};
        plan.require_mark(self,scope)?;
        Ok(plan)
    }
}
impl RustFileDelete{
    pub(crate) fn key(&self)->(PoolId,FileId){(self.pool,self.from.file)}
    pub(crate) fn require_mark(&self,tree:&RustRoot,scope:&RustSessionWitness)->io::Result<()>{
        if !(scope.is_create()||scope.is_set_info())||tree.pool()!=self.pool{return Err(denied());}
        let ask=Handoff::Delete(self.from.clone());
        self.target.with_transfer(tree,scope,|at|self.writable_source(&at,&ask).map(drop))?;
        tree.ordinary_reader(scope).map(drop)
    }
    fn writable_source<'a>(&self,at:&TransferAt<'a>,ask:&Handoff)->io::Result<OrdinaryRoot<'a>>{
        let catalog=at.source.at(|db,c|{
            Route::select(at,db,&c,ask)?;
            Ok(c)
        })?;
        // At most this exact F/ancestry, no independent S or FILE lock.
        // Physical RO is not a write grant just because the mode says RW.
        let media=OrdinaryRoot::at_parent(at.source,&catalog,&self.target.parent,None)?;
        require_writable(&media)?;
        Ok(media)
    }
    /// The write turn still rechecks the same FILE generation, ordinary
    /// parent, credential, mode and enrolled R. Never use this from Drop
    /// or disconnect, while a write handle owns an independent FILE EX,
    /// or if the share table has not elected the last actual normal close.
    pub(crate) fn normal_last_close(&self,tree:&RustRoot,scope:&RustSessionWitness)->io::Result<ConfirmedFileDelete>{
        if !scope.is_close()||tree.pool()!=self.pool{return Err(denied());}
        let ask=Handoff::Delete(self.from.clone());
        let op=OperationId::new()?;
        self.target.with_transfer(tree,scope,|at|{
            let media=self.writable_source(&at,&ask)?;
            at.transfer_inner(&ask,op,None,||tree.original_write_live(scope),false,
                |_:Cut|require_writable(&media)).map(ConfirmedFileDelete)
        })
    }
    pub(crate) fn require_confirmation(&self,tree:&RustRoot,scope:&RustSessionWitness,receipt:&ConfirmedFileDelete)->io::Result<()>{
        if !scope.is_close()||tree.pool()!=self.pool{return Err(denied());}
        let ask=Handoff::Delete(self.from.clone());
        self.target.with_transfer(tree,scope,|at|at.source.at(|db,c|{
            if Route::receipt(&at,db,&c,&ask,receipt.0.op)?.as_ref()!=Some(&receipt.0){
                return Err(denied());
            }
            Ok(())
        }))?;
        tree.ordinary_reader(scope).map(drop)
    }
}

fn require_writable(media:&OrdinaryRoot<'_>)->io::Result<()>{
    if media.sampled_capacity()?.read_only{return Err(io::ErrorKind::ReadOnlyFilesystem.into());}
    Ok(())
}
