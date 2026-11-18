//! The original REGISTRATION inode has one ordered decision stream. All command kinds
//! share the same pending allocation, sealed-frame slot, bounds and lower
//! frontier. The two private sidecars are operands, not votes.
use super::{allocation::{self,Allocation},draft::{allocation::{self as save_allocation,SaveAllocation},types::{Original,SaveFrame}},
    formal::{allocation::{self as formal_allocation,FormalAllocation},types::{self as formal_types,Commit}},
    interval::{allocation::{self as span_allocation,SpanAllocation},types::{self as span_types,Saved}},
    wire::{self,Frame,In,NativeObject,MAX_FRAME}};
use super::custody::{Method,allocation::{self as move_allocation,MoveAllocation},transfer::{self as transfer,Origin as MoveOrigin,Moved}};
use super::delta::{allocation::{self as delta_allocation,DeltaAllocation},map::Map as DeltaMap,types::{self as delta_types,Delta}};
use crate::{BranchId,NodeId,VersionId,VersionStorageId,fs::{self,HeldDir,invalid},EntryId,HistoryId,PoolId,OperationId,FileId,VolumeId};
use std::{collections::{BTreeMap,BTreeSet},ffi::CStr,io};

pub(in crate::ordinary) mod credits;
use credits::{Credits,Effect};

pub(crate) const MAX_JOURNAL:usize=8*1024*1024;
pub(in crate::ordinary) const MAX_DECISIONS:usize=8192;
pub(super) const INTENT:&CStr=c"FILE-INTENT";
const INTENT_MAGIC:&[u8]=b"ZXS/ORIGINAL-FILE-INTENT\0";
const MAX_INTENT:usize=MAX_FRAME+256;

pub(super) enum Decision{Create(Frame),Save(SaveFrame),Formal(Commit),Span(Saved),Delta(Delta),Move(Moved)}
impl Decision{
    pub(super) fn read(pool:PoolId,bytes:&[u8])->io::Result<Self>{
        let e=wire::envelope(pool,bytes)?;
        match e.kind{
            1=>Ok(Self::Create(Frame::from_envelope(pool,bytes,e)?)),
            2=>Ok(Self::Save(SaveFrame::from_envelope(pool,bytes,e)?)),
            3=>Ok(Self::Formal(Commit::from_envelope(pool,bytes,e)?)),
            4=>Ok(Self::Span(Saved::from_envelope(pool,bytes,e)?)),
            5=>Ok(Self::Delta(Delta::from_envelope(pool,bytes,e)?)),
            transfer::KIND=>Ok(Self::Move(Moved::read(pool,bytes,e)?)),
            _=>Err(invalid("unknown original SYSTEM command")),
        }
    }
    pub(super) fn moved(&self)->io::Result<&Moved>{match self{Self::Move(m)=>Ok(m),_=>Err(invalid("not an original FILE handoff"))}}
    pub(super) fn creation(&self)->io::Result<&Frame>{match self{Self::Create(f)=>Ok(f),_=>Err(invalid("original command is not FILE creation"))}}
    pub(super) fn save(&self)->io::Result<&SaveFrame>{match self{Self::Save(f)=>Ok(f),_=>Err(invalid("original command is not a first SAVE"))}}
    pub(super) fn formal(&self)->io::Result<&Commit>{match self{Self::Formal(f)=>Ok(f),_=>Err(invalid("original command is not a formal FILE commit"))}}
    pub(super) fn delta(&self)->io::Result<&Delta>{match self{Self::Delta(d)=>Ok(d),_=>Err(invalid("not a formal incremental decision"))}}
    pub(super) fn span(&self)->io::Result<&Saved>{match self{Self::Span(s)=>Ok(s),_=>Err(invalid("not an original working interval save"))}}
    pub(super) fn seq(&self)->u64{match self{Self::Create(f)=>f.seq,Self::Save(f)=>f.seq,Self::Formal(f)=>f.seq,Self::Span(f)=>f.seq,Self::Delta(f)=>f.seq,Self::Move(m)=>m.seq}}
    fn previous(&self)->[u8;32]{match self{Self::Create(f)=>f.previous,Self::Save(f)=>f.previous,Self::Formal(f)=>f.previous,Self::Span(f)=>f.previous,Self::Delta(f)=>f.previous,Self::Move(m)=>m.previous}}
    fn digest(&self)->[u8;32]{match self{Self::Create(f)=>f.digest,Self::Save(f)=>f.digest,Self::Formal(f)=>f.digest,Self::Span(f)=>f.digest,Self::Delta(f)=>f.digest,Self::Move(m)=>m.digest}}
    pub(super) fn request(&self)->[u8;32]{match self{Self::Create(f)=>f.request,Self::Save(f)=>f.request,Self::Formal(f)=>f.request,Self::Span(f)=>f.request,Self::Delta(f)=>f.request,Self::Move(m)=>m.request}}
    pub(super) fn bytes(&self)->&[u8]{match self{Self::Create(f)=>&f.bytes,Self::Save(f)=>&f.bytes,Self::Formal(f)=>&f.bytes,Self::Span(f)=>&f.bytes,Self::Delta(f)=>&f.bytes,Self::Move(m)=>&m.bytes}}
    fn effect(&self)->Effect{
        match self{Self::Move(m)=>m.original.effect(),Self::Create(_)|Self::Save(_)|Self::Formal(_)|Self::Span(_)|Self::Delta(_)=>Effect::Ordinary}
    }
    pub(super) fn op(&self)->OperationId{match self{Self::Create(f)=>f.create.op,Self::Save(f)=>f.original.op,Self::Formal(f)=>f.original.op,Self::Span(f)=>f.original.op,Self::Delta(f)=>f.original.op,Self::Move(m)=>m.original.op}}
}

pub(super) struct Intent{pub frame:Decision,pub stamp:fs::OriginalStamp,pub bytes:Vec<u8>}
impl Intent{
    pub(super) fn seal(system:&HeldDir,frame:&[u8])->io::Result<()>{
        let (file,entity)=system.anonymous_record()?;
        let origin=NativeObject{entity,birth:fs::Birth::read(&file)?};
        let mut bytes=INTENT_MAGIC.to_vec();origin.put(&mut bytes);
        bytes.extend_from_slice(&(frame.len() as u32).to_be_bytes());bytes.extend_from_slice(frame);
        let digest=crate::hash::digest(&bytes)?;bytes.extend_from_slice(&digest);system.add_record(file,INTENT,&bytes)
    }
    fn read(system:&HeldDir,pool:PoolId)->io::Result<Option<Self>>{
        let Some((stamp,bytes))=system.record(INTENT,MAX_INTENT)? else{return Ok(None)};
        if bytes.len()<INTENT_MAGIC.len()+32{return Err(invalid("short original FILE intent"));}
        let (body,digest)=bytes.split_at(bytes.len()-32);
        if crate::hash::digest(body)?.as_slice()!=digest{return Err(invalid("damaged original FILE intent"));}
        let mut input=In(body.strip_prefix(INTENT_MAGIC).ok_or_else(||invalid("unknown SYSTEM intent"))?);
        if NativeObject::get(&mut input)?!=NativeObject::stamp(&stamp){return Err(invalid("original FILE intent was replaced"));}
        let len=input.u32()? as usize;let frame=Decision::read(pool,input.take(len)?)?;input.end()?;
        Ok(Some(Self{frame,stamp,bytes}))
    }
}

use super::placement::Placement;
type Allowed<'a>=crate::pool::JournalRegistration<'a>;
#[derive(Default)]
struct OriginalIndex{
    credits:Credits,
    ops:BTreeSet<OperationId>,files:BTreeMap<FileId,usize>,entries:BTreeSet<EntryId>,histories:BTreeSet<HistoryId>,
    names:BTreeSet<(VolumeId,String)>,tickets:BTreeSet<OperationId>,native:BTreeSet<fs::Birth>,saved:BTreeMap<FileId,usize>,
    formal:BTreeMap<FileId,usize>,nodes:BTreeMap<NodeId,usize>,branches:BTreeSet<BranchId>,
    versions:BTreeSet<VersionId>,storage:BTreeSet<VersionStorageId>,spans:BTreeMap<FileId,Vec<usize>>,deltas:BTreeMap<FileId,usize>,moves:BTreeMap<FileId,usize>,
}
impl OriginalIndex{
    fn birth<'a>(&self,id:FileId,frames:&'a[Decision])->io::Result<&'a Frame>{
        self.files.get(&id).and_then(|i|frames.get(*i)).ok_or_else(||invalid("no original FILE creation at this SAVE cutoff"))?.creation()
    }
    fn placement<'a>(&self,id:FileId,ds:&'a[Decision])->io::Result<Placement<'a>>{
        let c=self.birth(id,ds)?;
        let m=self.moves.get(&id).map(|&i|ds[i].moved()).transpose()?;
        Placement::from_prefix(c,m)
    }
    fn original_save<'a>(&self,s:&Original,frames:&'a[Decision],allow:&Allowed<'_>)->io::Result<&'a Frame>{
        let c=self.birth(s.file,frames)?;
        if !s.matches_create(c)||!allow.file(s.volume,s.uid,c.create.directory.entity,&c.create.directory.birth,false){
            return Err(invalid("SAVE does not select this FILE, actual original birth or registered owner"));
        }
        Ok(c)
    }
    fn original_formal<'a>(&self,o:&formal_types::Original,ds:&'a[Decision],allow:&Allowed<'_>)->io::Result<(&'a Frame,&'a SaveFrame)>{
        let c=self.birth(o.file,ds)?;
        let w=self.saved.get(&o.file).and_then(|i|ds.get(*i)).ok_or_else(||invalid("formal commit has no actual confirmed working revision"))?.save()?;
        if !o.matches(c,w)||!allow.file(o.volume,o.uid,c.create.directory.entity,&c.create.directory.birth,false){
            return Err(invalid("explicit first commit does not bind this registered FILE and its exact saved revision"));
        }
        Ok((c,w))
    }
    fn new_ids(&self,o:&formal_types::Original)->bool{
        !self.formal.contains_key(&o.file)&&!self.nodes.contains_key(&o.node)&&!self.branches.contains(&o.branch)
            &&!self.versions.contains(&o.version)&&!self.storage.contains(&o.storage)
    }
    fn original_span<'a>(&self,o:&span_types::Original,ds:&'a[Decision],allow:&Allowed<'_>)->io::Result<Placement<'a>>{
        let place=self.placement(o.file,ds)?;
        let f=self.formal.get(&o.file).and_then(|i|ds.get(*i)).ok_or_else(||invalid("interval save has no original formal FILE baseline"))?.formal()?;
        let prior=self.spans.get(&o.file).and_then(|ids|ids.last()).map(|&i|ds[i].span()).transpose()?;
        if !o.matches(place,f,prior)||!allow.file(o.volume,o.uid,place.directory().entity,&place.directory().birth,false){
            return Err(invalid("interval save is not bound to this registered FILE and original formal node"));
        }
        Ok(place)
    }
    fn delta_source<'a>(&self,o:&delta_types::Original,ds:&'a[Decision],allow:&Allowed<'_>)->io::Result<Placement<'a>>{
        let place=self.placement(o.file,ds)?;
        let p=self.formal.get(&o.file).and_then(|i|ds.get(*i)).ok_or_else(||invalid("no formal parent for INCR"))?.formal()?;
        let indices=self.spans.get(&o.file).ok_or_else(||invalid("no original working chain for INCR"))?;
        let selected:Vec<&Saved>=indices.iter().map(|&i|ds[i].span()).collect::<io::Result<_>>()?;
        let last=selected.last().copied().ok_or_else(||invalid("no original saved working edge for INCR"))?;
        let map=DeltaMap::derive(p.original.content.bytes,selected.iter().copied())?;
        o.require(place,p,last,&map)?;
        if self.deltas.contains_key(&o.file)||self.nodes.contains_key(&o.node)||self.versions.contains(&o.version)||self.storage.contains(&o.storage)
            ||!allow.file(o.volume,o.uid,place.directory().entity,&place.directory().birth,false){return Err(invalid("conflicting formal INCR identity or owner"));}
        Ok(place)
    }
    fn registered_source<'a>(&self,o:&MoveOrigin,ds:&'a[Decision],allow:&Allowed<'_>)->io::Result<(&'a Frame,&'a Commit,Option<&'a Delta>)>{
        // Every input here belongs to the same already-verified prefix.
        let before=self.placement(o.file,ds)?;let c=before.birth();
        let f=self.formal.get(&o.file).and_then(|i|ds.get(*i)).ok_or_else(||invalid("handoff requires original first FULL"))?.formal()?;
        let d=self.deltas.get(&o.file).map(|&i|ds[i].delta()).transpose()?;
        if d.is_none()&&self.spans.get(&o.file).is_some_and(|v|!v.is_empty()){return Err(invalid("cannot hand off a dirty working chain"));}
        if self.tickets.contains(&o.ticket)||self.ops.contains(&o.ticket)||self.tickets.contains(&o.op)||self.ops.contains(&o.op){
            return Err(invalid("an original operation or custody ticket has already been used"));}
        registered_handoff(o,allow,before,f,d,self.names.contains(&(o.from,o.name.clone())),self.names.contains(&(o.to,o.name.clone())))?;
        Ok((c,f,d))
    }
    fn admit(&self,f:&Decision,frames:&[Decision],used:usize,prev:[u8;32],allow:&Allowed<'_>)->io::Result<()>{
        self.credits.room(frames.len(),used,f.bytes().len(),f.effect())?;
        if f.seq()!=frames.len() as u64+1||f.previous()!=prev||self.ops.contains(&f.op()){
            return Err(invalid("repeated operation or nonconsecutive original FILE decision"));
        }
        match f{
            Decision::Move(m)=>{let(_,f,d)=self.registered_source(&m.original,frames,allow)?;
                m.require_at(self.placement(m.original.file,frames)?,f,d)?;
                if m.original.mode==Method::Transfer&&m.target.each().any(|x|self.native.contains(&x.birth)){
                    return Err(invalid("transferred material cannot reuse any admitted native owner"));
                }
            }
            Decision::Create(f)=>{let c=&f.create;
                if !allow.file(c.volume,c.uid,c.directory.entity,&c.directory.birth,false)
                    ||self.files.contains_key(&c.file)||self.names.contains(&(c.volume,c.name.clone()))||self.entries.contains(&c.entry)
                    ||self.histories.contains(&c.history)||self.tickets.contains(&c.attempt)||self.native.contains(&c.local.birth)
                    ||self.native.contains(&c.directory.birth){return Err(invalid("conflicting original FILE creation"));}
            }
            Decision::Save(f)=>{let c=self.original_save(&f.original,frames,allow)?;let n=&f.material.object;
                if self.saved.contains_key(&f.original.file)||self.native.contains(&n.birth)||n.entity.device!=c.create.directory.entity.device
                    ||n.birth.filesystem()!=c.create.directory.birth.filesystem()||n.entity==c.create.local.entity||n.entity==c.create.directory.entity{
                    return Err(invalid("first SAVE is not a new working revision on the original FILE owner"));
                }
            }
            Decision::Formal(f)=>{
                let (c,_)=self.original_formal(&f.original,frames,allow)?;
                let n=&f.materials.node;let r=&f.materials.raw;
                if !self.new_ids(&f.original)||n.birth==r.birth||n.entity==r.entity||
                    [n,r].iter().any(|v|self.native.contains(&v.birth)||v.entity.device!=c.create.directory.entity.device||
                        v.birth.filesystem()!=c.create.directory.birth.filesystem()){
                    return Err(invalid("formal FULL and node must be separate new originals on the one admitted FILE owner"));
                }
            }
            Decision::Delta(d)=>{
                let owner=self.delta_source(&d.original,frames,allow)?;let n=&d.material.object;
                if self.native.contains(&n.birth)||n.entity.device!=owner.directory().entity.device||n.birth.filesystem()!=owner.directory().birth.filesystem(){
                    return Err(invalid("formal INCR is not new immutable material on its original FILE volume"));}
            }
            Decision::Span(s)=>{let o=&s.original;let owner=self.original_span(o,frames,allow)?;let n=&s.material.object;
                if self.deltas.contains_key(&o.file)||self.native.contains(&n.birth)||n.entity.device!=owner.directory().entity.device||
                    n.birth.filesystem()!=owner.directory().birth.filesystem(){return Err(invalid("not an independent changed extent on the original FILE"));}
            }
        }
        Ok(())
    }
    fn remember(&mut self,d:&Decision,index:usize)->io::Result<()>{
        self.credits.apply(d.effect(),d.digest())?;
        self.ops.insert(d.op());
        match d{
            Decision::Move(m)=>{let o=&m.original;self.moves.insert(o.file,index);self.tickets.insert(o.ticket);
                let(src_active,dst_active)=active_names(o);
                if src_active{self.names.remove(&(o.from,o.name.clone()));}
                if dst_active{self.names.insert((o.to,o.name.clone()));}
                if o.mode==Method::Transfer{for obj in m.target.each(){self.native.insert(obj.birth.clone());}}}
            Decision::Create(f)=>{let c=&f.create;self.files.insert(c.file,index);self.names.insert((c.volume,c.name.clone()));
                self.entries.insert(c.entry);self.histories.insert(c.history);self.tickets.insert(c.attempt);
                self.native.insert(c.directory.birth.clone());self.native.insert(c.local.birth.clone());}
            Decision::Save(f)=>{self.saved.insert(f.original.file,index);self.native.insert(f.material.object.birth.clone());}
            Decision::Formal(f)=>{let o=&f.original;self.formal.insert(o.file,index);self.nodes.insert(o.node,index);
                self.branches.insert(o.branch);self.versions.insert(o.version);self.storage.insert(o.storage);
                self.native.insert(f.materials.node.birth.clone());self.native.insert(f.materials.raw.birth.clone());}
            Decision::Delta(d)=>{let o=&d.original;self.deltas.insert(o.file,index);self.nodes.insert(o.node,index);
                self.versions.insert(o.version);self.storage.insert(o.storage);self.native.insert(d.material.object.birth.clone());}
            Decision::Span(s)=>{self.spans.entry(s.original.file).or_default().push(index);self.native.insert(s.material.object.birth.clone());}
        }
        Ok(())
    }
}

fn active_names(o:&MoveOrigin)->(bool,bool){
    use super::custody::purpose::Purpose;
    match o.purpose{Purpose::Ordinary=>(true,true),Purpose::Delete{..}=>(true,false),Purpose::Restore{..}=>(false,true)}
}
/// A logical precondition, not a warrant to install/delete material. The two
/// endpoints are admitted as a pair by the *same* immutable original record.
fn registered_handoff(o:&MoveOrigin,reg:&Allowed<'_>,before:Placement<'_>,f:&Commit,d:Option<&Delta>,at_source:bool,at_target:bool)->io::Result<()>{
    use crate::pool::lease::recycle::Direction;use super::custody::purpose::Purpose;
    let src=(o.from,o.src_root.entity,&o.src_root.birth);let dst=(o.to,o.dst_root.entity,&o.dst_root.birth);
    let eligible=match o.purpose{
        Purpose::Ordinary=>reg.file(src.0,o.uid,src.1,src.2,true)&&reg.file(dst.0,o.uid,dst.1,dst.2,true),
        Purpose::Delete{..}=>reg.dedicated(o.uid,Direction::ToRecycle,src,dst),
        Purpose::Restore{..}=>reg.dedicated(o.uid,Direction::FromRecycle,src,dst),
    };
    let(src_active,_)=active_names(o);
    if !eligible||at_source!=src_active||at_target{return Err(invalid("wrong registered role, source name or occupied current destination"));}
    o.purpose.check_transfer(o.mode,&o.src_root,&o.dst_root)?;before.require_next(o)?;o.require_history(before.birth(),f,d)?;
    before.sites(f,d)?.check(&o.src_root)
}

pub(crate) struct Journal{
    credits:Credits,
    decisions:Vec<Decision>,originals:BTreeMap<FileId,usize>,working:BTreeMap<FileId,usize>,formal:BTreeMap<FileId,usize>,nodes:BTreeMap<NodeId,usize>,spans:BTreeMap<FileId,Vec<usize>>,deltas:BTreeMap<FileId,usize>,moves:BTreeMap<FileId,usize>,pub(super) moving:Option<MoveAllocation>,pub(super) pending:Option<Intent>,pub(super) allocated:Option<Allocation>,
    pub(super) saving:Option<SaveAllocation>,pub(super) committing:Option<FormalAllocation>,pub(super) editing:Option<SpanAllocation>,pub(super) incrementing:Option<DeltaAllocation>,pub(super) tail:usize,genesis:[u8;32],frame_bytes:usize,
}
impl Journal{
    pub(crate) fn read(system:&HeldDir,allowed:crate::pool::JournalRegistration<'_>,tail:&[u8],floor:fs::Frontier)->io::Result<Self>{
        let(pool,initial_op,genesis)=allowed.basis();
        if tail.len()>MAX_JOURNAL{return Err(invalid("SYSTEM FILE journal exceeds budget"));}
        let mut input=tail;let mut ds=Vec::new();let mut prev=genesis;let mut idx=OriginalIndex::default();idx.ops.insert(initial_op);
        while !input.is_empty(){
            let Some(n)=Frame::size(input)? else{break};if input.len()<n{break;}
            let f=Decision::read(pool,&input[..n])?;idx.admit(&f,&ds,tail.len()-input.len(),prev,&allowed)?;
            prev=f.digest();idx.remember(&f,ds.len())?;ds.push(f);input=&input[n..];
        }
        let (mut allocated,mut saving,mut committing,mut editing,mut incrementing,mut moving)=(None,None,None,None,None,None);
        if let Some((stamp,bytes))=system.record(allocation::NAME,move_allocation::MAX)?{
            if bytes.starts_with(move_allocation::MAGIC){
                let a=MoveAllocation::from_bytes(system,pool,stamp,bytes)?;
                let last=ds.last().is_some_and(|d|matches!(d,Decision::Move(m) if a.matches(m)));
                if !last{
                    if ds.len()>=MAX_DECISIONS||a.seq!=ds.len() as u64+1||a.previous!=prev||idx.ops.contains(&a.original.op)||a.opened(){
                        return Err(invalid("handoff reservation not at its original frontier"));
                    }
                    // Reservation alone is never a decided custody. Frame,
                    // original intent and frontier remain independent checks.
                    idx.registered_source(&a.original,&ds,&allowed)?;
                    if a.stage().is_some_and(|s|idx.native.contains(&s.directory.birth)){return Err(invalid("handoff stage already belongs to admitted native material"));}
                }
                moving=Some(a);
            }else if bytes.len()>allocation::MAX{return Err(invalid("original non-MOVE FILE allocation exceeds its own bound"));}
            else if bytes.starts_with(delta_allocation::MAGIC){
                let a=DeltaAllocation::from_bytes(pool,stamp,bytes)?;
                let last=ds.last().is_some_and(|d|matches!(d,Decision::Delta(f) if a.matches(f)));
                if !last{
                    if ds.len()>=MAX_DECISIONS||a.seq!=ds.len() as u64+1||a.previous!=prev||idx.ops.contains(&a.original.op){return Err(invalid("not the original INCR frontier"));}
                    idx.delta_source(&a.original,&ds,&allowed)?;
                }
                incrementing=Some(a);
            }else if bytes.starts_with(span_allocation::MAGIC){
                let a=SpanAllocation::from_bytes(pool,stamp,bytes)?;
                let last=ds.last().is_some_and(|d|matches!(d,Decision::Span(f) if a.matches(f)));
                if !last&&(ds.len()>=MAX_DECISIONS||a.seq!=ds.len() as u64+1||a.previous!=prev||idx.ops.contains(&a.original.op)){
                    return Err(invalid("working interval reservation conflicts with original frontier"));
                }
                // When an already-appended frame exactly equals this still-open
                // original, its predecessor was checked before remember(). A fresh
                // reservation must instead use the currently admitted FILE edge.
                if !last{if idx.deltas.contains_key(&a.original.file){return Err(invalid("working reservation did not select the current formal HEAD"));}idx.original_span(&a.original,&ds,&allowed)?;}
                editing=Some(a);
            }else if bytes.starts_with(formal_allocation::MAGIC){
                let a=FormalAllocation::from_bytes(pool,stamp,bytes)?;idx.original_formal(&a.original,&ds,&allowed)?;
                let last=ds.last().is_some_and(|d|matches!(d,Decision::Formal(f) if a.matches(f)));
                if !last&&(ds.len()>=MAX_DECISIONS||a.seq!=ds.len() as u64+1||a.previous!=prev||idx.ops.contains(&a.original.op)||!idx.new_ids(&a.original)){
                    return Err(invalid("formal reservation is not at its original source and frontier"));
                }
                committing=Some(a);
            }else if bytes.starts_with(save_allocation::MAGIC){
                let a=SaveAllocation::from_bytes(pool,stamp,bytes)?;idx.original_save(&a.original,&ds,&allowed)?;
                let last=ds.last().is_some_and(|d|matches!(d,Decision::Save(f) if a.matches(f)));
                if !last&&(ds.len()>=MAX_DECISIONS||a.seq!=ds.len() as u64+1||a.previous!=prev||idx.ops.contains(&a.original.op)||idx.saved.contains_key(&a.original.file)){
                    return Err(invalid("first SAVE reservation is not at its original frontier"));
                }
                saving=Some(a);
            }else{
                let a=Allocation::from_bytes(system,pool,stamp,bytes)?;
                let last=ds.last().is_some_and(|d|matches!(d,Decision::Create(f) if a.matches(f)));
                if !allowed.file(a.volume,a.uid,a.root.entity,&a.root.birth,true)||
                    (!last&&(ds.len()>=MAX_DECISIONS||a.seq!=ds.len() as u64+1||a.previous!=prev||idx.ops.contains(&a.op)
                    ||idx.files.contains_key(&a.file)||idx.entries.contains(&a.entry)||idx.histories.contains(&a.history)
                    ||idx.names.contains(&(a.volume,a.name.clone()))||idx.tickets.contains(&a.attempt)
                    ||a.stage.as_ref().is_some_and(|s|idx.native.contains(&s.object.birth)))){
                    return Err(invalid("original FILE allocation no longer matches its registered root or frontier"));
                }
                allocated=Some(a);
            }
        }
        // A newly reserved, not-yet-appended slot must still fit including
        // all original return credits. If it is the exact complete last frame,
        // admission already checked its predecessor; never charge it twice.
        let slot=moving.as_ref().map(|a|(a.seq,a.original.effect())).or_else(||
            [allocated.as_ref().map(|a|a.seq),saving.as_ref().map(|a|a.seq),committing.as_ref().map(|a|a.seq),
             editing.as_ref().map(|a|a.seq),incrementing.as_ref().map(|a|a.seq)].into_iter().flatten().next().map(|seq|(seq,Effect::Ordinary)));
        if let Some((seq,e))=slot{if seq==ds.len() as u64+1{idx.credits.room(ds.len(),tail.len()-input.len(),MAX_FRAME,e)?;}}
        let pending=Intent::read(system,pool)?;
        if let Some(p)=&pending{
            let matches=match &p.frame{
                Decision::Move(m)=>moving.as_ref().is_some_and(|a|a.matches(m)),
                Decision::Create(f)=>allocated.as_ref().is_some_and(|a|a.matches(f)),
                Decision::Save(f)=>saving.as_ref().is_some_and(|a|a.matches(f)),
                Decision::Formal(f)=>committing.as_ref().is_some_and(|a|a.matches(f)),
                Decision::Delta(f)=>incrementing.as_ref().is_some_and(|a|a.matches(f)),
                Decision::Span(f)=>editing.as_ref().is_some_and(|a|a.matches(f)),
            };
            if !matches{return Err(invalid("sealed FILE material has no matching original reservation"));}
            if ds.last().is_some_and(|d|d.bytes()==p.frame.bytes()){
                if !input.is_empty(){return Err(invalid("unattributed tail after open decided intent"));}
            }else{
                idx.admit(&p.frame,&ds,tail.len()-input.len(),prev,&allowed)?;
                if !p.frame.bytes().starts_with(input)||(tail.len()-input.len()).checked_add(p.frame.bytes().len()).filter(|n|*n<=MAX_JOURNAL).is_none(){
                    return Err(invalid("SYSTEM suffix is not the originally reserved FILE decision"));
                }
            }
        }else if !input.is_empty(){return Err(invalid("unattributed or incomplete SYSTEM tail; refusing to alter it"));}
        let mut frame_bytes=tail.len()-input.len();let mut unconfirmed=input.len();
        if floor!=fs::frontier(ds.len() as u64,frame_bytes,prev){
            let last=ds.last().ok_or_else(||invalid("acknowledged SYSTEM frontier exceeds original journal"))?;
            let before=frame_bytes-last.bytes().len();
            if !input.is_empty()||!pending.as_ref().is_some_and(|p|p.frame.bytes()==last.bytes())||
                floor!=fs::frontier(ds.len() as u64-1,before,last.previous()){
                return Err(invalid("SYSTEM frames do not satisfy the acknowledged frontier"));
            }
            frame_bytes=before;unconfirmed=last.bytes().len();
            idx.credits.before_unconfirmed(last.effect(),last.digest())?;
            // The replay indices must never expose an otherwise complete but
            // not yet confirmed last frame (including a first working revision).
            match last {Decision::Move(m)=>{
                    if moving.as_ref().is_some_and(|a|a.opened()){return Err(invalid("target installation confirmation before original placement frontier"));}
                    // Only this unconfirmed frame is hidden. If there was
                    // earlier custody, restore the actual admitted predecessor,
                    // not the creation site or an untrusted location claim.
                    let id=m.original.file;
                    match ds[..ds.len()-1].iter().rposition(|d|matches!(d,Decision::Move(old) if old.original.file==id)){
                        Some(i)=>{idx.moves.insert(id,i);},None=>{idx.moves.remove(&id);}
                    }
                },Decision::Delta(d)=>{idx.deltas.remove(&d.original.file);idx.nodes.remove(&d.original.node);},Decision::Span(s)=>{
                    let chain=idx.spans.get_mut(&s.original.file).ok_or_else(||invalid("missing original span chain"))?;
                    if chain.pop()!=Some(ds.len()-1){return Err(invalid("wrong unconfirmed working edge"));}
                },Decision::Formal(f)=>{idx.formal.remove(&f.original.file);idx.nodes.remove(&f.original.node);},
                Decision::Save(f)=>{idx.saved.remove(&f.original.file);},Decision::Create(f)=>{idx.files.remove(&f.create.file);}}
            ds.pop();
        }
        Ok(Self{credits:idx.credits,decisions:ds,originals:idx.files,working:idx.saved,formal:idx.formal,nodes:idx.nodes,spans:idx.spans,deltas:idx.deltas,moves:idx.moves,moving,pending,allocated,saving,committing,editing,incrementing,tail:unconfirmed,genesis,frame_bytes})
    }
    pub(super) fn frontier(&self)->(u64,[u8;32]){
        (self.decisions.len() as u64,self.decisions.last().map_or(self.genesis,Decision::digest))
    }
    pub(super) fn prefix_digest(&self,seq:u64)->io::Result<[u8;32]>{
        if seq==0{return Ok(self.genesis);}
        usize::try_from(seq-1).ok().and_then(|i|self.decisions.get(i)).filter(|d|d.seq()==seq)
            .map(Decision::digest).ok_or_else(||invalid("no such confirmed original SYSTEM prefix"))
    }
    pub(super) fn acknowledgement(&self,f:&Decision)->io::Result<(fs::Frontier,fs::Frontier)>{
        let (seq,prev)=self.frontier();
        if self.tail!=f.bytes().len()||!self.pending.as_ref().is_some_and(|p|p.frame.bytes()==f.bytes())||f.seq()!=seq+1||f.previous()!=prev{
            return Err(invalid("no complete original frame for this acknowledgement"));
        }
        self.room_for_effect(f.bytes().len(),f.effect())?;
        Ok((fs::frontier(seq,self.frame_bytes,prev),fs::frontier(f.seq(),self.frame_bytes+f.bytes().len(),f.digest())))
    }
    /// Validate the whole prospective frame before creating its one
    /// original intent. Includes the real registered roles, physical
    /// incarnations at this prefix and remaining return credits.
    pub(in crate::ordinary) fn admit_dedicated_frame(&self,allowed:&Allowed<'_>,m:&Moved)->io::Result<()>{
        let(_,init,genesis)=allowed.basis();
        if self.genesis!=genesis||m.original.purpose==super::custody::purpose::Purpose::Ordinary||
            !self.moving.as_ref().is_some_and(|a|a.matches(m))||self.decided(m.original.op).is_some()||
            self.pending.as_ref().is_some_and(|i|i.frame.bytes()!=m.bytes){
            return Err(invalid("different sealed frame, original register or lifecycle operand"));}
        let mut i=OriginalIndex::default();i.ops.insert(init);
        for(n,d)in self.decisions.iter().enumerate(){i.remember(d,n)?;}
        i.admit(&Decision::Move(m.clone()),&self.decisions,self.frame_bytes,self.frontier().1,allowed)
    }
    pub(super) fn room_for(&self,n:usize)->io::Result<()>{
        self.room_for_effect(n,Effect::Ordinary)
    }
    pub(in crate::ordinary) fn room_for_effect(&self,n:usize,e:Effect)->io::Result<()>{
        self.credits.room(self.decisions.len(),self.frame_bytes,n,e)
    }
    pub(super) fn decided(&self,op:OperationId)->Option<&Decision>{self.decisions.iter().find(|d|d.op()==op)}
    pub(super) fn operation(&self,op:OperationId)->Option<&Frame>{self.decided(op)?.creation().ok()}
    pub(super) fn file(&self,id:FileId)->Option<&Frame>{self.decisions.get(*self.originals.get(&id)?)?.creation().ok()}
    pub(super) fn named(&self,v:VolumeId,name:&str)->Option<&Frame>{self.on(v).find(|f|f.create.name==name)}
    pub(super) fn on_all(&self)->impl Iterator<Item=&Frame>{self.decisions.iter().filter_map(|d|d.creation().ok())}
    pub(in crate::ordinary) fn confirmed_handoff(&self,op:OperationId)->io::Result<(&Moved,&MoveAllocation)>{
        let m=self.decided(op).ok_or_else(||invalid("no confirmed handoff at the original SYSTEM lower bound"))?.moved()?;
        let a=self.moving.as_ref().ok_or_else(||invalid("no open handoff completion"))?;
        if !a.matches(m)||self.placed(m.original.file).map(|d|d.digest)!=Some(m.digest){
            return Err(invalid("different original placement/operation at this completion cut"));
        }Ok((m,a))
    }
    pub(super) fn placed(&self,id:FileId)->Option<&Moved>{self.decisions.get(*self.moves.get(&id)?)?.moved().ok()}
    pub(super) fn placement(&self,id:FileId)->io::Result<Placement<'_>>{
        let c=self.file(id).ok_or_else(||invalid("no confirmed original FILE placement"))?;
        Placement::from_prefix(c,self.placed(id))
    }
    /// Only a fully confirmed original at its own position. An allocation,
    /// stage or even a sealed exact next frame is not a decided handoff.
    pub(in crate::ordinary) fn before_handoff(&self,m:&Moved)->io::Result<Placement<'_>>{
        let i=usize::try_from(m.seq).ok().and_then(|v|v.checked_sub(1)).ok_or_else(||invalid("invalid custody sequence"))?;
        let before=self.decisions.get(..i).ok_or_else(||invalid("custody outside original prefix"))?;
        if before.last().map_or(self.genesis,Decision::digest)!=m.previous{return Err(invalid("wrong original custody prefix digest"));}
        if !matches!(self.decisions.get(i),Some(Decision::Move(f)) if f==m){
            return Err(invalid("no exact confirmed original frame at this custody cutoff"));}
        let c=self.file(m.original.file).filter(|c|c.seq<m.seq).ok_or_else(||invalid("missing original FILE before custody"))?;
        let old=before.iter().rev().find_map(|d|match d{Decision::Move(x) if x.original.file==m.original.file=>Some(x),_=>None});
        Placement::from_prefix(c,old)
    }
    pub(super) fn volume_of(&self,f:&Frame)->VolumeId{self.placed(f.create.file).map_or(f.create.volume,|m|m.original.to)}
    pub(super) fn on(&self,v:VolumeId)->impl Iterator<Item=&Frame>{self.on_all().filter(move |f|self.volume_of(f)==v&&
        !self.placed(f.create.file).is_some_and(|m|matches!(m.original.purpose,super::custody::purpose::Purpose::Delete{..})))}
    pub(super) fn moves(&self)->impl Iterator<Item=&Moved>{self.decisions.iter().filter_map(|d|d.moved().ok())}
    /// Predecision check for an exact, unsigned-by-itself lifecycle proposal.
    /// Not the cold admission of a completed DELETE/RESTORE. Never accept a
    /// separately chosen pool/genesis, a stale settled op or a fresh second
    /// reservation. Only a qualified writer of the original pool calls this.
    pub(in crate::ordinary) fn check_new_lifecycle_origin(&self,reg:&Allowed<'_>,o:&MoveOrigin)->io::Result<()>{
        let(_,init,genesis)=reg.basis();
        if self.genesis!=genesis||o.purpose==super::custody::purpose::Purpose::Ordinary||o.op==init||o.ticket==init||
            self.moving.is_some()||self.allocated.is_some()||self.saving.is_some()||self.committing.is_some()||
            self.editing.is_some()||self.incrementing.is_some()||self.pending.is_some()||self.tail!=0||
            self.ticket_used(o.op)||self.ticket_used(o.ticket){return Err(invalid("not an unused dedicated proposal at its original SYSTEM prefix"));}
        self.room_for_effect(MAX_FRAME,o.effect())?;
        let prev=self.placement(o.file)?;let f=self.head(o.file).ok_or_else(||invalid("lifecycle proposal needs original FULL"))?;
        let d=self.delta(o.file);if d.is_none()&&self.interval(o.file).is_some(){return Err(invalid("uncommitted source revision"));}
        registered_handoff(o,reg,prev,f,d,self.named(o.from,&o.name).is_some(),self.named(o.to,&o.name).is_some())
    }
    /// Preflight is not an I/O permit. This is repeated from the very same
    /// writer snapshot immediately before naming the original reservation.
    pub(in crate::ordinary) fn admit_new_handoff(&self,p:&crate::pool::lease::LocalPool,op:OperationId,id:FileId,from:VolumeId,to:VolumeId)->io::Result<()>{
        p.require_business_op(op)?;self.room_for(MAX_FRAME)?;
        let c=self.file(id).ok_or_else(||invalid("no original FILE for handoff"))?;
        if self.pending.is_some()||self.allocated.is_some()||self.saving.is_some()||self.committing.is_some()||self.editing.is_some()||
            self.incrementing.is_some()||self.moving.is_some()||self.ticket_used(op)||from==to||c.create.volume!=from||
            !self.clean_initial_handoff(id)||self.named(to,&c.create.name).is_some(){
            return Err(invalid("handoff needs a free original slot and operation, clean initial main FULL with optional original INCR and unoccupied registered target"));
        }Ok(())
    }
    pub(in crate::ordinary) fn clean_initial_handoff(&self,id:FileId)->bool{
        self.placed(id).is_none()&&self.head(id).is_some()&&(self.delta(id).is_some()||self.interval(id).is_none())
    }
    pub(super) fn ticket_used(&self,ticket:OperationId)->bool{
        self.on_all().any(|c|c.create.attempt==ticket)||self.moves().any(|m|m.original.ticket==ticket)||self.decided(ticket).is_some()
    }
    pub(super) fn intervals(&self,id:FileId)->impl DoubleEndedIterator<Item=&Saved>{
        self.spans.get(&id).into_iter().flatten().map(move |&i|match &self.decisions[i]{
            Decision::Span(s)=>s,_=>unreachable!("only admitted working decisions enter this per-FILE index"),
        })
    }
    pub(super) fn interval(&self,id:FileId)->Option<&Saved>{self.intervals(id).next_back()}
    pub(super) fn head(&self,id:FileId)->Option<&Commit>{self.decisions.get(*self.formal.get(&id)?)?.formal().ok()}
    pub(super) fn delta(&self,file:FileId)->Option<&Delta>{self.decisions.get(*self.deltas.get(&file)?)?.delta().ok()}
    pub(super) fn delta_node(&self,file:FileId,node:NodeId)->Option<&Delta>{
        self.decisions.get(*self.nodes.get(&node)?)?.delta().ok().filter(|d|d.original.file==file)
    }
    pub(super) fn free_content_ids(&self,node:NodeId,version:VersionId,storage:VersionStorageId)->bool{
        !self.decisions.iter().any(|d|match d{
            Decision::Formal(f)=>{let o=&f.original;o.node==node||o.version==version||o.storage==storage},
            Decision::Delta(f)=>{let o=&f.original;o.node==node||o.version==version||o.storage==storage},
            _=>false,
        })
    }
    pub(super) fn node(&self,file:FileId,node:NodeId)->Option<&Commit>{
        self.decisions.get(*self.nodes.get(&node)?)?.formal().ok().filter(|f|f.original.file==file)
    }
    pub(super) fn require_free_formal_ids(&self,o:&formal_types::Original)->io::Result<()>{
        if !self.free_content_ids(o.node,o.version,o.storage)||self.decisions.iter().filter_map(|d|d.formal().ok()).any(|d|{
            let p=&d.original;p.file==o.file||p.node==o.node||p.version==o.version||p.branch==o.branch||p.storage==o.storage
        }){return Err(invalid("first formal identity already used"));}Ok(())
    }
    pub(super) fn saved(&self,id:FileId)->Option<&SaveFrame>{
        self.decisions.get(*self.working.get(&id)?)?.save().ok()
    }
}
