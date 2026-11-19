//! Separate original-issued private names, the one current business name and
//! non-serving remnants. Unknown names may never disappear from a full list.
use super::{destination::Method,finish::history,placed,retire,staging};
use crate::{FileId,OperationId,fs::invalid,ordinary::{allocation,journal::Journal,material},pool::lease::FileVolume};
use std::{collections::{BTreeMap,BTreeSet},ffi::CString,io};
#[derive(Default)]
pub(in crate::ordinary) struct View{pub pending:Vec<super::command::MoveReceipt>,pub lifecycle:Vec<super::recycle::RecycleReceipt>,pub deferred:Option<FileId>,pub hidden:BTreeSet<Vec<u8>>,pub named_stage:Option<Vec<u8>>}
pub(in crate::ordinary) fn view(s:&Journal,v:&FileVolume<'_>,tickets:&mut BTreeMap<OperationId,u8>)->io::Result<View>{
    let mut seen=View::default();
    for m in s.moves().filter(|m|m.original.to==v.id()){
        if let Some(slot)=m.stage{tickets.insert(m.original.ticket,slot);}
    }
    let Some(a)=s.moving.as_ref().filter(|a|a.original.to==v.id()||a.original.from==v.id())else{return Ok(seen)};
    let o=&a.original;
    let req=super::command::Request{call:o.purpose.call(),op:o.op,id:o.file,from:o.from,to:o.to,epoch:o.epoch};
    if req.call==super::purpose::Call::Move{
        seen.pending.push(req.observe(v.pool,s)?.ok_or_else(||invalid("pending ordinary MOVE lacks its receipt"))?);
    }else{
        seen.lifecycle.push(super::recycle::receipt(v.pool,s,req)?.ok_or_else(||invalid("dedicated handoff lacks its exact original receipt"))?);
    }
    let moved=s.placed(o.file).filter(|m|m.original.op==o.op);
    if v.id()==o.from{
        if let Some(m)=moved{
            let(c,f,d)=history(s,m)?;
            if let Some(n)=retire::observe_source(v,m,a.progress(),&c,&f,d.as_refs())?{seen.hidden.insert(n);}
        }return Ok(seen)
    }
    let final_name=crate::ordinary::storage_name::file_leaf(&o.name,o.file)?;
    let final_present=v.root.child_directory(&final_name)?.is_some();
    if let Some(m)=moved{
        if !a.opened(){seen.deferred=Some(o.file);
            if final_present{let(c,f,d)=history(s,m)?;let _=placed::complete(v,&final_name,m,&c,&f,d.as_refs())?;seen.hidden.insert(final_name.to_bytes().to_vec());}
        }
    }else if final_present{return Err(invalid("destination occupied before original custody decision"));}
    if o.mode==Method::Rename{return Ok(seen);}
    let Some(st)=a.stage()else{tickets.insert(o.ticket,allocation::SLOTS);return Ok(seen)};
    tickets.insert(o.ticket,st.slot);
    let n=material::staging(o.ticket,st.slot);
    if v.root.child_directory(&n)?.is_some(){
        if final_present{return Err(invalid("both true stage and final MOVE name exist"));}
        let dir=staging::bound_dir(v,a,st)?;let c=s.file(o.file).ok_or_else(||invalid("missing MOVE origin"))?;
        let f=s.head(o.file).ok_or_else(||invalid("missing FULL"))?;
        let d=if let Some(m)=moved{super::finish::frozen(s,m)?.2}else{s.actual_graph(o.file)?.children()};
        o.require_history(c,f,d)?;
        let before=if let Some(m)=moved{s.before_handoff(m)?}else{s.placement(o.file)?};
        before.require_next(o)?;let previous=before.sites(f,d)?;let rows=placed::roles(c,f,d,&previous)?;
        let sealed=s.pending.as_ref().map(|p|p.frame.moved()).transpose()?.or(moved);
        let mut names:BTreeSet<_>=dir.names()?.into_iter().collect();let mut gap=false;
        for role in rows{
            if !names.remove(role.name.to_bytes()){if sealed.is_some(){return Err(invalid("sealed private target lost a role"));}gap=true;continue;}
            if gap{return Err(invalid("private target has a hole before synchronized material"));}
            let pin=dir.pin_stream(&role.name,role.len)?.ok_or_else(||invalid("private target disappeared"))?;
            let got=crate::ordinary::wire::NativeObject::stamp(&pin.stamp);
            if got==*role.original||sealed.is_some_and(|m|m.target.at(role.kind)!=Some(&got)){return Err(invalid("different native occupant in original MOVE stage"));}
            crate::fs::require_material_token(&pin.file,&placed::token(a.digest,role.kind,&got)?)?;pin.named(&dir,&role.name)?;
        }
        if !names.is_empty(){return Err(invalid("unknown name in bound MOVE target"));}
        dir.require_named()?;seen.named_stage=Some(n.as_bytes().to_vec());
    }else if moved.is_none()||!final_present{return Err(invalid("bound MOVE stage disappeared with no exact installed destination"));}
    Ok(seen)
}
