//! Before the native decision only the original SYSTEM allocation may bind
//! and name target material. A surviving private name is never a FILE.
use super::{allocation::{MoveAllocation,Stage},destination::{Destination,Method},placed,source::{Bytes,Leaf,Verified},transfer::{Moved,Sites}};
use crate::{fs::{self,HeldDir,invalid,stream::PinnedRegular},ordinary::{OrdinaryParentLease as HeldParent,allocation,material,wire::NativeObject,Cut},
    pool::lease::{Access,LocalPool}};
use std::{collections::{BTreeMap,BTreeSet},io};
pub(super) struct Existing{stage:Stage,dir:HeldDir,allocation:[u8;32],parts:BTreeMap<Leaf,(Bytes,PinnedRegular)>}
impl Existing{
    pub(super) fn capture(src:&Verified,d:&Destination<'_>,v:&HeldParent<'_,'_>,a:&MoveAllocation,sealed:Option<&Moved>)->io::Result<Option<Self>>{
        if a.original.from!=src.volume()||a.original.to!=v.volume().id()||a.original.file!=src.file()||a.original.name!=src.name()||
            a.original.parents!=d.parents||a.original.src_root!=*src.source_root()||a.original.dst_root!=d.root||a.original.mode!=d.mode||
            a.original.source_bytes!=src.physical_bytes()||a.original.source_material!=src.digest()?||a.opened()||
            sealed.is_some_and(|m|!a.matches(m)){return Err(invalid("different original or already installed MOVE"));}
        let Some(stage)=a.stage() else {
            if a.original.mode==Method::Transfer&&sealed.is_some(){return Err(invalid("sealed transfer has no original stage"));}
            return Ok(None)
        };
        let dir=bound_dir(v,a,stage)?;
        Ok(Some(Self::capture_roles(src,a,sealed,stage,dir)?))
    }
    pub(super) fn capture_roles(src:&Verified,a:&MoveAllocation,sealed:Option<&Moved>,stage:&Stage,dir:HeldDir)->io::Result<Self>{
        if a.stage()!=Some(stage)||dir.label()?.is_some()||dir.entity!=stage.directory.entity||fs::Birth::read(&dir.file)?!=stage.directory.birth||
            sealed.is_some_and(|m|!a.matches(m)){return Err(invalid("not an authenticated original private transfer directory"));}
        let listed=dir.names()?;let mut found:BTreeSet<_>=listed.iter().cloned().collect();
        if listed.len()!=found.len(){return Err(invalid("duplicate namespace entry in MOVE stage"));}
        let mut parts=BTreeMap::new();let mut gap=false;
        for item in src.objects(){
            if !found.remove(item.name.to_bytes()){
                if sealed.is_some(){return Err(invalid("cannot re-create a missing sealed original target"));}
                gap=true;continue;
            }
            if gap{return Err(invalid("hole before a later already synchronized MOVE role; refuse reconstruction"));}
            let p=dir.pin_stream(&item.name,item.len)?.ok_or_else(||invalid("staged original disappeared"))?;
            let n=NativeObject::stamp(&p.stamp);if n==item.original||sealed.is_some_and(|m|m.target.at(item.kind)!=Some(&n)){
                return Err(invalid("substituted original transfer material"));}
            fs::require_material_token(&p.file,&placed::token(a.digest,item.kind,&n)?)?;p.named(&dir,&item.name)?;
            parts.insert(item.kind,(item.clone(),p));
        }
        if !found.is_empty(){return Err(invalid("unknown target stage occupant; no replacement or deletion"));}
        Ok(Self{stage:stage.clone(),dir,allocation:a.digest,parts})
    }
    pub(super) fn contains(&self,k:Leaf)->bool{self.parts.contains_key(&k)}
    pub(super) fn len(&self)->usize{self.parts.len()}
    pub(super) fn pin(&self,k:Leaf)->Option<&PinnedRegular>{self.parts.get(&k).map(|(_,p)|p)}
    pub(super) fn verify(&self)->io::Result<()>{
        for (item,p) in self.parts.values(){
            p.named(&self.dir,&item.name)?;
            if super::source::read_physical(p,item.len,|_,_|Ok(()))?!=item.digest{return Err(invalid("existing named target bytes differ from the original source"));}
            p.named(&self.dir,&item.name)?;
        }Ok(())
    }
    pub(super) fn require(&self,v:&HeldParent<'_,'_>,a:&MoveAllocation)->io::Result<()>{
        if a.digest!=self.allocation||a.stage()!=Some(&self.stage){return Err(invalid("different reservation or stage on retry"));}
        let d=bound_dir(v,a,&self.stage)?;self.require_directory(a,&d)
    }
    pub(super) fn require_directory(&self,a:&MoveAllocation,d:&HeldDir)->io::Result<()>{
        if a.digest!=self.allocation||a.stage()!=Some(&self.stage){return Err(invalid("changed original custody stage at recheck"));}
        if d.label()?.is_some()||d.entity!=self.dir.entity||!d.same_mount(&self.dir)?||fs::Birth::read(&d.file)?!=fs::Birth::read(&self.dir.file)?{return Err(invalid("held MOVE stage was replaced"));}
        let expected:BTreeSet<_>=self.parts.values().map(|(x,_)|x.name.to_bytes().to_vec()).collect();
        let actual=d.names()?;if actual.len()!=expected.len()||actual.iter().any(|n|!expected.contains(n)){return Err(invalid("target stage changed since capture"));}
        for (x,p) in self.parts.values(){p.named(d,&x.name)?;}Ok(())
    }
}
pub(super) fn bound_dir(v:&HeldParent<'_,'_>,a:&MoveAllocation,stage:&Stage)->io::Result<HeldDir>{
    a.original.check_ordinary(v,false)?;
    if v.volume().id()!=a.original.to||a.original.mode!=Method::Transfer{return Err(invalid("not original target registration for this stage"));}
    let dir=v.directory().exact_directory(&material::staging(a.original.ticket,stage.slot))?.ok_or_else(||invalid("bound target directory absent"))?;
    if dir.label()?.is_some()||dir.entity!=stage.directory.entity||fs::Birth::read(&dir.file)?!=stage.directory.birth{return Err(invalid("bound target directory replaced"));}
    Ok(dir)
}
/// Only a newly created empty directory on this call can mint this operand.
/// Existing empty unbound occupants consume their issued slots but are NEVER
/// adopted, traversed as FILEs, or removed.
pub(super) struct Fresh{dir:HeldDir,stage:Stage,allocation:[u8;32]}
impl Fresh{
    pub(super) fn under_writer(p:&LocalPool,v:&HeldParent<'_,'_>,a:&MoveAllocation,hook:&mut impl FnMut(Cut)->io::Result<()>)->io::Result<Self>{
        if !matches!(p.access,Access::Write)||!std::ptr::eq(v.volume().pool,p)||a.stage().is_some()||a.original.mode!=Method::Transfer||
            a.original.to!=v.volume().id(){return Err(invalid("not the unbound original transfer writer and volume"));}
        a.original.check_ordinary(v,false)?;
        let names=material::check_birth_name(v,&a.original.name,a.original.file,false)?;
        // The ordinary-domain census has already checked actual private
        // entries as a contiguous prefix of unlabelled empty native objects.
        let slot=(0..allocation::SLOTS).find(|&i|!names.iter().any(|n|n.as_slice()==material::staging(a.original.ticket,i).to_bytes()))
            .ok_or_else(||invalid("original target ticket exhausted; do not adopt an unbound slot"))?;
        let dir=v.directory().new_child_directory(&material::staging(a.original.ticket,slot))?;
        dir.only(None)?;dir.file.sync_all()?;v.directory().file.sync_all()?;hook(Cut::UnboundMkdir)?;
        dir.require_named()?;let directory=NativeObject{entity:dir.entity,birth:fs::Birth::read(&dir.file)?};
        Ok(Self{dir,stage:Stage{slot,directory},allocation:a.digest})
    }
    pub(super) fn witness(&self,v:&HeldParent<'_,'_>,a:&MoveAllocation)->io::Result<Stage>{
        if self.allocation!=a.digest||a.stage().is_some(){return Err(invalid("not the original fresh directory"));}
        let names=material::check_birth_name(v,&a.original.name,a.original.file,false)?;
        if (0..allocation::SLOTS).any(|i|names.iter().any(|n|n.as_slice()==material::staging(a.original.ticket,i).to_bytes())!=(i<=self.stage.slot)){
            return Err(invalid("not the last empty original slot created by this MOVE"));}
        self.dir.only(None)?;self.dir.require_named()?;let check=bound_dir(v,a,&self.stage)?;
        if check.entity!=self.dir.entity||!check.same_mount(&self.dir)?{return Err(invalid("fresh MOVE stage differs"));}
        Ok(self.stage.clone())
    }
}
pub(super) fn sites(dir:&HeldDir,roles:&BTreeMap<Leaf,NativeObject>,o:&super::transfer::Origin)->io::Result<Sites>{
    let get=|k|roles.get(&k).cloned().ok_or_else(||invalid("incomplete original MOVE role set"));
    if roles.len()!=o.role_count()||roles.contains_key(&Leaf::Incr)!=o.children.first().is_some()||roles.contains_key(&Leaf::IncrNext)!=o.children.second().is_some(){return Err(invalid("incomplete original FILE stage"));}
    Ok(Sites{directory:NativeObject{entity:dir.entity,birth:fs::Birth::read(&dir.file)?},
        local:get(Leaf::File)?,node:get(Leaf::FullNode)?,raw:get(Leaf::FullRaw)?,children:match(o.children.len(),roles.get(&Leaf::Incr),roles.get(&Leaf::IncrNext)){
            (0,None,None)=>super::transfer::ChildRoles::None,(1,Some(a),None)=>super::transfer::ChildRoles::One(a.clone()),
            (2,Some(a),Some(b))=>super::transfer::ChildRoles::Two(a.clone(),b.clone()),_=>return Err(invalid("gap in original MOVE role set"))}})
}
