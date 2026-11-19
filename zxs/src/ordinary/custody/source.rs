use super::{admission::{LifecycleGrant,Side},command::Request,purpose::Call,transfer::{ChildRoles,Children,Sites}};
use super::super::{delta::{physical::Body as IncrBody,types::Delta},
    formal::{self,physical::HistoryBody,types::Commit},journal::{Graph,Journal,cycle::Parent},material,
    wire::{Frame,NativeObject,empty_material}};
use crate::{FileId,HistoryId,NodeId,PoolId,VolumeId,fs::{self,HeldDir,invalid,stream::{self,PinnedRegular}},
    pool::lease::{Access,FileVolume,LocalPool,recycle::CustodyEnd}};
use std::{collections::BTreeSet,ffi::{CStr,CString},io,path::Path};

/// The complete inventory of this narrow clean producer. There is exactly
/// one of each required role. No taking a prefix of a dirty/bigger container.
#[derive(Clone,Copy,Debug,Eq,Ord,PartialEq,PartialOrd)]
pub(in crate::ordinary::custody) enum Leaf{File,FullNode,FullRaw,Incr,IncrNext}
impl Leaf{pub(super) fn tag(self)->u8{match self{Self::File=>1,Self::FullNode=>2,Self::FullRaw=>3,Self::Incr=>4,Self::IncrNext=>5}}}
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
fn selection(s:&Journal,id:FileId)->io::Result<(&Frame,&Commit,Children<'_>)>{
    let c=s.file(id).ok_or_else(||io::Error::new(io::ErrorKind::NotFound,"no original FILE for transfer"))?;
    let parent=s.head(id).ok_or_else(||invalid("this MOVE source requires a clean, materialized first formal node"))?;
    let g=s.actual_graph(id)?;
    if s.placed(id).is_some()||s.allocated.as_ref().is_some_and(|a|a.file==id)||s.saving.as_ref().is_some_and(|a|a.original.file==id)
        ||s.committing.as_ref().is_some_and(|a|a.original.file==id)||s.editing.as_ref().is_some_and(|a|a.original.file==id)
        ||s.incrementing.as_ref().is_some_and(|a|a.original.file==id)||s.active_work(id)?.last().is_some(){
        return Err(invalid("dirty or unfinished FILE cannot provide a complete clean handoff"));
    }
    Ok((c,parent,g.children()))
}
/// Only a checked, complete native custody inventory can bind this second
/// child to its immediate parent in THIS source or not-yet-serving target.
/// It confers no live serving, placement or retirement authority.
pub(in crate::ordinary) struct FrozenSecond<'a>{dir:&'a HeldDir,parent:Parent,d:&'a Delta,object:&'a NativeObject}
impl FrozenSecond<'_>{
    pub(in crate::ordinary) fn parts(&self)->(&HeldDir,Parent,&Delta,&NativeObject){(self.dir,self.parent,self.d,self.object)}
}
pub(crate) struct Clean{
    pool:PoolId,volume:VolumeId,root:NativeObject,c:Frame,parent:Commit,children:ChildRoles<Delta>,
    lifecycle:Option<(Request,[u8;32],Sites)>,
    dir:HeldDir,local:PinnedRegular,first:HistoryBody,packs:ChildRoles<IncrBody>,
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
    #[cfg(test)]
    pub(super) fn capture_lifecycle(system:&Path,r:Request)->io::Result<Self>{Self::capture_checked(system,r,|_|Ok(()))}
    pub(super) fn capture_checked(system:&Path,r:Request,check:impl FnOnce(&LocalPool)->io::Result<()>)->io::Result<Self>{
        if r.call==Call::Move{return Err(invalid("ordinary MOVE uses its own source selection"));}
        let(p,_,_)=LocalPool::with_snapshot(system,Access::Incoming)?;check(&p)?;
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
        let f=s.head(r.id).ok_or_else(||invalid("source has no original main FULL"))?;let d=s.actual_graph(r.id)?.children();
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
    pub(super) fn at_original_sites(volume:&FileVolume<'_>,dir:HeldDir,c:&Frame,f:&Commit,d:Children<'_>,sites:&Sites)->io::Result<Self>{
        volume.require_current()?;let root=NativeObject{entity:volume.root.entity,birth:fs::Birth::read(&volume.root.file)?};
        let clean=Self::at_held_sites(volume.pool.id(),volume.id(),root,dir,c,f,d,sites)?;volume.require_current()?;Ok(clean)
    }
    pub(super) fn at_held_sites(pool:PoolId,volume:VolumeId,root:NativeObject,dir:HeldDir,c:&Frame,f:&Commit,d:Children<'_>,sites:&Sites)->io::Result<Self>{
        let g=Graph::from_roles(c,f,d)?;
        if sites.children.len()!=g.children().len(){return Err(invalid("incomplete original FILE placement"));}
        sites.check(&root)?;
        material::verify_header_at(&dir,&c.create,&sites.directory,&sites.local)?;
        let local=material::custody_lease_at(&dir,&c.create,&sites.local,false)?;
        let open=||HistoryBody::at_originals(&dir,f,&sites.node,&sites.raw);
        let first=open()?;let packs=match(d,sites.children.as_refs()){
            (ChildRoles::None,ChildRoles::None)=>ChildRoles::None,
            (ChildRoles::One(d),ChildRoles::One(n))=>ChildRoles::One(IncrBody::at_original(&dir,open()?,d,n)?),
            (ChildRoles::Two(older,newer),ChildRoles::Two(a,b))=>{
                // Keep a separate pinned view of the older version, even if
                // all its bytes are overwritten or truncated by its child.
                let one=IncrBody::at_original(&dir,open()?,older,a)?;
                let two=IncrBody::at_original(&dir,open()?,older,a)?.extend_frozen(&FrozenSecond{
                    dir:&dir,parent:Parent::incremental(older),d:newer,object:b})?;
                ChildRoles::Two(one,two)
            }
            _=>return Err(invalid("unexpected retained role")),
        };
        let result=Self{pool,volume,root,lifecycle:None,c:c.clone(),parent:f.clone(),children:d.map(Clone::clone),dir,local,first,packs};
        result.exact_names()?;Ok(result)
    }
    fn each(&self,mut apply:impl FnMut(Leaf,&CStr,&PinnedRegular)->io::Result<()>)->io::Result<()>{
        apply(Leaf::File,c"FILE",&self.local)?;
        let(node,raw)=self.first.native_files();
        apply(Leaf::FullNode,&self.parent.original.node_name(),node)?;
        apply(Leaf::FullRaw,&self.parent.original.body_name(),raw)?;
        if self.children.len()!=self.packs.len(){return Err(invalid("incomplete retained INCR inventory"));}
        for (i,(d,body)) in self.children.iter().zip(self.packs.iter()).enumerate(){
            let kind=match i{0=>Leaf::Incr,1=>Leaf::IncrNext,_=>return Err(invalid("unsupported formal ordinal"))};
            apply(kind,&d.original.leaf(),body.pack_of(Parent::incremental(d))?)?;
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
        self.dir.require_named()?;
        if self.dir.label()?.is_some(){return Err(invalid("a FILE transfer container cannot be a labelled volume root"));}Ok(())
    }
    /// Immediately before using a frozen source, compare to its *same* FILE
    /// original. Unrelated later FILE operations do not substitute a new cut.
    pub(super) fn require(&self,p:&LocalPool,s:&Journal,v:&FileVolume<'_>)->io::Result<()>{
        let h=crate::ordinary::OrdinaryParentLease::at_cut(v,self.c.create.parent.address(self.c.create.volume),s)?;
        self.require_parent(p,s,&h)
    }
    fn require_parent(&self,p:&LocalPool,s:&Journal,h:&crate::ordinary::OrdinaryParentLease<'_,'_>)->io::Result<()>{
        let v=h.volume();
        if !self.c.create.parent.identifies(v.id(),h.basis())||!std::ptr::eq(p,v.pool){return Err(invalid("not the selected original SYSTEM lease"));}
        if self.lifecycle.is_some(){return Err(invalid("a lifecycle source requires its scoped original endpoint"));}
        let(c,f,d)=selection(s,self.c.create.file)?;
        if p.id()!=self.pool||v.id()!=self.c.create.volume||c.digest!=self.c.digest||f.digest!=self.parent.digest||
            d.map(|v|v.digest)!=self.children.as_refs().map(|v|v.digest)||v.root.entity!=self.root.entity||
            fs::Birth::read(&v.root.file)?!=self.root.birth||self.dir.entity!=c.create.directory.entity||
            fs::Birth::read(&self.dir.file)?!=c.create.directory.birth||self.dir.path!=h.directory().checked_child_path(&crate::ordinary::storage_name::file_leaf(&c.create.name,c.create.file)?)?{
            return Err(invalid("handoff source no longer belongs to its original FILE/root/frontier"));
        }
        let n=crate::ordinary::storage_name::file_leaf(&c.create.name,c.create.file)?;
        let original=h.exact_child(&n)?.ok_or_else(||invalid("original source spelling is not present"))?;
        if original.entity!=self.dir.entity||!original.same_mount(&self.dir)?||fs::Birth::read(&original.file)?!=c.create.directory.birth{
            return Err(invalid("original FILE source was renamed or replaced"));}
        h.require_at(s)?;self.exact_names()?;h.require_at(s)
    }
    fn require_lifecycle(&self,p:&LocalPool,g:&LifecycleGrant<'_>,v:&CustodyEnd<'_, '_>)->io::Result<()>{
        let Some((r,prior,sites))=&self.lifecycle else{return Err(invalid("not a frozen lifecycle source"));};
        let s=g.predecision(p)?;
        if !v.belongs(g,Side::Source)||*r!=g.request()||p.id()!=self.pool||v.id()!=self.volume||
            v.id()!=r.from||g.root(Side::Source)!=&self.root||g.prior()!=*prior||g.sites()!=sites||
            s.file(r.id).map(|c|c.digest)!=Some(self.c.digest)||s.head(r.id).map(|f|f.digest)!=Some(self.parent.digest)||
            s.actual_graph(r.id)?.hashes()!=self.children.as_refs().map(|d|d.digest){
            return Err(invalid("different lifecycle operation, source or formal history"));}
        v.check_source(&self.dir)?;Self::previous_labels(&s,v,g,&self.dir,&self.c)?;
        self.exact_names()?;v.check_source(&self.dir)
    }
    pub(super) fn verify(self)->io::Result<Verified>{
        self.exact_names()?;
        // Verify the ENTIRE first FULL even if a child overwrites all of it.
        // Verifying only the latest user-visible bytes would lose history.
        self.first.write_to(io::sink())?;
        for body in self.packs.iter(){body.whole(io::sink())?;}
        let mut bytes=Vec::with_capacity(3+self.children.len());let mut total=0u64;
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
                Leaf::Incr|Leaf::IncrNext=>read_physical(pin,len,|_,_|Ok(()))?,
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
    pub(super) fn originals(&self)->(&Frame,&Commit,Children<'_>){(&self.source.c,&self.source.parent,self.source.children.as_refs())}
    pub(super) fn require_at(&self,h:&crate::ordinary::OrdinaryParentLease<'_,'_>,name:&CStr,root:&NativeObject,sites:&Sites)->io::Result<()>{
        let v=h.volume();use std::os::unix::ffi::OsStrExt;
        if v.pool.id()!=self.pool()||self.source.root!=*root||v.root.entity!=root.entity||
            fs::Birth::read(&v.root.file)?!=root.birth||self.source.dir.path!=h.directory().path.join(std::ffi::OsStr::from_bytes(name.to_bytes()))||
            self.source.dir.entity!=sites.directory.entity||fs::Birth::read(&self.source.dir.file)?!=sites.directory.birth||
            self.objects().len()!=3+sites.children.len()||self.objects().iter().any(|b|sites.at(b.kind)!=Some(&b.original)){
                return Err(invalid("reopened material is not the original MOVE role, directory and exact named content"));
        }
        let actual=h.exact_child(name)?.ok_or_else(||invalid("original MOVE operand no longer has its exact spelling"))?;
        if actual.entity!=self.source.dir.entity||!actual.same_mount(&self.source.dir)?||fs::Birth::read(&actual.file)?!=sites.directory.birth{
            return Err(invalid("not this exact originally named MOVE operand"));}
        h.require()?;self.source.exact_names()?;h.require()
    }
    /// Only the parent entry changed. The directory FD, original held regular
    /// files, their full stamps, and their applicable read leases did not.
    /// A new process has no such Verified and must do a fresh complete scan.
    pub(super) fn follow_transfer_rename(&mut self,v:&crate::ordinary::OrdinaryParentLease<'_,'_>,m:&super::transfer::Moved,old:&CStr,new:&CStr)->io::Result<()>{
        if self.source.dir.path!=v.directory().checked_child_path(old)?||v.volume().id()!=m.original.to||
            self.volume()!=v.volume().id()||self.file()!=m.original.file{return Err(invalid("different verified transfer target at rename"));}
        self.source.dir.path=v.directory().checked_child_path(new)?;
        // Resolves the exact final spelling and matches directory birth/mount,
        // all frozen roles, every reopened child against its retained pin, and
        // the actual member set. It does not take a fresh stamp as its proof.
        self.require_at(v,new,&m.original.dst_root,&m.target)?;
        self.source.each(|kind,_,pin|{
            let original=NativeObject::stamp(&pin.stamp);
            fs::require_material_token(&pin.file,&super::placed::token(m.allocation,kind,&original)?)
        })
    }
    pub(super) fn require_at_lifecycle(&self,end:&CustodyEnd<'_,'_>,goal:&super::target::Goal)->io::Result<()>{
        let m=&goal.frame;let sites=&m.target;
        if self.volume()!=end.id()||self.source.root!=m.original.dst_root||self.file()!=m.original.file||
            self.source.dir.entity!=sites.directory.entity||fs::Birth::read(&self.source.dir.file)?!=sites.directory.birth||
            self.objects().len()!=3+sites.children.len()||self.objects().iter().any(|row|sites.at(row.kind)!=Some(&row.original)){
            return Err(invalid("not the selected lifecycle destination incarnation"));}
        end.check_target_child(goal,&self.source.dir)?;self.source.exact_names()?;end.check_target_child(goal,&self.source.dir)
    }
    pub(super) fn digest(&self)->io::Result<[u8;32]>{
        let mut h=crate::hash::Sha256::new();h.update(b"ZXS/FILE/ALL-CLEAN-MATERIAL\0")?;
        let(c,parent,d)=self.originals();h.update(&c.digest)?;h.update(&parent.digest)?;
        h.update(&[d.len() as u8])?;for child in d.iter(){h.update(&child.digest)?;}
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
    pub(super) fn head(&self)->NodeId{self.source.children.iter().last().map_or(self.source.parent.original.node,|d|d.original.node)}
    pub(super) fn objects(&self)->&[Bytes]{&self.bytes}
    /// Use the actual ordered formal graph and the source addresses from this
    /// original custody decision, never a standalone length interval.
    pub(super) fn allocation_count(&self,sites:&Sites)->io::Result<usize>{
        let(c,f,ds)=self.originals();let roles=super::placed::roles(c,f,ds,sites)?;
        if roles.len()!=self.bytes.len()||roles.iter().zip(&self.bytes).any(|(r,b)|
            r.kind!=b.kind||r.name!=b.name||r.len!=b.len||r.original!=&b.original){
            return Err(invalid("the complete original FILE roles, names, sizes and native objects are required before allocation"));
        }
        Ok(roles.len())
    }
    pub(super) fn physical_bytes(&self)->u64{self.total}
    pub(super) fn require(&self,pool:&LocalPool,s:&Journal,volume:&FileVolume<'_>)->io::Result<()>{self.source.require(pool,s,volume)}
    pub(super) fn require_parent(&self,p:&LocalPool,s:&Journal,h:&crate::ordinary::OrdinaryParentLease<'_,'_>)->io::Result<()>{self.source.require_parent(p,s,h)}
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
