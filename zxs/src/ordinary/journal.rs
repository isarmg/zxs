//! The original REGISTRATION inode has one ordered decision stream. All command kinds
//! share the same pending allocation, sealed-frame slot, bounds and lower
//! frontier. The two private sidecars are operands, not votes.
use super::{allocation::{self,Allocation},draft::{allocation::{self as save_allocation,SaveAllocation},types::{Original,SaveFrame}},
    wire::{self,Frame,In,NativeObject,MAX_FRAME}};
use crate::{fs::{self,HeldDir,invalid},Entity,EntryId,HistoryId,PoolId,OperationId,FileId,VolumeId};
use std::{collections::{BTreeMap,BTreeSet},ffi::CStr,io};

pub(crate) const MAX_JOURNAL:usize=8*1024*1024;
const MAX_DECISIONS:usize=8192;
pub(super) const INTENT:&CStr=c"FILE-INTENT";
const INTENT_MAGIC:&[u8]=b"ZXS/ORIGINAL-FILE-INTENT\0";
const MAX_INTENT:usize=MAX_FRAME+256;

pub(super) enum Decision{Create(Frame),Save(SaveFrame)}
impl Decision{
    pub(super) fn read(pool:PoolId,bytes:&[u8])->io::Result<Self>{
        let e=wire::envelope(pool,bytes)?;
        match e.kind{
            1=>Ok(Self::Create(Frame::from_envelope(pool,bytes,e)?)),
            2=>Ok(Self::Save(SaveFrame::from_envelope(pool,bytes,e)?)),
            _=>Err(invalid("unknown original SYSTEM command")),
        }
    }
    pub(super) fn creation(&self)->io::Result<&Frame>{match self{Self::Create(f)=>Ok(f),_=>Err(invalid("original command is not FILE creation"))}}
    pub(super) fn save(&self)->io::Result<&SaveFrame>{match self{Self::Save(f)=>Ok(f),_=>Err(invalid("original command is not a first SAVE"))}}
    pub(super) fn seq(&self)->u64{match self{Self::Create(f)=>f.seq,Self::Save(f)=>f.seq}}
    fn previous(&self)->[u8;32]{match self{Self::Create(f)=>f.previous,Self::Save(f)=>f.previous}}
    fn digest(&self)->[u8;32]{match self{Self::Create(f)=>f.digest,Self::Save(f)=>f.digest}}
    pub(super) fn request(&self)->[u8;32]{match self{Self::Create(f)=>f.request,Self::Save(f)=>f.request}}
    pub(super) fn bytes(&self)->&[u8]{match self{Self::Create(f)=>&f.bytes,Self::Save(f)=>&f.bytes}}
    pub(super) fn op(&self)->OperationId{match self{Self::Create(f)=>f.create.op,Self::Save(f)=>f.original.op}}
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

type Allowed<'a>=dyn Fn(VolumeId,u32,Entity,&fs::Birth,bool)->bool+'a;
#[derive(Default)]
struct OriginalIndex{
    ops:BTreeSet<OperationId>,files:BTreeMap<FileId,usize>,entries:BTreeSet<EntryId>,histories:BTreeSet<HistoryId>,
    names:BTreeSet<(VolumeId,String)>,tickets:BTreeSet<OperationId>,native:BTreeSet<fs::Birth>,saved:BTreeMap<FileId,usize>,
}
impl OriginalIndex{
    fn birth<'a>(&self,id:FileId,frames:&'a[Decision])->io::Result<&'a Frame>{
        self.files.get(&id).and_then(|i|frames.get(*i)).ok_or_else(||invalid("no original FILE creation at this SAVE cutoff"))?.creation()
    }
    fn original_save<'a>(&self,s:&Original,frames:&'a[Decision],allow:&Allowed<'_>)->io::Result<&'a Frame>{
        let c=self.birth(s.file,frames)?;
        if !s.matches_create(c)||!allow(s.volume,s.uid,c.create.directory.entity,&c.create.directory.birth,false){
            return Err(invalid("SAVE does not select this FILE, actual original birth or registered owner"));
        }
        Ok(c)
    }
    fn admit(&self,f:&Decision,frames:&[Decision],prev:[u8;32],allow:&Allowed<'_>)->io::Result<()>{
        if frames.len()>=MAX_DECISIONS||f.seq()!=frames.len() as u64+1||f.previous()!=prev||self.ops.contains(&f.op()){
            return Err(invalid("repeated operation or nonconsecutive original FILE decision"));
        }
        match f{
            Decision::Create(f)=>{let c=&f.create;
                if !allow(c.volume,c.uid,c.directory.entity,&c.directory.birth,false)
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
        }
        Ok(())
    }
    fn remember(&mut self,d:&Decision,index:usize){
        self.ops.insert(d.op());
        match d{
            Decision::Create(f)=>{let c=&f.create;self.files.insert(c.file,index);self.names.insert((c.volume,c.name.clone()));
                self.entries.insert(c.entry);self.histories.insert(c.history);self.tickets.insert(c.attempt);
                self.native.insert(c.directory.birth.clone());self.native.insert(c.local.birth.clone());}
            Decision::Save(f)=>{self.saved.insert(f.original.file,index);self.native.insert(f.material.object.birth.clone());}
        }
    }
}

pub(crate) struct Journal{
    decisions:Vec<Decision>,originals:BTreeMap<FileId,usize>,working:BTreeMap<FileId,usize>,pub(super) pending:Option<Intent>,pub(super) allocated:Option<Allocation>,
    pub(super) saving:Option<SaveAllocation>,pub(super) tail:usize,genesis:[u8;32],frame_bytes:usize,
}
impl Journal{
    pub(crate) fn read(system:&HeldDir,pool:PoolId,initial_op:OperationId,genesis:[u8;32],tail:&[u8],floor:fs::Frontier,
        allowed:impl Fn(VolumeId,u32,Entity,&fs::Birth,bool)->bool)->io::Result<Self>{
        if tail.len()>MAX_JOURNAL{return Err(invalid("SYSTEM FILE journal exceeds budget"));}
        let mut input=tail;let mut ds=Vec::new();let mut prev=genesis;let mut idx=OriginalIndex::default();idx.ops.insert(initial_op);
        while !input.is_empty(){
            let Some(n)=Frame::size(input)? else{break};if input.len()<n{break;}
            let f=Decision::read(pool,&input[..n])?;idx.admit(&f,&ds,prev,&allowed)?;
            prev=f.digest();idx.remember(&f,ds.len());ds.push(f);input=&input[n..];
        }
        let (mut allocated,mut saving)=(None,None);
        if let Some((stamp,bytes))=system.record(allocation::NAME,allocation::MAX)?{
            if bytes.starts_with(save_allocation::MAGIC){
                let a=SaveAllocation::from_bytes(pool,stamp,bytes)?;idx.original_save(&a.original,&ds,&allowed)?;
                let last=ds.last().is_some_and(|d|matches!(d,Decision::Save(f) if a.matches(f)));
                if !last&&(ds.len()>=MAX_DECISIONS||a.seq!=ds.len() as u64+1||a.previous!=prev||idx.ops.contains(&a.original.op)||idx.saved.contains_key(&a.original.file)){
                    return Err(invalid("first SAVE reservation is not at its original frontier"));
                }
                saving=Some(a);
            }else{
                let a=Allocation::from_bytes(system,pool,stamp,bytes)?;
                let last=ds.last().is_some_and(|d|matches!(d,Decision::Create(f) if a.matches(f)));
                if !allowed(a.volume,a.uid,a.root.entity,&a.root.birth,true)||
                    (!last&&(ds.len()>=MAX_DECISIONS||a.seq!=ds.len() as u64+1||a.previous!=prev||idx.ops.contains(&a.op)
                    ||idx.files.contains_key(&a.file)||idx.entries.contains(&a.entry)||idx.histories.contains(&a.history)
                    ||idx.names.contains(&(a.volume,a.name.clone()))||idx.tickets.contains(&a.attempt)
                    ||a.stage.as_ref().is_some_and(|s|idx.native.contains(&s.object.birth)))){
                    return Err(invalid("original FILE allocation no longer matches its registered root or frontier"));
                }
                allocated=Some(a);
            }
        }
        let pending=Intent::read(system,pool)?;
        if let Some(p)=&pending{
            let matches=match &p.frame{
                Decision::Create(f)=>allocated.as_ref().is_some_and(|a|a.matches(f)),
                Decision::Save(f)=>saving.as_ref().is_some_and(|a|a.matches(f)),
            };
            if !matches{return Err(invalid("sealed FILE material has no matching original reservation"));}
            if ds.last().is_some_and(|d|d.bytes()==p.frame.bytes()){
                if !input.is_empty(){return Err(invalid("unattributed tail after open decided intent"));}
            }else{
                idx.admit(&p.frame,&ds,prev,&allowed)?;
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
            // The replay indices must never expose an otherwise complete but
            // not yet confirmed last frame (including a first working revision).
            match last {Decision::Save(f)=>{idx.saved.remove(&f.original.file);},Decision::Create(f)=>{idx.files.remove(&f.create.file);}}
            ds.pop();
        }
        Ok(Self{decisions:ds,originals:idx.files,working:idx.saved,pending,allocated,saving,tail:unconfirmed,genesis,frame_bytes})
    }
    pub(super) fn frontier(&self)->(u64,[u8;32]){
        (self.decisions.len() as u64,self.decisions.last().map_or(self.genesis,Decision::digest))
    }
    pub(super) fn acknowledgement(&self,f:&Decision)->io::Result<(fs::Frontier,fs::Frontier)>{
        let (seq,prev)=self.frontier();
        if self.tail!=f.bytes().len()||!self.pending.as_ref().is_some_and(|p|p.frame.bytes()==f.bytes())||f.seq()!=seq+1||f.previous()!=prev{
            return Err(invalid("no complete original frame for this acknowledgement"));
        }
        self.room_for(f.bytes().len())?;
        Ok((fs::frontier(seq,self.frame_bytes,prev),fs::frontier(f.seq(),self.frame_bytes+f.bytes().len(),f.digest())))
    }
    pub(super) fn room_for(&self,n:usize)->io::Result<()>{
        if self.decisions.len()>=MAX_DECISIONS||self.frame_bytes.checked_add(n).filter(|v|*v<=MAX_JOURNAL).is_none(){return Err(invalid("SYSTEM decision budget reached"));}
        Ok(())
    }
    pub(super) fn decided(&self,op:OperationId)->Option<&Decision>{self.decisions.iter().find(|d|d.op()==op)}
    pub(super) fn operation(&self,op:OperationId)->Option<&Frame>{self.decided(op)?.creation().ok()}
    pub(super) fn file(&self,id:FileId)->Option<&Frame>{self.decisions.get(*self.originals.get(&id)?)?.creation().ok()}
    pub(super) fn named(&self,v:VolumeId,name:&str)->Option<&Frame>{self.on(v).find(|f|f.create.name==name)}
    pub(super) fn on_all(&self)->impl Iterator<Item=&Frame>{self.decisions.iter().filter_map(|d|d.creation().ok())}
    pub(super) fn on(&self,v:VolumeId)->impl Iterator<Item=&Frame>{self.on_all().filter(move |f|f.create.volume==v)}
    pub(super) fn saved(&self,id:FileId)->Option<&SaveFrame>{
        self.decisions.get(*self.working.get(&id)?)?.save().ok()
    }
}
