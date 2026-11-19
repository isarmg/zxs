//! An original MOVE is already decided here. The named target must be
//! completely reopened, with all versions, independently of the source.
//! Only then can the exact latter be retired, under its own original leases.
use super::{command::Request,destination::Method,placed::{self,Reopened,TargetInput},retire,source::{Clean,Verified},transfer::{ChildRoles,Children,Moved,Sites}};
use crate::{fs::{self,invalid},ordinary::{material,wire::Frame,formal::types::Commit,delta::types::Delta,Cut},
    pool::lease::{Access,FileVolume,LocalPool}};
use std::{ffi::{CStr,CString},io,path::Path};
/// Only the original native cut being handed off, never a later node merely
/// because it is currently the FILE's tip.
pub(super) fn frozen_graph<'a>(s:&'a crate::ordinary::journal::Journal,m:&Moved)->io::Result<(&'a Frame,crate::ordinary::journal::Graph<'a>)>{
    let(before,graph)=s.handoff_graph(m)?;m.require_at_graph(before,graph)?;Ok((before.birth(),graph))
}
pub(super) fn frozen<'a>(s:&'a crate::ordinary::journal::Journal,m:&Moved)->io::Result<(&'a Frame,&'a Commit,Children<'a>)>{
    let(c,g)=frozen_graph(s,m)?;
    Ok((c,g.full(),g.children()))
}
pub(super) fn history(s:&crate::ordinary::journal::Journal,m:&Moved)->io::Result<(Frame,Commit,ChildRoles<Delta>)>{
    let(c,f,d)=frozen(s,m)?;Ok((c.clone(),f.clone(),d.map(Clone::clone)))
}
fn name(m:&Moved)->io::Result<CString>{crate::ordinary::storage_name::file_leaf(&m.original.name,m.original.file)}
/// A pre-rename source may be on a different registered root of the same
/// mount. Pin/scan all original bytes before the namespace transition.
fn at_source(v:&FileVolume<'_>,label:&CStr,m:&Moved,c:&Frame,f:&Commit,d:Children<'_>)->io::Result<Clean>{
    placed::root(v,&m.original.src_root)?;
    let dir=v.root.exact_directory(label)?.ok_or_else(||invalid("original source missing before rename"))?;
    dir.exclusive()?;
    Clean::at_original_sites(v,dir,c,f,d,&Sites::birthplace(c,f,d))
}
enum Scan{Target{input:TargetInput,fresh:bool,name:CString,frame:Moved},Source{input:Clean,frame:Moved}}
fn select(system:&Path,r:Request)->io::Result<Scan>{
    let(p,_,s)=LocalPool::with_snapshot(system,Access::Incoming)?;
    r.observe(&p,&s)?.ok_or_else(||invalid("missing original MOVE"))?;
    let(m,a)=s.confirmed_handoff(r.op)?;let(c,f,d)=history(&s,m)?;let dst=p.ordinary(m.original.to)?;
    placed::root(&dst,&m.original.dst_root)?;let name=name(m)?;
    if dst.root.exact_directory(&name)?.is_some(){
        if let Some(st)=a.stage(){
            if dst.root.exact_directory(&material::staging(m.original.ticket,st.slot))?.is_some(){return Err(invalid("both original installed and reserved MOVE directories are named"));}
        }
        let input=Reopened::input(&dst,&name,m,&c,&f,d.as_refs())?;return Ok(Scan::Target{input,fresh:false,name,frame:m.clone()});
    }
    if a.opened(){return Err(invalid("previously admitted installed target missing; never recreate or revert to source"));}
    material::vacant_for_move(&dst,&m.original.name,m.original.file)?;
    match m.original.mode{
        Method::Rename=>{
            let src=p.ordinary(m.original.from)?;
            if !src.root.same_mount(&dst.root)?{return Err(invalid("no cross-mount fallback for original rename"));}
            Ok(Scan::Source{input:at_source(&src,&name,m,&c,&f,d.as_refs())?,frame:m.clone()})
        }
        Method::Transfer=>{
            let st=a.stage().ok_or_else(||invalid("no original bound target at published transfer"))?;
            let name=material::staging(m.original.ticket,st.slot);
            let input=Reopened::input(&dst,&name,m,&c,&f,d.as_refs())?;
            Ok(Scan::Target{input,fresh:true,name,frame:m.clone()})
        }
    }
}
fn sync_rename(system:&Path,r:Request,m:&Moved,v:&Verified,hook:&mut impl FnMut(Cut)->io::Result<()>)->io::Result<()>{
    let(p,_,s)=LocalPool::with_snapshot(system,Access::Write)?;
    let(now,a)=s.confirmed_handoff(r.op)?;if now!=m||a.opened()||m.request!=r.digest(&p)?{return Err(invalid("original rename changed"));}
    let src=p.ordinary(m.original.from)?;let dst=p.ordinary(m.original.to)?;let n=name(m)?;let(c,f,d)=history(&s,m)?;
    let sites=Sites::birthplace(&c,&f,d.as_refs());v.require_at(&src,&n,&m.original.src_root,&sites)?;
    if v.physical_bytes()!=m.original.source_bytes||v.digest()?!=m.original.source_material{return Err(invalid("original rename history changed before application"));}
    placed::root(&dst,&m.original.dst_root)?;material::vacant_for_move(&dst,&m.original.name,m.original.file)?;
    src.root.handoff_directory(&n,&dst.root,&n,sites.directory.entity,&sites.directory.birth,||hook(Cut::MoveInstalled))
}
fn install_transfer(system:&Path,r:Request,m:&Moved,stage:&CStr,v:Reopened,hook:&mut impl FnMut(Cut)->io::Result<()>)->io::Result<Reopened>{
    let(p,_,s)=LocalPool::with_snapshot(system,Access::Write)?;
    let(now,a)=s.confirmed_handoff(r.op)?;if now!=m||a.opened()||m.request!=r.digest(&p)?{return Err(invalid("original transfer changed"));}
    let dst=p.ordinary(m.original.to)?;let st=a.stage().ok_or_else(||invalid("no original transfer directory"))?;
    if material::staging(m.original.ticket,st.slot).as_c_str()!=stage{return Err(invalid("different original private target"));}
    v.require(&dst,stage,now)?;material::vacant_for_move(&dst,&m.original.name,m.original.file)?;
    let target=name(m)?;
    dst.root.handoff_directory(stage,&dst.root,&target,m.target.directory.entity,&m.target.directory.birth,||hook(Cut::MoveInstalled))?;
    v.after_transfer_rename(&dst,now,stage,target)
}
pub(super) fn complete(system:&Path,r:Request,hook:&mut impl FnMut(Cut)->io::Result<()>)->io::Result<()>{
    // At most two states: the still-private original followed by the final
    // installed name. No root scanning/reselection and no recursive retries.
    for _ in 0..2{match select(system,r)?{
        Scan::Source{input,frame}=>{let v=input.verify()?;sync_rename(system,r,&frame,&v,hook)?;}
        Scan::Target{input,fresh,name,frame}=>{
            let checked=Reopened::verify(input)?;
            let checked=if fresh{install_transfer(system,r,&frame,&name,checked,hook)?}else{checked};
            {
                let(p,_,s)=LocalPool::with_snapshot(system,Access::Write)?;let(now,a)=s.confirmed_handoff(r.op)?;
                if now!=&frame||r.digest(&p)?!=now.request{return Err(invalid("different original installed target"));}
                let dst=p.ordinary(now.original.to)?;checked.verify_installed(&dst,now)?;dst.root.file.sync_all()?;dst.require_current()?;
                if !a.opened(){a.confirm_opened(&p,&dst,&checked,||hook(Cut::MoveTargetFlagBeforeSync))?;}else{a.confirm_phase(&p)?;}
            }
            hook(Cut::MoveTargetChecked)?;
            return retire::complete(system,r,&frame,&checked,hook)
        }
    }}
    Err(invalid("original MOVE still awaiting its complete final target"))
}
