//! Logical history never changes address. Only an original SYSTEM handoff
//! selects the alternate native objects for that same FILE/entry/history.
use super::{destination::Method,source::{Clean,Leaf,Verified},transfer::{Children,Moved,Sites}};
use crate::{fs::{self,HeldDir,invalid,stream::PinnedRegular},pool::lease::FileVolume,
    ordinary::{formal::types::Commit,
        journal::Journal,material,wire::{self,Frame,NativeObject}}};
use std::{collections::BTreeSet,ffi::{CStr,CString},io};

pub(super) struct Role<'a>{pub kind:Leaf,pub name:CString,pub len:u64,pub original:&'a NativeObject}
pub(super) fn roles<'a>(c:&Frame,f:&Commit,d:Children<'_>,sites:&'a Sites)->io::Result<Vec<Role<'a>>>{
    let mut out=vec![
        Role{kind:Leaf::File,name:c"FILE".to_owned(),len:wire::empty_material(c.create.file,c.create.history,c.create.op,&c.create.name).len()as u64,original:&sites.local},
        Role{kind:Leaf::FullNode,name:f.original.node_name(),len:crate::ordinary::formal::physical::index_size(&f.original)?,original:&sites.node},
        Role{kind:Leaf::FullRaw,name:f.original.body_name(),len:f.original.content.bytes,original:&sites.raw},
    ];
    if d.len()!=sites.children.len(){return Err(invalid("native role count differs from the frozen original history"));}
    for (i,(node,n)) in d.iter().zip(sites.children.iter()).enumerate(){
        let kind=match i{0=>Leaf::Incr,1=>Leaf::IncrNext,_=>return Err(invalid("unsupported retained ordinal"))};
        out.push(Role{kind,name:node.original.leaf(),len:crate::ordinary::delta::physical::packed_size(&node.original)?,original:n});
    }Ok(out)
}
pub(super) fn token(allocation:[u8;32],kind:Leaf,n:&NativeObject)->io::Result<[u8;32]>{
    let mut b=b"ZXS/MOVE/OWN-TARGET\0".to_vec();b.extend_from_slice(&allocation);b.push(kind.tag());n.put(&mut b);crate::hash::digest(&b)
}
pub(super) fn root(v:&FileVolume<'_>,wanted:&NativeObject)->io::Result<()>{
    v.require_current()?;
    if v.root.entity!=wanted.entity||fs::Birth::read(&v.root.file)?!=wanted.birth{return Err(invalid("not the MOVE's exact registered parent"));}
    Ok(())
}
pub(super) fn held_role(dir:&HeldDir,r:&Role<'_>,token_from:Option<[u8;32]>)->io::Result<PinnedRegular>{
    let p=dir.pin_stream(&r.name,r.len)?.ok_or_else(||invalid("missing retained FILE role"))?;
    if &NativeObject::stamp(&p.stamp)!=r.original{return Err(invalid("different native incarnation of selected FILE role"));}
    if let Some(a)=token_from{fs::require_material_token(&p.file,&token(a,r.kind,r.original)?)?;}
    p.named(dir,&r.name)?;Ok(p)
}
pub(super) fn complete(v:&FileVolume<'_>,name:&CStr,m:&Moved,c:&Frame,f:&Commit,d:Children<'_>)->io::Result<HeldDir>{
    if m.original.purpose==super::purpose::Purpose::Ordinary{m.require_history(c,f,d)?;}else{
        let(_,s)=v.pool.snapshot()?;let(actual,p,child)=super::finish::frozen(&s,m)?;
        if !matches!(m.original.purpose,super::purpose::Purpose::Restore{..})||actual.digest!=c.digest||p.digest!=f.digest||
            child.map(|v|v.digest)!=d.map(|v|v.digest){return Err(invalid("not this confirmed ordinary restore target's exact frozen history"));}
    }
    root(v,&m.original.dst_root)?;
    if v.id()!=m.original.to{return Err(invalid("wrong registered MOVE destination"));}
    let dir=v.exact_directory(name)?.ok_or_else(||invalid("decided FILE not yet installed at its original target"))?;
    material::verify_header_at(&dir,&c.create,&m.target.directory,&m.target.local)?;
    let rows=roles(c,f,d,&m.target)?;
    let want:BTreeSet<_>=rows.iter().map(|r|r.name.to_bytes().to_vec()).collect();
    let actual=dir.names()?;if actual.len()!=want.len()||actual.iter().any(|x|!want.contains(x)){
        return Err(invalid("missing or unknown material in relocated FILE"));
    }
    for r in &rows{let _=held_role(&dir,r,(m.original.mode==Method::Transfer).then_some(m.allocation))?;}
    dir.require_named()?;root(v,&m.original.dst_root)?;Ok(dir)
}
/// Admit only the transferred fixed role set here. The caller must also
/// enumerate *all* names against this same current prefix. Later working
/// objects and the first B-native INCR never inherit a MOVE material token.
pub(in crate::ordinary) fn active_dir(s:&Journal,v:&FileVolume<'_>,c:&Frame)->io::Result<Option<HeldDir>>{
    let Some(m)=s.placed(c.create.file)else{return Ok(None)};
    let (actual,f,d)=super::finish::frozen(s,m)?;
    if actual.digest!=c.digest||v.id()!=m.original.to{return Err(invalid("not the confirmed current FILE owner"));}
    if let Some(a)=s.moving.as_ref().filter(|a|a.original.op==m.original.op){
        if !a.matches(m)||!a.opened(){return Err(io::Error::new(io::ErrorKind::WouldBlock,"MOVE decided; target installation not yet confirmed"));}
        a.confirm_phase(v.pool)?;
    }
    root(v,&m.original.dst_root)?;
    let name=crate::ordinary::storage_name::file_leaf(&m.original.name,m.original.file)?;
    let dir=v.exact_directory(&name)?.ok_or_else(||invalid("current owner unavailable; never read from the previous location"))?;
    if let Some(slot)=m.stage{if v.root.entry_present(&material::staging(m.original.ticket,slot))?{
        return Err(invalid("unexpected private stage on an installed owner"));}
    }
    material::verify_header_at(&dir,&c.create,&m.target.directory,&m.target.local)?;
    for r in roles(c,f,d,&m.target)?{let _=held_role(&dir,&r,(m.original.mode==Method::Transfer).then_some(m.allocation))?;}
    dir.require_named()?;root(v,&m.original.dst_root)?;Ok(Some(dir))
}
/// A completed DATA scan of the exact installed name or private stage. The
/// transfer decision cannot replace it, and a string/checksum cannot mint it.
pub(super) struct TargetInput{clean:Clean,decision:Moved,name:CString}
pub(super) struct Reopened{verified:Verified,decision:Moved,name:CString}
impl Reopened{
    pub(super) fn input(v:&FileVolume<'_>,name:&CStr,m:&Moved,c:&Frame,f:&Commit,d:Children<'_>)->io::Result<TargetInput>{
        let dir=complete(v,name,m,c,f,d)?;dir.share()?;
        Ok(TargetInput{clean:Clean::at_original_sites(v,dir,c,f,d,&m.target)?,decision:m.clone(),name:name.to_owned()})
    }
    pub(super) fn input_lifecycle(p:&crate::pool::lease::LocalPool,id:crate::VolumeId,m:&Moved,name:CString,dir:HeldDir,c:&Frame,f:&Commit,d:Children<'_>)->io::Result<TargetInput>{
        if id!=m.original.to{return Err(invalid("wrong dedicated destination"));}
        let clean=Clean::at_held_sites(p.id(),id,m.original.dst_root.clone(),dir,c,f,d,&m.target)?;
        Ok(TargetInput{clean,decision:m.clone(),name})
    }
    pub(super) fn require_lifecycle(&self,end:&crate::pool::lease::recycle::CustodyEnd<'_,'_>,goal:&super::target::Goal,name:&CStr)->io::Result<()>{
        if self.name.as_c_str()!=name||self.decision!=goal.frame{return Err(invalid("target proof belongs to another frame or address"));}
        self.verified.require_at_lifecycle(end,goal)
    }
    pub(super) fn verify(input:TargetInput)->io::Result<Self>{
        let TargetInput{clean,decision:m,name}=input;let verified=clean.verify()?;
        if verified.file()!=m.original.file||verified.physical_bytes()!=m.original.source_bytes||verified.digest()?!=m.original.source_material{
            return Err(invalid("complete destination bytes differ from original native FILE history"));
        }
        Ok(Self{verified,decision:m,name})
    }
    pub(super) fn require(&self,v:&FileVolume<'_>,name:&CStr,m:&Moved)->io::Result<()>{
        if self.decision!=*m||self.name.as_c_str()!=name||v.id()!=m.original.to{return Err(invalid("cannot offer a different handoff original or target"));}
        self.verified.require_at(v,name,&m.original.dst_root,&m.target)
    }
    /// Read-only: the original confirmed writer performed the rename and
    /// synced the parent. An independently verified predecision target can
    /// never perform that namespace change merely by holding this proof.
    pub(super) fn after_transfer_rename(mut self,v:&FileVolume<'_>,m:&Moved,private:&CStr,final_name:CString)->io::Result<Self>{
        if self.decision!=*m||self.name.as_c_str()!=private||m.original.mode!=Method::Transfer||
            m.original.purpose!=super::purpose::Purpose::Ordinary||
            m.stage.map(|slot|material::staging(m.original.ticket,slot)).as_deref()!=Some(private)||
            final_name!=crate::ordinary::storage_name::file_leaf(&m.original.name,m.original.file)?||v.root.entry_present(private)?{
            return Err(invalid("not the same original private-to-installed transfer"));}
        self.verified.follow_transfer_rename(v,m,private,&final_name)?;self.name=final_name;Ok(self)
    }
    pub(super) fn verify_installed(&self,v:&FileVolume<'_>,m:&Moved)->io::Result<()>{
        let name=crate::ordinary::storage_name::file_leaf(&m.original.name,m.original.file)?;
        if let Some(slot)=m.stage{if v.root.entry_present(&material::staging(m.original.ticket,slot))?{
            return Err(invalid("original target private name occupied after installation"));}}
        self.require(v,&name,m)
    }
    pub(super) fn files(&self)->&[super::source::Bytes]{self.verified.objects()}
}
