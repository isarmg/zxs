//! A FILE's formal baseline and only the working edits still following it,
//! derived from a confirmed SYSTEM prefix. Neither an original reservation,
//! a sealed intent nor a complete frame above the floor enters this view.
//! No new on-disk fields, native permissions, local files or mutable index.
use super::{Commit,Decision,Delta,Frame,Journal,Placement,Saved};
use crate::{BranchId,FileId,HistoryId,NodeId,OperationId,VersionId,VersionStorageId,VolumeId,fs::invalid,
    ordinary::interval::types::Predecessor};
use std::io;

/// Evidence of one exact formal parent. Equal revision/length/content do
/// not make an acknowledged working operation that same formal operation.
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Parent{
    pub file:FileId,pub history:HistoryId,pub branch:BranchId,pub node:NodeId,pub version:VersionId,
    pub storage:VersionStorageId,pub creation:[u8;32],pub operation:OperationId,pub decision:[u8;32],pub revision:u64,pub length:u64,
}
impl Parent{
    pub(in crate::ordinary) fn full(f:&Commit)->Self{let o=&f.original;Self{file:o.file,history:o.history,
        branch:o.branch,node:o.node,version:o.version,storage:o.storage,creation:o.creation,operation:o.op,decision:f.digest,revision:1,length:o.content.bytes}}
    pub(in crate::ordinary) fn incremental(d:&Delta)->Self{let o=&d.original;Self{file:o.file,history:o.history,
        branch:o.branch,node:o.node,version:o.version,storage:o.storage,creation:o.creation,operation:o.op,decision:d.digest,revision:o.revision,length:o.logical}}
    pub(in crate::ordinary) fn predecessor(self,last:Option<&Saved>)->Predecessor{match last{
        None=>Predecessor{op:self.operation,decision:self.decision,revision:self.revision,length:self.length},
        Some(s)=>Predecessor{op:s.original.op,decision:s.digest,revision:s.original.revision(),length:s.original.result},
    }}
    pub(in crate::ordinary) fn owns(self,s:&Saved)->bool{let o=&s.original;
        o.file==self.file&&o.history==self.history&&o.branch==self.branch&&o.node==self.node&&o.version==self.version&&
        o.commit==self.decision&&o.base_len==self.length&&o.birth==self.creation
    }
    fn accepts(self,s:&Saved,last:Option<&Saved>)->bool{self.owns(s)&&s.original.predecessor==self.predecessor(last)}
    pub(in crate::ordinary) fn precedes(self,d:&Delta)->bool{let o=&d.original;
        o.creation==self.creation&&o.file==self.file&&o.history==self.history&&o.branch==self.branch&&o.parent==self.node&&
        o.parent_version==self.version&&o.parent_decision==self.decision&&o.parent_length==self.length&&
        o.revision>self.revision&&o.node!=self.node&&o.version!=self.version&&o.storage!=self.storage
    }
    pub(super) fn child(self,d:&Delta,last:&Saved)->bool{let o=&d.original;
        self.precedes(d)&&self.owns(last)&&o.working==last.original.op&&o.working_decision==last.digest&&
        o.revision==last.original.revision()&&o.logical==last.original.result
    }
}
/// A volatile comparison for releasing a slow input or continuing a held
/// working stream. The job-specific physical lease and original decision
/// must also be checked at the actual side effect; this is not an I/O grant.
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Edge{
    pub parent:Parent,pub predecessor:Predecessor,pub volume:VolumeId,pub epoch:u64,pub site:[u8;32],
}
pub(in crate::ordinary) struct Cycle<'a>{
    birth:&'a Frame,full:&'a Commit,children:Vec<&'a Delta>,work:Vec<&'a Saved>,parent:Parent,site:Placement<'a>,
}
impl<'a> Cycle<'a>{
    pub(in crate::ordinary) fn graph(&self)->io::Result<super::Graph<'a>>{
        let g=super::Graph::at(self.birth,self.full,&self.children)?;
        if g.tip()!=self.parent{return Err(invalid("formal graph tip differs from this original FILE cycle"));}Ok(g)
    }
    pub(in crate::ordinary) fn parent(&self)->Parent{self.parent}
    pub(in crate::ordinary) fn placement(&self)->Placement<'a>{self.site}
    pub(in crate::ordinary) fn edge_before(&self,op:OperationId)->Edge{
        // A decided Span can still hold its original cleanup slot. Its own
        // exact replay checks the edge BEFORE itself, never itself as parent.
        let prior=match self.work.split_last(){
            Some((last,rest)) if last.original.op==op=>rest.last().copied(),_=>self.work.last().copied(),
        };Edge{parent:self.parent,predecessor:self.parent.predecessor(prior),
            volume:self.site.volume(),epoch:self.site.epoch(),site:self.site.key()}
    }
    pub(in crate::ordinary) fn require_captured(&self,op:OperationId,want:Edge)->io::Result<()>{
        if self.edge_before(op)!=want{return Err(invalid("original formal baseline, working edge or custody changed while receiving input"));}Ok(())
    }
    pub(in crate::ordinary) fn work(&self)->&[&'a Saved]{&self.work}
    pub(in crate::ordinary) fn clean_transportable(&self)->io::Result<(&'a Commit,Option<&'a Delta>)>{
        // Existing MOVE, DELETE and RESTORE must never send only a prefix of
        // a file's history, or silently leave an acknowledged current draft.
        if !self.work.is_empty()||self.children.len()>1{return Err(invalid("uncommitted working cycle or unsupported retained formal graph"));}
        Ok((self.full,self.children.first().copied()))
    }
    fn observe(&mut self,d:&'a Decision)->io::Result<()>{
        let id=self.birth.create.file;
        match d{
            Decision::Span(s) if s.original.file==id=>{
                if self.site.retained().is_some()||s.original.birth!=self.birth.digest||s.original.volume!=self.site.volume()||
                    !self.parent.accepts(s,self.work.last().copied()){return Err(invalid("working operation belongs to a different formal cycle or direct predecessor"));}
                self.work.push(s);
            }
            Decision::Delta(n) if n.original.file==id=>{
                let last=self.work.last().copied().ok_or_else(||invalid("formal child has no original working edge in this cycle"))?;
                if self.site.retained().is_some()||n.original.creation!=self.birth.digest||n.original.volume!=self.site.volume()||
                    !self.parent.child(n,last){return Err(invalid("formal child did not publish its exact parent and visible working edge"));}
                self.children.push(n);self.parent=Parent::incremental(n);self.work.clear();
            }
            Decision::Move(m) if m.original.file==id=>{
                if !self.work.is_empty(){return Err(invalid("confirmed custody interrupted an acknowledged current working cycle"));}
                self.site.require_next(&m.original)?;self.site=Placement::from_prefix(self.birth,Some(m))?;
            }
            Decision::Formal(f) if f.original.file==id=>return Err(invalid("a second first FULL is not a formal child")),
            _=>{},
        }Ok(())
    }
}
impl Journal{
    pub(in crate::ordinary) fn confirmed_file_cycle(&self,id:FileId)->io::Result<Cycle<'_>>{
        let(seq,head)=self.frontier();self.file_cycle_at(id,seq,head)
    }
    pub(in crate::ordinary) fn file_cycle_at(&self,id:FileId,seq:u64,head:[u8;32])->io::Result<Cycle<'_>>{
        if self.prefix_digest(seq)?!=head{return Err(invalid("different original confirmed cutoff"));}
        let c=self.file(id).filter(|c|c.seq<=seq).ok_or_else(||invalid("FILE did not exist at this confirmed cutoff"))?;
        let f=self.head(id).filter(|f|c.seq<f.seq&&f.seq<=seq&&f.original.file==id&&f.original.history==c.create.history&&f.original.creation==c.digest)
            .ok_or_else(||invalid("FILE had no confirmed first FULL yet"))?;
        let start=usize::try_from(f.seq).map_err(|_|invalid("invalid formal sequence"))?;
        let end=usize::try_from(seq).map_err(|_|invalid("invalid cycle cutoff"))?;
        let mut cycle=Cycle{birth:c,full:f,children:Vec::new(),work:Vec::new(),parent:Parent::full(f),site:Placement::from_prefix(c,None)?};
        // This is a walk over the already authenticated in-memory prefix,
        // not a lookup of newest bytes or a disk scan. Other FILEs are inert.
        for d in self.decisions.get(start..end).ok_or_else(||invalid("unavailable confirmed original FILE prefix"))?{cycle.observe(d)?;}
        Ok(cycle)
    }
}
