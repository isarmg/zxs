use super::{admission::{LifecycleGrant,Side},command::Request,purpose::Call,transfer::Sites};
use super::super::{delta::{physical::Body as IncrBody,types::Delta},
    formal::{self,physical::HistoryBody,types::Commit},journal::Journal,material,
    wire::{Frame,NativeObject,empty_material}};
use crate::{FileId,HistoryId,NodeId,PoolId,VolumeId,fs::{self,HeldDir,invalid,stream::{self,PinnedRegular}},
    pool::lease::{Access,FileVolume,LocalPool,recycle::CustodyEnd}};
use std::{collections::BTreeSet,ffi::{CStr,CString},io,path::Path};

/// The complete inventory of this narrow clean producer. There is exactly
/// one of each required role. No taking a prefix of a dirty/bigger container.
#[derive(Clone,Copy,Debug,Eq,Ord,PartialEq,PartialOrd)]
pub(in crate::ordinary::custody) enum Leaf{File,FullNode,FullRaw,Incr}
impl Leaf{pub(super) fn tag(self)->u8{match self{Self::File=>1,Self::FullNode=>2,Self::FullRaw=>3,Self::Incr=>4}}}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(super) struct Bytes{pub kind:Leaf,pub name:CString,pub original:NativeObject,pub len:u64,pub digest:[u8;32]}

/// Read/validate the actual pinned stream; the receiver never chooses a path.
/// Progress is checked around each source buffer; a temporary target never
/// makes a partial failure a verified copy.
pub(super) fn read_physical(pin:&PinnedRegular,expected:u64,mut part:impl FnMut(u64,&[u8])->io::Result<()>)->io::Result<[u8;32]>{
    pin.unchanged()?;if expected!=pin.stamp.stat.size(){return Err(invalid("wrong source physical object length"));}
    let mut h=crate::hash::Sha256::new();let mut buffer=vec![0;expected.min(1024*1024) as usize];let mut at=0;
    while at<expected{
        let n=(expected-at).min(buffer.len() as u64) as usize;let b=&mut buffer[..n];
        stream::read_at(&pin.file,b,at)?;pin.unchanged()?;h.update(b)?;part(at,b)?;pin.unchanged()?;at+=n as u64;
    }
    pin.unchanged()?;Ok(h.finish())
}
fn selection(s:&Journal,id:FileId)->io::Result<(&Frame,&Commit,Option<&Delta>)>{
    let c=s.file(id).ok_or_else(||io::Error::new(io::ErrorKind::NotFound,"no original FILE for transfer"))?;
    let parent=s.head(id).ok_or_else(||invalid("this MOVE source requires a clean, materialized first formal node"))?;
    let d=s.delta(id);
    if s.placed(id).is_some()||s.allocated.as_ref().is_some_and(|a|a.file==id)||s.saving.as_ref().is_some_and(|a|a.original.file==id)
        ||s.committing.as_ref().is_some_and(|a|a.original.file==id)||s.editing.as_ref().is_some_and(|a|a.original.file==id)
        ||s.incrementing.as_ref().is_some_and(|a|a.original.file==id)||(d.is_none()&&s.interval(id).is_some()){
        return Err(invalid("dirty or unfinished FILE cannot provide a complete clean handoff"));
    }
    Ok((c,parent,d))
}
pub(crate) struct Clean{
    pool:PoolId,volume:VolumeId,root:NativeObject,c:Frame,parent:Commit,delta:Option<Delta>,
    lifecycle:Option<(Request,[u8;32],Sites)>,
    dir:HeldDir,local:PinnedRegular,first:HistoryBody,second:Option<IncrBody>,
}
impl Clean{
    /// The FILE's existing directory writer lease prevents a new operation
    /// from modifying this FILE while we scan, but no SYSTEM/volume lock is
    /// held across content reads. Nonparticipants can never grant a new owner.
    pub(super) fn capture(system:&Path,id:FileId)->io::Result<Self>{
        let (pool,_,s)=LocalPool::with_snapshot(system,Access::Incoming)?;let(c,f,d)=selection(&s,id)?;
        let volume=pool.ordinary(c.create.volume)?;
        let saved=s.saved(id).ok_or_else(||invalid("formal FILE lacks original working genesis"))?;
        let dir=formal::directory(&volume,&s,c,saved)?;dir.exclusive()?;
        let result=Self::at_original_sites(&volume,dir,c,f,d,&Sites::birthplace(c,f,d))?;
        result.require(&pool,&s,&volume)?;Ok(result)
    }
    /// Read the actual current source of a new/pending paired handoff. No
    /// source fallback when R is unavailable, and no opening A just because
    /// a later B-native FILE/INCR was first created there. A completed
    /// decision must use target observation and exact retirement instead.
    pub(super) fn capture_lifecycle(system:&Path,r:Request)->io::Result<Self>{
        if r.call==Call::Move{return Err(invalid("ordinary MOVE uses its own source selection"));}
        let(p,_,_)=LocalPool::with_snapshot(system,Access::Incoming)?;
        let grant=LifecycleGrant::capture(&p,r)?;let end=p.custody_end(&grant,Side::Source)?;
        end.read_clean_source()
    }
    /// Called only inside the original endpoint's fixed-name reader. The
    /// child is the selected FILE, never the recycle root or a supplied path.
    pub(super) fn from_lifecycle(p:&LocalPool,g:&LifecycleGrant<'_>,end:&CustodyEnd<'_, '_>,dir:HeldDir)->io::Result<Self>{
        let s=g.predecision(p)?;let r=g.request();
        if !end.belongs(g,Side::Source){return Err(invalid("not this predecision source reader"));}
        end.check_source(&dir)?;dir.exclusive()?;end.check_source(&dir)?;
        let c=s.file(r.id).ok_or_else(||invalid("original FILE unavailable"))?;
        let f=s.head(r.id).ok_or_else(||invalid("source has no original main FULL"))?;let d=s.delta(r.id);
        Self::previous_labels(&s,end,g,&dir,c)?;
        let mut clean=Self::at_held_sites(p.id(),end.id(),g.root(Side::Source).clone(),dir,c,f,d,g.sites())?;
        clean.lifecycle=Some((r,g.prior(),g.sites().clone()));
        clean.require_lifecycle(p,g,end)?;Ok(clean)
    }
    fn previous_labels(s:&Journal,v:&CustodyEnd<'_, '_>,g:&LifecycleGrant<'_>,dir:&HeldDir,c:&Frame)->io::Result<()>{
        if let Some(m)=s.placed(c.create.file){
            let(_,f,d)=super::finish::frozen(s,m)?;
            let(_,installed)=m.original.purpose.leaves(c.create.file,m.original.op,&c.create.name)?;
            if m.original.to!=v.id()||installed.as_c_str()!=g.name(Side::Source)||m.original.dst_root!=*g.root(Side::Source)||
                dir.entity!=m.target.directory.entity||fs::Birth::read(&dir.file)?!=m.target.directory.birth{
                return Err(invalid("current custody source differs from its last exact installed original"));}
            for role in super::placed::roles(c,f,d,&m.target)?{
                let _=super::placed::held_role(dir,&role,(m.original.mode==super::Method::Transfer).then_some(m.allocation))?;
            }
        }else if c.create.volume!=v.id()||v.role()!=crate::Role::File{
            return Err(invalid("FILE has no prior handoff to the offered lifecycle source"));}
        Ok(())
    }
    /// Shared reader, not another authority. The caller supplies the true
    /// selected location and native roles from the preceding original decision
    /// or the exact held source/target handoff prior to that decision.
    pub(super) fn at_original_sites(volume:&FileVolume<'_>,dir:HeldDir,c:&Frame,f:&Commit,d:Option<&Delta>,sites:&Sites)->io::Result<Self>{
        volume.require_current()?;let root=NativeObject{entity:volume.root.entity,birth:fs::Birth::read(&volume.root.file)?};
        let clean=Self::at_held_sites(volume.pool.id(),volume.id(),root,dir,c,f,d,sites)?;volume.require_current()?;Ok(clean)
    }
    fn at_held_sites(pool:PoolId,volume:VolumeId,root:NativeObject,dir:HeldDir,c:&Frame,f:&Commit,d:Option<&Delta>,sites:&Sites)->io::Result<Self>{
        if sites.incr.is_some()!=d.is_some(){return Err(invalid("incomplete original FILE placement"));}
        material::verify_header_at(&dir,&c.create,&sites.directory,&sites.local)?;
        let local=material::custody_lease_at(&dir,&c.create,&sites.local,false)?;
        let open=||HistoryBody::at_originals(&dir,f,&sites.node,&sites.raw);
        let first=open()?;let second=match(d,&sites.incr){(Some(d),Some(n))=>Some(IncrBody::at_original(&dir,open()?,d,n)?),
            (None,None)=>None,_=>return Err(invalid("unexpected retained role"))};
        sites.check(&root)?;
        let result=Self{pool,volume,root,lifecycle:None,c:c.clone(),parent:f.clone(),delta:d.cloned(),dir,local,first,second};
        result.exact_names()?;Ok(result)
    }
    fn each(&self,mut apply:impl FnMut(Leaf,&CStr,&PinnedRegular)->io::Result<()>)->io::Result<()>{
        apply(Leaf::File,c"FILE",&self.local)?;
        let(node,raw)=self.first.native_files();
        apply(Leaf::FullNode,&self.parent.original.node_name(),node)?;
        apply(Leaf::FullRaw,&self.parent.original.body_name(),raw)?;
        match(&self.delta,&self.second){
            (Some(d),Some(body))=>apply(Leaf::Incr,&d.original.leaf(),body.native_pack())?,
            (None,None)=>{},_=>return Err(invalid("incomplete selected formal INCR")),
        }Ok(())
    }
    fn exact_names(&self)->io::Result<()>{
        self.dir.require_named()?;let mut expected=BTreeSet::new();
        self.each(|_,name,pin|{
            if !expected.insert(name.to_bytes().to_vec()){return Err(invalid("formal role names collide"));}
            pin.named(&self.dir,name)
        })?;
        let listing=self.dir.names()?;let n=listing.len();
        let actual:BTreeSet<_>=listing.into_iter().collect();
        if actual.len()!=n||actual!=expected{return Err(invalid("unsupported, missing or extra original transfer material"));}
        self.dir.require_named()
    }
    /// Immediately before using a frozen source, compare to its *same* FILE
    /// original. Unrelated later FILE operations do not substitute a new cut.
    pub(super) fn require(&self,p:&LocalPool,s:&Journal,v:&FileVolume<'_>)->io::Result<()>{
        if !std::ptr::eq(p,v.pool){return Err(invalid("not the selected original SYSTEM lease"));}
        if self.lifecycle.is_some(){return Err(invalid("a lifecycle source requires its scoped original endpoint"));}
        let(c,f,d)=selection(s,self.c.create.file)?;
        if p.id()!=self.pool||v.id()!=self.c.create.volume||c.digest!=self.c.digest||f.digest!=self.parent.digest||
            d.map(|v|v.digest)!=self.delta.as_ref().map(|v|v.digest)||v.root.entity!=self.root.entity||
            fs::Birth::read(&v.root.file)?!=self.root.birth||self.dir.entity!=c.create.directory.entity||
            fs::Birth::read(&self.dir.file)?!=c.create.directory.birth||self.dir.path!=v.root.path.join(&c.create.name){
            return Err(invalid("handoff source no longer belongs to its original FILE/root/frontier"));
        }
        v.require_current()?;self.exact_names()?;v.require_current()
    }
    fn require_lifecycle(&self,p:&LocalPool,g:&LifecycleGrant<'_>,v:&CustodyEnd<'_, '_>)->io::Result<()>{
        let Some((r,prior,sites))=&self.lifecycle else{return Err(invalid("not a frozen lifecycle source"));};
        let s=g.predecision(p)?;
        if !v.belongs(g,Side::Source)||*r!=g.request()||p.id()!=self.pool||v.id()!=self.volume||
            v.id()!=r.from||g.root(Side::Source)!=&self.root||g.prior()!=*prior||g.sites()!=sites||
            s.file(r.id).map(|c|c.digest)!=Some(self.c.digest)||s.head(r.id).map(|f|f.digest)!=Some(self.parent.digest)||
            s.delta(r.id).map(|d|d.digest)!=self.delta.as_ref().map(|d|d.digest){
            return Err(invalid("different lifecycle operation, source or formal history"));}
        v.check_source(&self.dir)?;Self::previous_labels(&s,v,g,&self.dir,&self.c)?;
        self.exact_names()?;v.check_source(&self.dir)
    }
    pub(super) fn verify(self)->io::Result<Verified>{
        self.exact_names()?;
        // Verify the ENTIRE first FULL even if a child overwrites all of it.
        // Verifying only the latest user-visible bytes would lose history.
        self.first.write_to(io::sink())?;
        if let Some(body)=&self.second{body.whole(io::sink())?;}
        let mut bytes=Vec::with_capacity(4);let mut total=0u64;
        self.each(|kind,name,pin|{
            let len=pin.stamp.stat.size();
            let digest=match kind{
                Leaf::File=>{
                    let logical=empty_material(self.c.create.file,self.c.create.history,self.c.create.op,&self.c.create.name);
                    if len!=logical.len() as u64||self.c.create.local_digest!=crate::hash::digest(&logical)?{
                        return Err(invalid("original FILE local birth differs"));
                    }
                    self.c.create.local_digest
                }
                // Full verification above read *these exact two held files*.
                Leaf::FullNode=>self.parent.materials.index_digest,
                Leaf::FullRaw=>self.parent.original.content.sha256,
                // material.index is NOT the raw package SHA. Raw packing is
                // checked separately, including node, map, dense bytes, links.
                Leaf::Incr=>read_physical(pin,len,|_,_|Ok(()))?,
            };
            total=total.checked_add(len).ok_or_else(||invalid("transfer material byte total overflow"))?;
            bytes.push(Bytes{kind,name:name.to_owned(),original:NativeObject::stamp(&pin.stamp),len,digest});Ok(())
        })?;
        self.exact_names()?;Ok(Verified{source:self,bytes,total})
    }
}
/// Not serializable. Raw checksums, inode numbers, file names or this inventory
/// by themselves cannot become a new serving owner or authorize cleanup.
pub(in crate::ordinary::custody) struct Verified{source:Clean,bytes:Vec<Bytes>,total:u64}
impl Verified{
    pub(super) fn originals(&self)->(&Frame,&Commit,Option<&Delta>){(&self.source.c,&self.source.parent,self.source.delta.as_ref())}
    pub(super) fn require_at(&self,v:&FileVolume<'_>,name:&CStr,root:&NativeObject,sites:&Sites)->io::Result<()>{
        use std::os::unix::ffi::OsStrExt;
        if v.pool.id()!=self.pool()||self.source.root!=*root||v.root.entity!=root.entity||
            fs::Birth::read(&v.root.file)?!=root.birth||self.source.dir.path!=v.root.path.join(std::ffi::OsStr::from_bytes(name.to_bytes()))||
            self.source.dir.entity!=sites.directory.entity||fs::Birth::read(&self.source.dir.file)?!=sites.directory.birth||
            self.objects().iter().any(|b|sites.at(b.kind)!=Some(&b.original)){
                return Err(invalid("reopened material is not the original MOVE role, directory and exact named content"));
        }
        v.require_current()?;self.source.exact_names()?;v.require_current()
    }
    pub(super) fn digest(&self)->io::Result<[u8;32]>{
        let mut h=crate::hash::Sha256::new();h.update(b"ZXS/FILE/ALL-CLEAN-MATERIAL\0")?;
        let(c,parent,d)=self.originals();h.update(&c.digest)?;h.update(&parent.digest)?;
        h.update(&[u8::from(d.is_some())])?;if let Some(d)=d{h.update(&d.digest)?;}
        for role in self.objects(){
            let n=u8::try_from(role.name.to_bytes().len()).map_err(|_|invalid("noncanonical transfer name length"))?;
            h.update(&[role.kind.tag(),n])?;h.update(role.name.to_bytes())?;h.update(&role.len.to_be_bytes())?;h.update(&role.digest)?;
        }
        Ok(h.finish())
    }
    pub(super) fn pool(&self)->PoolId{self.source.pool}
    pub(super) fn name(&self)->&str{&self.source.c.create.name}
    pub(super) fn source_root(&self)->&NativeObject{&self.source.root}
    pub(super) fn same_mount(&self,root:&HeldDir)->io::Result<bool>{self.source.dir.same_mount(root)}
    pub(super) fn file(&self)->FileId{self.source.c.create.file}
    pub(super) fn history(&self)->HistoryId{self.source.c.create.history}
    pub(super) fn volume(&self)->VolumeId{self.source.volume}
    pub(super) fn head(&self)->NodeId{self.source.delta.as_ref().map_or(self.source.parent.original.node,|d|d.original.node)}
    pub(super) fn objects(&self)->&[Bytes]{&self.bytes}
    pub(super) fn physical_bytes(&self)->u64{self.total}
    pub(super) fn require(&self,pool:&LocalPool,s:&Journal,volume:&FileVolume<'_>)->io::Result<()>{self.source.require(pool,s,volume)}
    pub(super) fn require_lifecycle(&self,p:&LocalPool,g:&LifecycleGrant<'_>,v:&CustodyEnd<'_, '_>)->io::Result<()>{
        self.source.require_lifecycle(p,g,v)
    }
    /// Supplies only the named immutable original from the actual checked
    /// source, not a selector for a new name/path, and rechecks the raw hash.
    pub(super) fn stream(&self,required:Leaf,mut output:impl FnMut(u64,&[u8])->io::Result<()>)->io::Result<()>{
        let fact=self.bytes.iter().find(|x|x.kind==required).ok_or_else(||invalid("unsupported FILE transfer role"))?;
        let mut found=false;self.source.each(|kind,name,pin|{
            if kind==required{
                if found||name!=fact.name.as_c_str()||NativeObject::stamp(&pin.stamp)!=fact.original{
                    return Err(invalid("selected source no longer has its exact transfer role"));
                }
                pin.named(&self.source.dir,name)?;
                if read_physical(pin,fact.len,&mut output)?!=fact.digest{return Err(invalid("transferred physical bytes do not match original proof"));}
                pin.named(&self.source.dir,name)?;found=true;
            }Ok(())
        })?;
        if !found{return Err(invalid("exact FILE transfer role disappeared"));}Ok(())
    }
}
#[cfg(test)]
#[path="../../../tests/unit/custody_source.rs"]
mod tests;
