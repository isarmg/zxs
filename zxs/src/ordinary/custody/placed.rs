//! Logical history never changes address. Only an original SYSTEM handoff
//! selects the alternate native objects for that same FILE/entry/history.
use super::{destination::Method,source::{Clean,Leaf,Verified},transfer::{Moved,Sites}};
use crate::{fs::{self,HeldDir,invalid,stream::PinnedRegular},pool::lease::FileVolume,
    ordinary::{delta::{physical::Body as Incremental,types::Delta},formal::{physical::HistoryBody,types::Commit},
        journal::Journal,material,wire::{self,Frame,NativeObject}}};
use std::{collections::BTreeSet,ffi::{CStr,CString},io};

pub(super) struct Role<'a>{pub kind:Leaf,pub name:CString,pub len:u64,pub original:&'a NativeObject}
pub(super) fn roles<'a>(c:&Frame,f:&Commit,d:&Delta,sites:&'a Sites)->io::Result<Vec<Role<'a>>>{
    Ok(vec![
        Role{kind:Leaf::File,name:c"FILE".to_owned(),len:wire::empty_material(c.create.file,c.create.history,c.create.op,&c.create.name).len()as u64,original:&sites.local},
        Role{kind:Leaf::FullNode,name:f.original.node_name(),len:crate::ordinary::formal::physical::index_size(&f.original)?,original:&sites.node},
        Role{kind:Leaf::FullRaw,name:f.original.body_name(),len:f.original.content.bytes,original:&sites.raw},
        Role{kind:Leaf::Incr,name:d.original.leaf(),len:crate::ordinary::delta::physical::packed_size(&d.original)?,
            original:sites.incr.as_ref().ok_or_else(||invalid("a retained incremental node has no native role"))?},
    ])
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
pub(super) fn complete(v:&FileVolume<'_>,name:&CStr,m:&Moved,c:&Frame,f:&Commit,d:&Delta)->io::Result<HeldDir>{
    m.require_history(c,f,Some(d))?;root(v,&m.original.dst_root)?;
    if v.id()!=m.original.to{return Err(invalid("wrong registered MOVE destination"));}
    let dir=v.root.child_directory(name)?.ok_or_else(||invalid("decided FILE not yet installed at its original target"))?;
    material::verify_header_at(&dir,&c.create,&m.target.directory,&m.target.local)?;
    let rows=roles(c,f,d,&m.target)?;
    let want:BTreeSet<_>=rows.iter().map(|r|r.name.to_bytes().to_vec()).collect();
    let actual=dir.names()?;if actual.len()!=want.len()||actual.iter().any(|x|!want.contains(x)){
        return Err(invalid("missing or unknown material in relocated FILE"));
    }
    for r in &rows{let _=held_role(&dir,r,(m.original.mode==Method::Transfer).then_some(m.allocation))?;}
    dir.require_named()?;root(v,&m.original.dst_root)?;Ok(dir)
}
pub(in crate::ordinary) struct Current<'a>{pub dir:HeldDir,pub m:&'a Moved,pub f:&'a Commit,pub d:&'a Delta}
impl<'a> Current<'a>{
    pub(in crate::ordinary) fn open(s:&'a Journal,v:&FileVolume<'_>,c:&Frame)->io::Result<Option<Self>>{
        let Some(m)=s.placed(c.create.file) else{return Ok(None)};
        if let Some(a)=s.moving.as_ref().filter(|a|a.original.op==m.original.op){
            if !a.matches(m)||!a.opened(){return Err(io::Error::new(io::ErrorKind::WouldBlock,"MOVE decided; entire target has not yet been reopened"));}
            a.confirm_phase(v.pool)?;
        }
        let f=s.head(c.create.file).ok_or_else(||invalid("misplaced original FULL"))?;
        let d=s.delta(c.create.file).ok_or_else(||invalid("misplaced original INCR"))?;
        let name=CString::new(c.create.name.as_bytes()).map_err(|_|invalid("invalid selected name"))?;
        let dir=complete(v,&name,m,c,f,d)?;Ok(Some(Self{dir,m,f,d}))
    }
    pub(in crate::ordinary) fn full(&self)->io::Result<HistoryBody>{
        HistoryBody::at_originals(&self.dir,self.f,&self.m.target.node,&self.m.target.raw)?.user_read()
    }
    pub(in crate::ordinary) fn incremental(&self)->io::Result<Incremental>{
        let n=self.m.target.incr.as_ref().ok_or_else(||invalid("missing relocated INCR"))?;
        Incremental::at_original(&self.dir,self.full()?,self.d,n)
    }
}
/// A completed DATA scan of the exact installed name or private stage. The
/// transfer decision cannot replace it, and a string/checksum cannot mint it.
pub(super) struct TargetInput{clean:Clean,decision:Moved,name:CString}
pub(super) struct Reopened{verified:Verified,decision:Moved,name:CString}
impl Reopened{
    pub(super) fn input(v:&FileVolume<'_>,name:&CStr,m:&Moved,c:&Frame,f:&Commit,d:&Delta)->io::Result<TargetInput>{
        let dir=complete(v,name,m,c,f,d)?;dir.share()?;
        Ok(TargetInput{clean:Clean::at_original_sites(v,dir,c,f,Some(d),&m.target)?,decision:m.clone(),name:name.to_owned()})
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
    pub(super) fn verify_installed(&self,v:&FileVolume<'_>,m:&Moved)->io::Result<()>{
        let name=CString::new(m.original.name.as_bytes()).map_err(|_|invalid("invalid original target"))?;
        self.require(v,&name,m)
    }
    pub(super) fn files(&self)->&[super::source::Bytes]{self.verified.objects()}
}
