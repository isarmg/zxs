//! Immediate ordinary names, from this exact SYSTEM parent. Issued private
//! spelling is evidence of a reservation only, never authority to adopt,
//! remove or report an unresolved occupant as a business directory.
use super::{path::{self,HeldParent},types::{Created,Parent}};
use crate::{fs::{self,HeldDir,invalid},ordinary::{allocation::SLOTS,journal::Journal,material,wire::NativeObject},OperationId};
use std::{collections::{BTreeMap,BTreeSet},ffi::{CStr,CString},io};

pub(super) fn human(name:&str)->io::Result<CString>{
    Ok(crate::ordinary::storage_name::DirectoryName::parse(name.as_bytes())?.leaf())
}
pub(in crate::ordinary) fn same_child(dir:&HeldDir,original:&NativeObject)->io::Result<()>{
    if dir.entity!=original.entity||fs::Birth::read(&dir.file)?!=original.birth||dir.label()?.is_some(){
        return Err(invalid("ordinary directory is not its original unlabelled native child"));}
    dir.require_named()
}
fn empty_at(parent:&HeldDir,name:&CStr,object:Option<&NativeObject>)->io::Result<HeldDir>{
    let h=parent.child_directory(name)?.ok_or_else(||invalid("issued ordinary private directory disappeared"))?;
    if let Some(n)=object{same_child(&h,n)?;}
    if h.label()?.is_some(){return Err(invalid("a registered volume is not an ordinary directory"));}
    h.only(None)?;h.require_named()?;Ok(h)
}
pub(in crate::ordinary) struct Observation<'a>{
    pub rows:Vec<&'a Created>,pub consumed:BTreeSet<Vec<u8>>,pub private_count:usize,pub pending_install:bool,
}
/// Inspect only original directory entries on this parent. Caller separately
/// accounts for FILE and custody names, and must refuse leftovers.
pub(in crate::ordinary) fn observe<'a>(s:&'a Journal,address:Parent,parent:&HeldDir,raw:&BTreeSet<Vec<u8>>)->io::Result<Observation<'a>>{
    let active=s.making.as_ref().filter(|a|a.origin.parent.address==address);
    let decided=active.is_some_and(|a|s.decided(a.origin.op).is_some());
    let mut result=Observation{rows:Vec::new(),consumed:BTreeSet::new(),private_count:0,pending_install:decided};
    let mut spent:BTreeMap<OperationId,u8>=BTreeMap::new();
    for d in s.directories_at(address){
        if active.is_some_and(|a|a.origin.op==d.origin.op){continue;}
        if !raw.contains(d.origin.name.as_bytes()){return Err(invalid("missing confirmed ordinary directory"));}
        path::installed_in(parent,d,Some(raw))?;result.consumed.insert(d.origin.name.as_bytes().to_vec());
        result.rows.push(d);spent.insert(d.origin.ticket,d.slot);
    }
    if let Some(a)=active{
        let o=&a.origin;let terminal=human(&o.name)?;let final_here=raw.contains(terminal.to_bytes());
        let mut gap=false;let mut stage_here=false;
        for i in 0..SLOTS{
            let n=material::staging(o.ticket,i);
            if !raw.contains(n.to_bytes()){gap=true;continue;}
            if let Some(st)=&a.stage{
                if i>st.slot{return Err(invalid("past the original bound ordinary directory slot"));}
                if i==st.slot{empty_at(parent,&n,Some(&st.object))?;stage_here=true;}
                else{empty_at(parent,&n,None)?;}
            }else{
                if gap{return Err(invalid("gap before an unbound ordinary directory slot"));}
                empty_at(parent,&n,None)?;
            }
            result.consumed.insert(n.into_bytes());result.private_count+=1;
        }
        if let Some(st)=&a.stage{
            if decided{
                if stage_here==final_here{return Err(invalid("published mkdir needs exactly one original private or final child"));}
                if final_here{empty_at(parent,&terminal,Some(&st.object))?;result.consumed.insert(terminal.into_bytes());}
            }else if !stage_here||final_here{return Err(invalid("unpublished mkdir requires its bound original and an empty final name"));}
        }else if final_here||decided{return Err(invalid("unbound mkdir cannot have an installed or published ordinary directory"));}
    }
    for name in raw{
        if let Some((ticket,slot))=material::private_slot(name){
            if spent.get(&ticket).is_some_and(|limit|slot<*limit){
                let n=material::staging(ticket,slot);empty_at(parent,&n,None)?;
                result.consumed.insert(name.clone());result.private_count+=1;
            }
        }
    }
    Ok(result)
}
/// At any new mkdir or during its original exclusive reservation the single
/// global slot excludes simultaneous incomplete FILE/custody commands.
/// Check each visible FILE's actual local identity, not unrelated bodies.
pub(super) fn census(parent:&HeldParent<'_,'_>,s:&Journal)->io::Result<BTreeSet<Vec<u8>>>{
    if s.allocated.is_some()||s.saving.is_some()||s.editing.is_some()||s.committing.is_some()||s.incrementing.is_some()||s.moving.is_some(){
        return Err(invalid("a different original writer owns this namespace cut"));}
    let dir=parent.directory();let address=parent.basis().address;let raw=dir.names()?.into_iter().collect::<BTreeSet<_>>();
    let mut known=observe(s,address,dir,&raw)?.consumed;
    let mut file_tickets=BTreeMap::new();
    for f in s.on_all().filter(|f|f.create.parent.address(f.create.volume)==address){file_tickets.insert(f.create.attempt,f.create.slot);}
    // A private ticket is issued at one exact ordinary destination. A
    // sibling parent or the dedicated recycler cannot inherit its slots.
    for m in s.moves(){
        if m.original.ends()?.1.ordinary()==Some(address){
            if let Some(i)=m.stage{file_tickets.insert(m.original.ticket,i);}
        }
    }
    for f in s.at_parent(address){
        let c=&f.create;let place=s.placement(c.file)?;let n=crate::ordinary::storage_name::file_leaf(&c.name,c.file)?;
        if place.ordinary_parent()?!=Some(address){return Err(invalid("different ordinary FILE owner or parent in inventory"));}
        let h=dir.child_directory(&n)?.ok_or_else(||invalid("missing current FILE sibling of ordinary directory"))?;
        material::verify_header_at(&h,c,place.directory(),place.local())?;
        if !known.insert(n.into_bytes()){return Err(invalid("one physical name used by both FILE and directory"));}
    }
    for n in &raw{
        if let Some((t,i))=material::private_slot(n){
            if file_tickets.get(&t).is_some_and(|limit|i<*limit){
                // A new write may not cross an unknown object beneath a
                // private spelling. Check it without adopting or deleting it.
                empty_at(dir,&material::staging(t,i),None)?;known.insert(n.clone());
            }
        }
    }
    if known!=raw{return Err(invalid("unrecognized or missing sibling at the original mkdir parent"));}
    parent.require()?;if dir.names()?.into_iter().collect::<BTreeSet<_>>()!=raw{return Err(invalid("ordinary parent inventory changed"));}
    Ok(raw)
}
