//! The one existing SYSTEM register inode holds the original genesis followed
//! by checksummed decisions. FILE-INTENT is an immutable recovery operand: it
//! cannot publish anything. No tail is repaired from a name, hash or new request.
use super::{allocation::Allocation,wire::{Frame,In,NativeObject,MAX_FRAME}};
use crate::{fs::{self,HeldDir,invalid},PoolId,OperationId,FileId,VolumeId};
use std::{collections::BTreeSet,ffi::CStr,io};

pub(crate) const MAX_JOURNAL:usize=8*1024*1024;
const MAX_DECISIONS:usize=8192;
pub(super) const INTENT:&CStr=c"FILE-INTENT";
const INTENT_MAGIC:&[u8]=b"ZXS/ORIGINAL-FILE-INTENT\0";
const MAX_INTENT:usize=MAX_FRAME+256;

pub(super) struct Intent {
    pub frame:Frame,pub stamp:fs::OriginalStamp,pub bytes:Vec<u8>,
}
impl Intent{
    pub(super) fn seal(system:&HeldDir,frame:&Frame)->io::Result<()>{
        let (file,entity)=system.anonymous_record()?;
        let origin=NativeObject{entity,birth:fs::Birth::read(&file)?};
        let mut bytes=INTENT_MAGIC.to_vec();origin.put(&mut bytes);
        bytes.extend_from_slice(&(frame.bytes.len() as u32).to_be_bytes());bytes.extend_from_slice(&frame.bytes);
        let digest=crate::hash::digest(&bytes)?;bytes.extend_from_slice(&digest);
        system.add_record(file,INTENT,&bytes)
    }
    fn read(system:&HeldDir,pool:PoolId)->io::Result<Option<Self>>{
        let Some((stamp,bytes))=system.record(INTENT,MAX_INTENT)? else{return Ok(None)};
        if bytes.len()<INTENT_MAGIC.len()+32{return Err(invalid("short original FILE intent"));}
        let (body,digest)=bytes.split_at(bytes.len()-32);
        if crate::hash::digest(body)?.as_slice()!=digest{return Err(invalid("damaged original FILE intent"));}
        let mut input=In(body.strip_prefix(INTENT_MAGIC).ok_or_else(||invalid("unknown SYSTEM intent"))?);
        let origin=NativeObject::get(&mut input)?;
        if origin!=NativeObject::stamp(&stamp){return Err(invalid("original FILE intent was replaced"));}
        let len=input.u32()? as usize;let frame=Frame::read(pool,input.take(len)?)?;input.end()?;
        Ok(Some(Self{frame,stamp,bytes}))
    }
}

pub(crate) struct Journal{
    frames:Vec<Frame>,
    pub(super) pending:Option<Intent>,
    pub(super) allocated:Option<Allocation>,
    pub(super) tail:usize,
    pub(super) genesis:[u8;32],
    frame_bytes:usize,
}
impl Journal {
    pub(crate) fn read(system:&HeldDir,pool:PoolId,initial_op:OperationId,genesis:[u8;32],tail:&[u8],floor:fs::Frontier,
        allowed:impl Fn(VolumeId,u32,crate::Entity,&fs::Birth,bool)->bool)->io::Result<Self>{
        if tail.len()>MAX_JOURNAL{return Err(invalid("SYSTEM FILE journal exceeds budget"));}
        let mut input=tail;let mut frames=Vec::new();let mut prev=genesis;
        let mut ops=BTreeSet::new();let mut files=BTreeSet::new();let mut entries=BTreeSet::new();
        let mut histories=BTreeSet::new();let mut names=BTreeSet::new();
        ops.insert(initial_op); // An OpId identifies one command across the entire pool.
        let mut attempts=BTreeSet::new();let mut originals=BTreeSet::new();
        while !input.is_empty() {
            let Some(n)=Frame::size(input)? else{break};
            if input.len()<n{break;}
            let frame=Frame::read(pool,&input[..n])?;let c=&frame.create;
            if frames.len()>=MAX_DECISIONS||frame.seq!=frames.len() as u64+1 ||frame.previous!=prev||!allowed(c.volume,c.uid,c.directory.entity,&c.directory.birth,false)
                ||!attempts.insert(c.attempt)||!originals.insert(c.directory.birth.clone())||!originals.insert(c.local.birth.clone())
                ||!ops.insert(c.op)||!files.insert(c.file)||!entries.insert(c.entry)||!histories.insert(c.history)
                ||!names.insert((c.volume,c.name.clone())){
                return Err(invalid("nonconsecutive or conflicting original FILE decision"));
            }
            prev=frame.digest;frames.push(frame);input=&input[n..];
        }
        let allocated=Allocation::read(system,pool)?;
        if let Some(a)=&allocated{
            if !allowed(a.volume,a.uid,a.root.entity,&a.root.birth,true){return Err(invalid("FILE allocation has no original ordinary owner"));}
            let already=frames.last().is_some_and(|last|a.matches(last));
            if !already && (a.seq!=frames.len() as u64+1||a.previous!=prev||frames.len()>=MAX_DECISIONS||
                attempts.contains(&a.attempt)||a.stage.as_ref().is_some_and(|s|originals.contains(&s.object.birth))||ops.contains(&a.op)||files.contains(&a.file)||entries.contains(&a.entry)||histories.contains(&a.history)
                ||names.contains(&(a.volume,a.name.clone()))){
                return Err(invalid("FILE allocation is not at its original frontier"));
            }
        }
        let pending=Intent::read(system,pool)?;
        if let Some(original)=&pending{
            let f=&original.frame;let c=&f.create;
            if !allocated.as_ref().is_some_and(|a|a.matches(f)){return Err(invalid("sealed FILE material has no matching original allocation"));}
            if !allowed(c.volume,c.uid,c.directory.entity,&c.directory.birth,false){return Err(invalid("FILE intent is outside its original registration"));}
            let already=frames.last().is_some_and(|last|last.bytes==f.bytes);
            if already {
                if !input.is_empty(){return Err(invalid("unexpected tail after decided open intent"));}
            }else if f.seq!=frames.len() as u64+1||f.previous!=prev||frames.len()>=MAX_DECISIONS
                ||originals.contains(&c.local.birth)||ops.contains(&c.op)||files.contains(&c.file)||entries.contains(&c.entry)||histories.contains(&c.history)
                ||names.contains(&(c.volume,c.name.clone()))||!f.bytes.starts_with(input)
                ||(tail.len()-input.len()).checked_add(f.bytes.len()).filter(|n|*n<=MAX_JOURNAL).is_none(){
                return Err(invalid("journal suffix is not the original reserved FILE decision"));
            }
        }else if !input.is_empty(){
            return Err(invalid("unattributed or incomplete SYSTEM tail; refusing to alter it"));
        }
        let mut frame_bytes=tail.len()-input.len();
        let mut unconfirmed=input.len();
        if floor!=fs::frontier(frames.len() as u64,frame_bytes,prev){
            // A single full original frame may precede its acknowledgement.
            // Its independent sealed operand must still exist. A shorter actual
            // log, missing operand or different branch at the same height is NOT
            // an invitation to synthesize or discard a decision.
            let last=frames.last().ok_or_else(||invalid("acknowledged SYSTEM frontier exceeds the complete original journal"))?;
            let before=frame_bytes-last.bytes.len();
            if !input.is_empty() || !pending.as_ref().is_some_and(|p|p.frame.bytes==last.bytes)
                || floor!=fs::frontier(frames.len() as u64-1,before,last.previous){
                return Err(invalid("original SYSTEM frames do not satisfy the acknowledged frontier"));
            }
            frame_bytes=before;unconfirmed=last.bytes.len();
            frames.pop(); // never present this lagging frame as an acknowledged operation
        }
        Ok(Self{frames,pending,allocated,tail:unconfirmed,genesis,frame_bytes})
    }
    pub(super) fn frontier(&self)->(u64,[u8;32]){
        (self.frames.len() as u64,self.frames.last().map_or(self.genesis,|f|f.digest))
    }
    pub(super) fn acknowledgement(&self,original:&Frame)->io::Result<(fs::Frontier,fs::Frontier)>{
        let (seq,digest)=self.frontier();
        if self.tail!=original.bytes.len() || !self.pending.as_ref().is_some_and(|p|p.frame.bytes==original.bytes)
            || original.seq!=seq+1 || original.previous!=digest{
            return Err(invalid("no complete original frame for this acknowledgement"));
        }
        self.room_for(original.bytes.len())?;
        Ok((fs::frontier(seq,self.frame_bytes,digest),
            fs::frontier(original.seq,self.frame_bytes+original.bytes.len(),original.digest)))
    }
    pub(super) fn room_for(&self,n:usize)->io::Result<()>{
        if self.frames.len()>=MAX_DECISIONS||self.frame_bytes.checked_add(n).filter(|v|*v<=MAX_JOURNAL).is_none(){
            return Err(invalid("SYSTEM decision budget reached"));
        }
        Ok(())
    }
    pub(super) fn operation(&self,op:OperationId)->Option<&Frame>{self.frames.iter().find(|f|f.create.op==op)}
    pub(super) fn file(&self,id:FileId)->Option<&Frame>{self.frames.iter().find(|f|f.create.file==id)}
    pub(super) fn named(&self,v:VolumeId,name:&str)->Option<&Frame>{self.frames.iter().find(|f|f.create.volume==v&&f.create.name==name)}
    pub(super) fn on_all(&self)->impl Iterator<Item=&Frame>{self.frames.iter()}
    pub(super) fn on(&self,v:VolumeId)->impl Iterator<Item=&Frame>{self.frames.iter().filter(move|f|f.create.volume==v)}
}
