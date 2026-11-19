//! Strict census of an ordinary target while one FILE may be changing. The
//! only exception is that FILE's exact terminal/issued slots, which the
//! writer separately checks before sealing/installing. Unknown, unissued
//! and partly recognizable names never become namespace members.
use super::{directory::{self,types::Parent},journal::Journal,material,storage_name::file_leaf};
use crate::{FileId,VolumeId,fs::{self,HeldDir,invalid},pool::lease::LocalPool};
use std::{collections::{BTreeMap,BTreeSet},ffi::CString,io};

pub(super) fn ordinary_target_domain(p:&LocalPool,s:&Journal,id:VolumeId,root:&HeldDir,display:&str,
    subject:FileId,raw:&[Vec<u8>])->io::Result<()>{
    let address=Parent::Root(id);let birth=fs::Birth::read(&root.file)?;
    if !p.journal_registration().file(id,fs::local_uid(),root.entity,&birth,true){
        return Err(invalid("target census is not its registered ordinary parent"));}
    root.require_named()?;
    let listed:BTreeSet<_>=raw.iter().cloned().collect();
    let mut known=directory::inventory::observe(s,address,root,&listed)?.consumed;
    let mut lower=BTreeMap::new();
    for f in s.on_all().filter(|f|f.create.parent.address(f.create.volume)==address){lower.insert(f.create.attempt,f.create.slot);}
    for m in s.moves().filter(|m|m.original.to==id){if let Some(i)=m.stage{lower.insert(m.original.ticket,i);}}
    for f in s.at_parent(address).filter(|f|f.create.file!=subject){
        let c=&f.create;let place=s.placement(c.file)?;let n=file_leaf(&c.name,c.file)?;
        if place.ordinary_parent()!=Some(address)||!known.insert(n.to_bytes().to_vec()){
            return Err(invalid("missing or multiply-used original ordinary sibling"));}
        let d=root.directory_in_sample(&n,&listed)?.ok_or_else(||invalid("ordinary FILE sibling disappeared"))?;
        material::verify_header_at(&d,c,place.directory(),place.local())?;
    }
    // An exact match here is not returned as a FILE. Before any side effect
    // the owning stage/installation path still checks vacancy or its native
    // original with this cut; no special file or alias is opened by census.
    let own=file_leaf(display,subject)?;
    let create=s.allocated.as_ref().filter(|a|a.parent.address(a.volume)==address&&a.file==subject);
    let moving=s.moving.as_ref().filter(|a|a.original.to==id&&a.original.file==subject&&a.original.mode==super::custody::Method::Transfer);
    let current=create.map(|a|(a.attempt,a.stage.as_ref().map(|x|x.slot)))
        .or_else(||moving.map(|a|(a.original.ticket,a.stage().map(|x|x.slot))));
    let empty_private=|leaf:&std::ffi::CStr|->io::Result<()>{
        let d=root.child_directory(leaf)?.ok_or_else(||invalid("issued empty private slot missing"))?;
        if d.label()?.is_some(){return Err(invalid("a registered root is not a skipped slot"));}
        d.only(None)?;d.require_named()
    };
    if let Some((ticket,bound))=current{
        let mut gap=false;
        for i in 0..super::allocation::SLOTS{
            let n=material::staging(ticket,i);let present=root.exact_presence(raw,&n)?;
            // This single bound name is independently verified as its actual
            // staged object/successful installation by the owning operation.
            if bound==Some(i){if present{known.insert(n.into_bytes());}continue;}
            if present{
                if gap||bound.is_some_and(|j|i>j){return Err(invalid("unreachable private FILE ticket occupancy"));}
                empty_private(&n)?;known.insert(n.into_bytes());
            }else{
                if bound.is_some_and(|j|i<j){return Err(invalid("unverified missing lower FILE ticket slot"));}
                gap=true;
            }
        }
    }
    for n in &listed{
        if known.contains(n)||n.as_slice()==own.to_bytes(){continue;}
        if let Some((t,i))=material::private_slot(n){
            if lower.get(&t).is_some_and(|&end|i<end){
                let leaf=CString::new(n.clone()).map_err(|_|invalid("invalid enumerated private slot"))?;
                empty_private(&leaf)?;continue;
            }
        }
        return Err(invalid("unattributed native sibling at the original ordinary target"));
    }
    if listed.len()!=raw.len()||root.names()?.into_iter().collect::<BTreeSet<_>>()!=listed{
        return Err(invalid("ordinary target changed during exact namespace admission"));}
    root.require_named()
}
