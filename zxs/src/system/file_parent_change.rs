//! Same ordinary volume, two actually installed parents. The origin is
//! still the already-open sole native FILE and the one kind-14 decision.
//! Neither this path nor its cold recovery issues a second business file.
use super::*;
use super::ordinary_directory::Route;
use crate::{fs::WitnessRead,ordinary::portable_birth::{
    FlatParentMove,ParentMoveStep,PackedParentMove,PackedParentStep,lock_parent_pair}};
use super::work_record::Members;

enum Moving<'a>{Flat(FlatParentMove<'a>),Packed(PackedParentMove<'a>)}
impl<'a> Moving<'a>{
    fn hold(original:&Original,birth:&'a Birth,from:&'a HeldDir,to:&'a HeldDir,stage:RenameStage,members:&Members)->io::Result<Self>{
        let check=|w:&WitnessRead<'_>|members.require(w);
        if let Some(id)=original.from.backing{
            Ok(Self::Packed(PackedParentMove::hold(birth,id,from,to,&original.from.name,&original.name,stage,check)?))
        }else{
            Ok(Self::Flat(FlatParentMove::hold(birth,from,to,&original.from.name,&original.name,stage,check)?))
        }
    }
    fn original(&self,opened:&Installed)->io::Result<()>{
        match self{Self::Flat(x)=>x.require_original(opened),Self::Packed(x)=>x.require_original(opened)}
    }
    fn require_reservable(&self,old:&HeldDir,new:&HeldDir)->io::Result<()>{
        match self{Self::Flat(x)=>x.require_reservable(old,new),Self::Packed(x)=>x.require_reservable(old,new)}
    }
    fn at_old(&self)->bool{
        match self{Self::Flat(x)=>x.observed()==ParentMoveStep::Old,Self::Packed(x)=>x.observed()==PackedParentStep::Old}
    }
    fn install<T>(self,mut admit:impl FnMut()->io::Result<()>,mut point:impl FnMut()->io::Result<()>,
        finish:impl FnOnce(&Installed)->io::Result<T>)->io::Result<(Installed,T)>{
        match self{Self::Flat(x)=>x.install(|_|admit(),|_|point(),finish),Self::Packed(x)=>x.install(|_|admit(),|_|point(),finish)}
    }
    fn restore(self,mut admit:impl FnMut()->io::Result<()>,finish:impl FnOnce(&Installed)->io::Result<()>)->io::Result<()>{
        match self{
            Self::Flat(x)=>x.restore(|_|admit(),|_|Ok(()),finish)?,
            Self::Packed(x)=>x.restore(|_|admit(),|_|Ok(()),finish)?,
        };
        Ok(())
    }
}
fn routes(db:&Connection,c:&Catalog,original:&Original,from:&Route,to:&Route)->io::Result<()>{
    if Route::at(db,c,original.from.volume,original.from.parent)?!=*from||
       Route::at(db,c,original.from.volume,original.to_parent)?!=*to{return Err(denied());}
    Ok(())
}
fn physical_parents(from:OrdinaryView<'_>,to:&OrdinaryRoot<'_>)->io::Result<()>{
    from.require_streaming()?;to.require_streaming()
}

impl<'s,'r> Tree<'s,'r>{
    pub(in crate::system::reader::smb) fn move_existing_parent(&self,root:&OrdinaryRoot<'r>,expected:&Place,opened:&Installed,
        parts:&[String],op:OperationId,name:&str,alive:&impl Fn()->io::Result<()>,
        point:impl Fn(Cut)->io::Result<()>)->io::Result<(Place,Installed,OrdinaryRoot<'r>)>{
        let source=self.session.source;let owner=&self.session.original;
        let require=|cut:&RenameCut<'_>|{
            debug_assert!(std::ptr::eq(cut.source,source));
            self.require(cut.db,cut.catalog,true)
        };
        let at=RenameAt{source,setting:self.setting,parent:self.parent,
            actor:owner.account,auth:owner.op,epoch:owner.generation,require:&require};
        let moved=at.move_between(root.view(),expected,opened,parts,op,name,alive,point)?;
        Ok((moved.renamed.place,moved.renamed.body,moved.target))
    }
}

/// Returned only by the single shared kind14 engine after actual confirmed
/// source/target authority and physical movement. The target remains held.
pub(super) struct Moved<'s>{
    pub(super) renamed:Renamed,pub(super) target:OrdinaryRoot<'s>,pub(super) route:Route,
}
impl<'s> RenameAt<'s,'_>{
    pub(super) fn move_between(&self,root:OrdinaryView<'_>,expected:&Place,opened:&Installed,
        parts:&[impl AsRef<str>],op:OperationId,name:&str,alive:&impl Fn()->io::Result<()>,
        point:impl Fn(Cut)->io::Result<()>)->io::Result<Moved<'s>>{
        display::component(name.as_bytes(),222)?;
        let src=self.source;let grant=self.setting;
        if expected.role!=Role::File||(expected.volume,expected.parent)!=(grant.volume,self.parent)||
           !root.for_tree(src,grant.volume)||root.parent()!=self.parent{return Err(denied());}
        let (catalog,original,birth,source,target,members,alias)=src.at(|db,c|{
            self.authorized(db,&c)?;alive()?;
            let selected=Place::current(db,&c,expected.file)?;
            if &selected!=expected{return Err(denied());}
            Place::require_no_rename(db,expected.file)?;
            let source=Route::at(db,&c,grant.volume,self.parent)?;
            let target=Route::by_names(db,&c,grant.volume,parts)?;
            if target.parent()==self.parent{return Err(invalid("a two-parent move cannot use the original parent"));}
            vacant(db,&c,expected,target.parent(),name)?;
            let b=selected.bind(db,&c)?.birth;
            let r=c.rows().get(1).filter(|r|r.role==Role::Recycle).ok_or_else(denied)?;
            b.validate_transfer_geometry(name,(&r.path,r.limit),op)?;
            let original=Original{op,from:expected.clone(),to_parent:target.parent(),name:name.to_owned(),
                share:grant.share,setting:grant.op,auth:self.auth,actor:self.actor,epoch:self.epoch,
                save:work_record::selected(db,&b)?.map(|r|r.record.key().save),
                manual:work_record::latest_commit(db,&b)?.map(|n|n.0)};
            original.digest(&b)?;
            let members=work_record::members(db,&c,&b)?;
            let alias=file_create::display_owner_at(db,&c,grant.volume,target.parent(),name)?;
            Ok((c,original,b,source,target,members,alias))
        })?;
        let to=OrdinaryRoot::at_parent(src,&catalog,&target,None)?;
        physical_parents(root,&to)?;
        let requested=Moving::hold(&original,&birth,root.folder(),to.folder(),RenameStage::BeforeReserve,&members)?;
        requested.original(opened)?;
        point(Cut::ParentSampled)?;
        // Under both namespace locks, bill the destination without removing
        // the source charge. The pending tuple keeps both across restarts.
        let (a,b)=lock_parent_pair(root.folder(),to.folder())?;
        let from_names=census(&a)?;let toward=census(&b)?;
        let target_alias=directory::check_alias_name(&to,name,alias.as_ref(),Some(&toward))?;
        let destination=original.target();
        let terminal=destination.leaf()?;let outside=birth.witness()?;
        let pocket=destination.private_leaf()?;
        let more=if let Some(p)=pocket.as_ref(){Additional::Pocket(p)}else{Additional::IncomingFlat(&outside,&terminal)};
        physical_parents(root,&to)?;requested.original(opened)?;
        let w=Connection::open(&src.dir,true)?;
        // Reconfirm the actual target raw vacancy under both namespace
        // locks. The accounting census alone cannot tell a promised entry
        // from a foreign terminal, witness or pocket which has just arrived.
        requested.require_reservable(&a,&b)?;
        let attempt=src.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
            self.authorized(db,&c)?;
            routes(db,&c,&original,&source,&target)?;
            if Place::current(db,&c,expected.file)?!=*expected||
                work_record::members(db,&c,&birth)?!=members||
                file_create::display_owner_at(db,&c,grant.volume,target.parent(),name)?!=alias{return Err(busy());}
            vacant(db,&c,expected,target.parent(),name)?;
            ordinary_directory::budget(db,grant.volume,expected.parent,&from_names,None)?;
            ordinary_directory::budget(db,grant.volume,target.parent(),&toward,Some(more))?;
            alive()?;original.issue(db,&birth)
        });
        drop(w);drop(target_alias);drop(b);drop(a);
        if let Err(error)=attempt{
            match src.at(|db,_|Original::fetch(db,&birth,op)){
                Ok(None)=>return Err(error),
                Ok(Some((same,Status::Pending))) if same==original=>{},
                _=>return Err(unknown()),
            }
        }
        // After issuing, any lost evidence can be outcome-unknown. There
        // is no new operation or retry by following an occupied name.
        let execute=(||->io::Result<(Place,Installed)>{
            point(Cut::Reserved)?;
            let admit=||{
                physical_parents(root,&to)?;
                src.at(|db,c|{
                    self.authorized(db,&c)?;routes(db,&c,&original,&source,&target)?;
                    original.require_exact(db,&birth,Status::Pending)?;original.live(db)?;
                    alive()
                })?;
                alive() // nothing blocking between this and the physical syscall
            };
            let (actual,())=requested.install(admit,||point(Cut::NameChanged),|actual|{
                physical_parents(root,&to)?;actual.require(to.folder())?;
                members.require(&WitnessRead::at(actual.directory())?)?;
                let _alias=directory::check_alias_name(&to,name,alias.as_ref(),None)?;
                let w=Connection::open(&src.dir,true)?;
                let result=src.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
                    self.authorized(db,&c)?;routes(db,&c,&original,&source,&target)?;
                    original.require_exact(db,&birth,Status::Pending)?;original.live(db)?;
                    if file_create::display_owner_at(db,&c,grant.volume,target.parent(),name)?!=alias{return Err(busy());}
                    alive()?;original.confirm(db)?;
                    if Place::current(db,&c,expected.file)?!=destination{return Err(denied());}Ok(())
                });
                drop(w);
                if result.is_err(){
                    src.at(|db,c|{
                        self.authorized(db,&c)?;routes(db,&c,&original,&source,&target)?;
                        original.require_exact(db,&birth,Status::Confirmed)?;
                        if Place::current(db,&c,expected.file)?!=destination{return Err(denied());}Ok(())
                    })?;
                }
                Ok(())
            })?;
            point(Cut::Confirmed)?;
            Ok((destination,actual))
        })().map_err(|_|unknown())?;
        Ok(Moved{renamed:Renamed{place:execute.0,body:execute.1,proof:ExactRename(original)},target:to,route:target})
    }
}

pub(super) fn recover(src:&InitialReader,original:&Original,kind:Recovery,birth:&Birth,
    from:&OrdinaryRoot<'_>,to:&OrdinaryRoot<'_>,source:&Route,target:&Route,members:&Members,
    old_alias:Option<&file_create::Alias>,new_alias:Option<&file_create::Alias>,allowed:&Allowed<'_>,
    alive:&impl Fn()->io::Result<()>)->io::Result<()>{
    physical_parents(from.view(),to)?;
    let step=Moving::hold(original,birth,from.folder(),to.folder(),RenameStage::Reserved,members)?;
    let untouched=step.at_old();let forward=!untouched&&kind==Recovery::Continue;
    let (there,name,parent,alias)=if forward{(to,&original.name,original.to_parent,new_alias)}
        else{(from,&original.from.name,original.from.parent,old_alias)};
    let admit=||{
        physical_parents(from.view(),to)?;
        src.at(|db,c|{
            routes(db,&c,original,source,target)?;
            if original.recovery(db,&c,birth,allowed)?!=kind{return Err(denied());}
            alive()
        })?;
        alive()
    };
    let terminal=|actual:&Installed|{
        physical_parents(from.view(),to)?;actual.require(there.folder())?;
        members.require(&WitnessRead::at(actual.directory())?)?;
        let _alias=directory::check_alias_name(there,name,alias,None)?;
        let w=Connection::open(&src.dir,true)?;
        src.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
            routes(db,&c,original,source,target)?;
            if original.recovery(db,&c,birth,allowed)?!=kind||
                file_create::display_owner_at(db,&c,original.from.volume,parent,name)?.as_ref()!=alias{return Err(denied());}
            alive()?;
            let position=if forward{original.confirm(db)?;original.target()}else{
                let (setting,auth)=match kind{Recovery::Continue=>(original.setting,original.auth),
                    Recovery::Undo{setting,auth}=>(setting,auth)};
                original.abort(db,setting,auth,if untouched{Abort::Untouched}else{Abort::RolledBack})?;
                original.from.clone()
            };
            if Place::current(db,&c,original.from.file)?!=position{return Err(denied());}Ok(())
        })
    };
    admit()?;
    if forward{step.install(admit,||Ok(()),terminal)?;}else{step.restore(admit,terminal)?;}
    Ok(())
}
