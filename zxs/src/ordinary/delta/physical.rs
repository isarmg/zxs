//! One immutable object, named from the original VersionStorageId + creation
//! name. Its own local header/node/map and checked dense change stream do not
//! borrow even one byte from a working extent after publication.
use super::{allocation::DeltaAllocation,map::{Map,Source as ByteSource},types::{Content,Delta,Material,Original}};
use crate::{fs::{self,HeldDir,invalid,stream::{self,PinnedRegular}},ordinary::{content::{self,BLOCK},draft::types::DraftClaim,
    formal::{PinnedParent,physical::HistoryBody},indexed::Indexed,interval::{physical::{Composed,cleanup_witness,retire_verified},types::Saved},output::CheckedOutput,wire::NativeObject}};
use std::{fs::File,io::{self,Write}};

fn shape(o:&Original)->io::Result<(u64,u64,u64)>{
    let h=(o.prefix(&Content::default()).len()+Map::checked_bytes(o.rows)?) as u64;let t=h.checked_add(o.changed).ok_or_else(||invalid("INCR offset overflow"))?;
    let end=content::chunks(o.changed).checked_mul(32).and_then(|n|n.checked_add(t)).filter(|x|*x<=i64::MAX as u64).ok_or_else(||invalid("INCR exceeds native space"))?;
    Ok((h,t,end))
}
pub(in crate::ordinary) fn packed_size(o:&Original)->io::Result<u64>{Ok(shape(o)?.2)}
fn header(f:&File,o:&Original,m:&Map)->io::Result<(Content,Vec<u8>)>{
    let mut bytes=vec![0;shape(o)?.0 as usize];stream::read_at(f,&mut bytes,0)?;
    let tail=4+m.rows() as usize*9;let at=bytes.len()-tail-96;
    let c=Content{logical:bytes[at..at+32].try_into().unwrap(),packed:bytes[at+32..at+64].try_into().unwrap(),head:bytes[at+64..at+96].try_into().unwrap()};
    if bytes!=o.header(m,&c){return Err(invalid("formal INCR local node, parent or map is not its original"));}Ok((c,bytes))
}
fn token(a:&DeltaAllocation,obj:&NativeObject)->io::Result<[u8;32]>{
    let mut b=b"ZXS/FORMAL/INCR/PREPARED\0".to_vec();b.extend_from_slice(&a.digest);obj.put(&mut b);crate::hash::digest(&b)
}
fn scan(f:&File,o:&Original,m:&Map,check:impl FnMut()->io::Result<()>)->io::Result<(Content,[u8;32])>{
    let (c,h)=header(f,o,m)?;let mut hash=crate::hash::Sha256::new();hash.update(&h)?;let (raw,table,_)=shape(o)?;
    content::visit(DraftClaim{bytes:o.changed,sha256:c.packed},c.head,|i,buf,next|{
        stream::read_at(f,buf,raw+i*BLOCK)?;stream::read_at(f,next,table+i*32)?;hash.update(next)
    },check,|_,_,_|Ok(()))?;
    Ok((c,hash.finish()))
}
fn seal_links(f:&File,raw:u64,table:u64,claim:DraftClaim)->io::Result<[u8;32]>{
    let mut head=content::end(claim)?;let mut buf=vec![0;claim.bytes.min(BLOCK) as usize];
    for i in (0..content::chunks(claim.bytes)).rev(){
        let n=(claim.bytes-i*BLOCK).min(BLOCK) as usize;stream::read_at(f,&mut buf[..n],raw+i*BLOCK)?;
        stream::write_at(f,&head,table+i*32)?;head=content::link(i,n,crate::hash::digest(&buf[..n])?,head)?;
    }Ok(head)
}
struct Held{key:crate::ordinary::journal::cycle::Parent,pin:PinnedRegular,o:Original,m:Map,c:Content,index:[u8;32]}
impl Held{
    fn at_original(dir:&HeldDir,d:&Delta,object:&NativeObject)->io::Result<Self>{
        let o=&d.original;let pin=dir.pin_stream(&o.leaf(),shape(o)?.2)?.ok_or_else(||invalid("actual published INCR missing"))?;
        if &NativeObject::stamp(&pin.stamp)!=object{return Err(invalid("different physical INCR original"));}
        pin.file.try_lock_shared().map_err(io::Error::from)?;
        let mut bytes=vec![0;Map::checked_bytes(o.rows)?];stream::read_at(&pin.file,&mut bytes,o.prefix(&Content::default()).len() as u64)?;
        let map=Map::decode(&bytes,o.rows,o.parent_length,o.logical,o.changed)?;
        if map.digest()?!=o.map{return Err(invalid("actual local INCR map differs from its original decision"));}
        let (c,_)=header(&pin.file,o,&map)?;
        if c!=d.material.content{return Err(invalid("different original INCR commitments"));}
        pin.named(dir,&o.leaf())?;Ok(Self{key:crate::ordinary::journal::cycle::Parent::incremental(d),pin,o:o.clone(),m:map,c,index:d.material.index})
    }
    fn range(&self,at:u64,n:u64,out:impl Write)->io::Result<()>{
        let (raw,links,_)=shape(&self.o)?;
        Indexed{raw:&self.pin.file,index:&self.pin.file,header:self.o.header(&self.m,&self.c),raw_start:raw,link_start:links,
            claim:DraftClaim{bytes:self.o.changed,sha256:self.c.packed},head:self.c.head,digest:self.index}
            .write_range(at,n,out,||self.pin.unchanged())
    }
}
pub(in crate::ordinary) struct Body{parent:HistoryBody,first:Held,second:Option<Held>}
impl Body{
    pub(in crate::ordinary) fn open(dir:&HeldDir,parent:HistoryBody,d:&Delta)->io::Result<Self>{
        Self::at_original(dir,parent,d,&d.material.object)
    }
    pub(in crate::ordinary) fn at_original(dir:&HeldDir,parent:HistoryBody,d:&Delta,object:&NativeObject)->io::Result<Self>{
        if !parent.confirmed_parent()?.precedes(d){return Err(invalid("wrong actual FULL parent for incremental node"));}
        parent.require(dir)?;let first=Held::at_original(dir,d,object)?;
        let ret=Self{parent,first,second:None};ret.require(dir)?;Ok(ret)
    }
    /// Private successor: only a node from the same actual confirmed FILE
    /// cycle, not a hand-built Delta or an arbitrary latest node. At most
    /// two immutable children are supported by ordinary reads and custody.
    pub(in crate::ordinary) fn extend(self,at:&crate::ordinary::formal::PlacedSecond<'_>)->io::Result<Self>{
        let(dir,parent,d,object)=at.parts();self.follow(dir,parent,d,object)
    }
    pub(in crate::ordinary) fn extend_frozen(self,at:&crate::ordinary::custody::FrozenSecond<'_>)->io::Result<Self>{
        let(dir,parent,d,object)=at.parts();self.follow(dir,parent,d,object)
    }
    fn follow(mut self,dir:&HeldDir,parent:crate::ordinary::journal::cycle::Parent,d:&Delta,object:&NativeObject)->io::Result<Self>{
        if self.second.is_some()||self.first.key!=parent||!parent.precedes(d){return Err(invalid("wrong physical INCR parent; never skip a formal edge"));}
        self.require(dir)?;let child=Held::at_original(dir,d,object)?;self.second=Some(child);self.require(dir)?;Ok(self)
    }
    fn top(&self)->&Held{self.second.as_ref().unwrap_or(&self.first)}
    pub(in crate::ordinary) fn pack_of(&self,wanted:crate::ordinary::journal::cycle::Parent)->io::Result<&PinnedRegular>{
        let tip=self.top();if tip.key!=wanted{return Err(invalid("wrong retained pack ordinal"));}Ok(&tip.pin)
    }
    pub(in crate::ordinary) fn unchanged(&self)->io::Result<()>{
        self.parent.unchanged()?;self.first.pin.unchanged()?;if let Some(n)=&self.second{n.pin.unchanged()?;}Ok(())
    }
    pub(in crate::ordinary) fn require(&self,dir:&HeldDir)->io::Result<()>{
        self.parent.require(dir)?;self.first.pin.named(dir,&self.first.o.leaf())?;
        if let Some(n)=&self.second{n.pin.named(dir,&n.o.leaf())?;}Ok(())
    }
    pub(in crate::ordinary) fn range(&self,at:u64,len:u64,out:impl Write)->io::Result<()>{
        self.unchanged()?;self.parent.write_range(0,0,io::sink())?;self.first.range(0,0,io::sink())?;
        if let Some(n)=&self.second{n.range(0,0,io::sink())?;}
        let mut out=CheckedOutput{out,check:||self.unchanged()};let zeros=[0u8;64*1024];
        let older=self.second.as_ref().map(|_|&self.first.m);
        self.top().m.resolve(older,self.parent.confirmed_parent()?.length,at,len,|from,offset,n|match from{
            ByteSource::Full=>self.parent.write_range(offset,n,&mut out),
            ByteSource::First=>self.first.range(offset,n,&mut out),
            ByteSource::Second=>self.second.as_ref().ok_or_else(||invalid("missing second immutable pack"))?.range(offset,n,&mut out),
            ByteSource::Zero=>{let mut left=n;while left!=0{let size=left.min(zeros.len() as u64) as usize;out.write_all(&zeros[..size])?;left-=size as u64;}Ok(())}
        })?;self.unchanged()
    }
    pub(in crate::ordinary) fn whole(&self,out:impl Write)->io::Result<()>{
        struct HashOut<W>{out:W,h:crate::hash::Sha256}
        impl<W:Write> Write for HashOut<W>{
            fn write(&mut self,b:&[u8])->io::Result<usize>{let n=self.out.write(b)?;self.h.update(&b[..n])?;Ok(n)}
            fn flush(&mut self)->io::Result<()>{self.out.flush()}
        }
        let mut out=HashOut{out,h:crate::hash::Sha256::new()};
        self.range(0,self.top().o.logical,&mut out)?;
        if out.h.finish()!=self.top().c.logical{return Err(invalid("composed formal INCR differs from its original logical bytes"));}Ok(())
    }
}
enum Target{Private(File),Named(PinnedRegular)}
impl Target{
    fn file(&self)->&File{match self{Self::Private(f)=>f,Self::Named(p)=>&p.file}}
    fn stamp(&self)->io::Result<fs::OriginalStamp>{match self{Self::Private(f)=>stream::anonymous_stamp(f),Self::Named(p)=>{p.unchanged()?;Ok(p.stamp.clone())}}}
}
pub(super) struct Input{dir:HeldDir,pub o:Original,m:Map,source:Option<Composed>,published:Option<PinnedParent>,parent:std::sync::Arc<PinnedParent>,target:Target,already:Option<Material>,retained:Vec<Saved>}
pub(super) struct Verified{input:Input,pre:fs::OriginalStamp,c:Content,index:[u8;32],cleanup:Vec<Option<fs::OriginalStamp>>}
impl Input{
    pub(super) fn open(dir:HeldDir,o:Original,m:Map,source:Option<Composed>,published:Option<PinnedParent>,parent:std::sync::Arc<PinnedParent>,
        a:Option<&DeltaAllocation>,d:Option<&Delta>,retained:Vec<Saved>)->io::Result<Self>{
        dir.exclusive()?;
        let k=parent.key();
        if o.file!=k.file||o.history!=k.history||o.branch!=k.branch||o.creation!=k.creation||o.parent!=k.node||
            o.parent_version!=k.version||o.parent_decision!=k.decision||o.parent_length!=k.length||o.revision<=k.revision||
            source.is_none()==published.is_none()||source.as_ref().is_some_and(|working|!working.uses(&parent))||
            d.is_some_and(|v|v.original!=o)||published.as_ref().is_some_and(|body|!d.is_some_and(|v|body.key()==crate::ordinary::journal::cycle::Parent::incremental(v))){
            return Err(invalid("INCR lacks its exact held parent and original predecision or published source"));
        }
        let target=match dir.pin_stream(&o.leaf(),shape(&o)?.2)?{
            Some(p)=>{
                let a=a.ok_or_else(||invalid("unreserved destination INCR already exists"))?;
                fs::require_material_token(&p.file,&token(a,&NativeObject::stamp(&p.stamp))?)?;p.named(&dir,&o.leaf())?;
                if let Some(d)=d{if d.material.object!=NativeObject::stamp(&p.stamp){return Err(invalid("cannot substitute a decided INCR"));}}
                Target::Named(p)
            }
            None if d.is_some()=>return Err(invalid("cannot rebuild missing published INCR")),
            None=>Target::Private(dir.anonymous_record()?.0),
        };
        Ok(Self{dir,o,m,source,published,parent,target,already:d.map(|d|d.material.clone()),retained})
    }
    pub(super) fn verify(self)->io::Result<Verified>{
        let o=&self.o;let (raw,table,total)=shape(o)?;let f=self.target.file();
        let temp=matches!(&self.target,Target::Private(_));let mut wanted=None;
        // A later fixed-node read always authenticates its actual parent
        // index. Even an all-changed or all-zero map must pass that same gate
        // before taking a SYSTEM slot. No noncontributing parent RAW is read.
        self.parent.write_range(0,0,io::sink())?;
        if temp{stream::allocate_stream(f,total)?;}
        if let Some(source)=&self.source{
            let (mut logical,mut packed)=(crate::hash::Sha256::new(),crate::hash::Sha256::new());let (mut n,mut all)=(0u64,0u64);
            source.contributing(|changed,bytes|{
                let next=all.checked_add(bytes.len() as u64).filter(|x|*x<=o.logical).ok_or_else(||invalid("working input exceeds original final length"))?;
                if changed{
                    let stop=n.checked_add(bytes.len() as u64).filter(|v|*v<=o.changed).ok_or_else(||invalid("changed input exceeds original packed length"))?;
                    if temp{stream::write_at(f,bytes,raw+n)?;}packed.update(bytes)?;n=stop;
                }
                logical.update(bytes)?;all=next;Ok(())
            })?;
            if all!=o.logical||n!=o.changed{return Err(invalid("working original did not furnish all required final bytes"));}
            let (logical,packed)=(logical.finish(),packed.finish());
            if temp{
                let head=seal_links(f,raw,table,DraftClaim{bytes:n,sha256:packed})?;
                stream::write_at(f,&o.header(&self.m,&Content{logical,packed,head}),0)?;
            }
            wanted=Some((logical,packed));
        }
        f.sync_all()?;let pre=self.target.stamp()?;
        if pre.stat.size()!=total{return Err(invalid("wrong prepared INCR file size"));}
        let (c,index)=scan(f,o,&self.m,||Ok(()))?;
        if self.target.stamp()?!=pre||wanted.is_some_and(|x|x!=(c.logical,c.packed)){return Err(invalid("actual prepared INCR does not equal checked original input"));}
        if let Some(d)=&self.already{if d.content!=c||d.index!=index{return Err(invalid("published INCR can never be rebuilt from dirty material"));}}
        if let Some(p)=&self.published{p.write_to(io::sink())?;p.require(&self.dir)?;}
        self.parent.require(&self.dir)?;
        let mut cleanup=Vec::with_capacity(self.retained.len());
        for s in &self.retained{cleanup.push(cleanup_witness(&self.dir,s,self.published.is_some())?);}
        Ok(Verified{input:self,pre,c,index,cleanup})
    }
}
impl Verified{
    pub(super) fn original(&self)->&Original{&self.input.o}
    pub(super) fn require_source(&self,dir:&HeldDir,o:&Original)->io::Result<()>{
        if self.input.o!=*o||self.input.dir.entity!=dir.entity||fs::Birth::read(&self.input.dir.file)?!=fs::Birth::read(&dir.file)?{return Err(invalid("INCR source FILE changed"));}
        self.input.dir.require_named()?;self.input.parent.require(dir)?;if let Some(s)=&self.input.source{s.finish()?;}
        if let Some(p)=&self.input.published{p.require(dir)?;}Ok(())
    }
    pub(super) fn install(&self,dir:&HeldDir,a:&DeltaAllocation,after_link:impl FnOnce()->io::Result<()>)->io::Result<NamedCandidate>{
        self.require_source(dir,&a.original)?;
        if self.input.target.stamp()?!=self.pre{return Err(invalid("verified incremental target changed before preparation"));}
        let o=&a.original;let bytes=shape(o)?.2;
        match &self.input.target{
            Target::Private(f)=>{fs::set_preparation_token(f,&token(a,&NativeObject::stamp(&self.pre))?)?;dir.link_stream(&o.leaf(),f,&self.pre,bytes)?;after_link()?;}
            Target::Named(p)=>p.named(dir,&o.leaf())?,
        }
        let p=dir.pin_stream(&o.leaf(),bytes)?.ok_or_else(||invalid("prepared INCR not installed at its original location"))?;
        let obj=NativeObject::stamp(&p.stamp);fs::require_material_token(&p.file,&token(a,&obj)?)?;
        if obj!=NativeObject::stamp(&self.pre)||header(&p.file,o,&self.input.m)?.0!=self.c{return Err(invalid("wrong prepared INCR after installation"));}
        p.named(dir,&o.leaf())?;dir.file.sync_all()?;Ok(NamedCandidate(p))
    }
    /// A post-link stamp alone never certifies data from the unlinked phase.
    /// Read *this exact named* dense stream and actual index outside global
    /// locks. Bound carries its same checked native FD/stamp into the decision.
    pub(super) fn verify_named(&self,candidate:NamedCandidate)->io::Result<Bound>{
        let NamedCandidate(p)=candidate;let dir=&self.input.dir;let o=&self.input.o;
        p.named(dir,&o.leaf())?;p.file.sync_all()?;
        let (content,index)=scan(&p.file,o,&self.input.m,||p.unchanged())?;
        if content!=self.c||index!=self.index{return Err(invalid("newly named incremental bytes differ from verified inputs"));}
        p.named(dir,&o.leaf())?;Ok(Bound{pin:p,o:o.clone(),c:content,index})
    }
    pub(super) fn retire(mut self,dir:&HeldDir,v:&crate::pool::lease::FileVolume<'_>,bound:&Bound,d:&Delta,mut after:impl FnMut(crate::ordinary::Cut)->io::Result<()>)->io::Result<()>{
        self.require_source(dir,&d.original)?;drop(self.input.source.take());
        // Read the actual named destination, its local map and every required
        // authenticated original index again, this time on the published side. Do not
        // infer install or cleanup authority from an in-memory prepared hash.
        let reopened=crate::ordinary::formal::for_retirement(v,dir,d)?;
        reopened.write_range(0,0,io::sink())?;
        let(_,now)=v.pool.snapshot()?;let lease=now.placement(d.original.file)?.file_lease(dir,true)?;
        let checked=||->io::Result<()>{
            lease.named(dir,c"FILE")?;reopened.require(dir)?;
            if bound.feedback(dir)?!=d.material{return Err(invalid("published INCR no longer verifies; preserve old work"));}Ok(())
        };
        checked()?;
        for (s,w) in self.input.retained.iter().zip(&self.cleanup){
            checked()?;retire_verified(dir,s,w,||after(crate::ordinary::Cut::WorkUnlinked))?;after(crate::ordinary::Cut::WorkReleased)?;
        }
        checked()
    }
}
pub(super) struct NamedCandidate(PinnedRegular);
pub(super) struct Bound{pin:PinnedRegular,o:Original,c:Content,index:[u8;32]}
impl Bound{
    pub(super) fn feedback(&self,dir:&HeldDir)->io::Result<Material>{
        self.pin.named(dir,&self.o.leaf())?;dir.file.sync_all()?;Ok(Material{object:NativeObject::stamp(&self.pin.stamp),content:self.c.clone(),index:self.index})
    }
}
/// A known issued destination name is not itself a verified prepared version.
pub(in crate::ordinary) fn inspect(dir:&HeldDir,a:&DeltaAllocation,m:&Map,d:Option<&Delta>)->io::Result<()>{
    let o=&a.original;let Some(pin)=dir.pin_stream(&o.leaf(),shape(o)?.2)? else{
        return if d.is_some(){Err(invalid("sealed INCR is no longer present"))}else{Ok(())};
    };
    let obj=NativeObject::stamp(&pin.stamp);fs::require_material_token(&pin.file,&token(a,&obj)?)?;
    let (c,_)=header(&pin.file,o,m)?;if d.is_some_and(|d|d.original!=*o||d.material.object!=obj||d.material.content!=c){return Err(invalid("different material under original INCR reservation"));}
    pin.named(dir,&o.leaf())
}
