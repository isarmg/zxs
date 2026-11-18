//! A FILE birth and its *current native owner at one original prefix*.
//! The constructor is fed only by the authenticated journal index. The MOVE
//! never rewrites an original FILE decision; a later SAVE never amends MOVE.
use super::{custody::transfer::Moved,delta::{types::Delta,physical::Body},
    formal::{types::Commit,physical::HistoryBody},wire::{Frame,NativeObject},material};
use crate::{VolumeId,fs::{HeldDir,invalid,stream::PinnedRegular}};
use std::io;

#[derive(Clone,Copy)]
pub(super) struct Placement<'a>{birth:&'a Frame,applied:Option<&'a Moved>}
impl<'a> Placement<'a>{
    pub(super) fn from_prefix(birth:&'a Frame,applied:Option<&'a Moved>)->io::Result<Self>{
        if let Some(m)=applied{let c=&birth.create;let o=&m.original;
            if m.seq<=birth.seq||o.creation!=birth.digest||o.file!=c.file||o.history!=c.history||o.entry!=c.entry||
                o.uid!=c.uid||o.name!=c.name||o.from!=c.volume||o.from==o.to||o.epoch!=0{
                return Err(invalid("native placement is not this original FILE's admitted handoff"));
            }
        }
        Ok(Self{birth,applied})
    }
    pub(super) fn birth(self)->&'a Frame{self.birth}
    pub(super) fn volume(self)->VolumeId{self.applied.map_or(self.birth.create.volume,|m|m.original.to)}
    pub(super) fn directory(self)->&'a NativeObject{self.applied.map_or(&self.birth.create.directory,|m|&m.target.directory)}
    fn local(self)->&'a NativeObject{self.applied.map_or(&self.birth.create.local,|m|&m.target.local)}
    pub(super) fn file_lease(self,dir:&HeldDir,exclusive:bool)->io::Result<PinnedRegular>{
        material::verify_header_at(dir,&self.birth.create,self.directory(),self.local())?;
        material::custody_lease_at(dir,&self.birth.create,self.local(),exclusive)
    }
    pub(super) fn full(self,dir:&HeldDir,f:&Commit)->io::Result<HistoryBody>{
        if f.original.file!=self.birth.create.file||f.original.creation!=self.birth.digest||
            self.applied.is_some_and(|m|m.original.parent!=f.digest||f.seq>=m.seq){
            return Err(invalid("not the original formal FULL of the selected FILE at its placement cutoff"));
        }
        match self.applied{
            None=>HistoryBody::current(dir,f),
            Some(m)=>HistoryBody::at_originals(dir,f,&m.target.node,&m.target.raw)
        }
    }
    pub(super) fn incremental(self,dir:&HeldDir,parent:HistoryBody,d:&Delta)->io::Result<Body>{
        if d.original.file!=self.birth.create.file||d.original.creation!=self.birth.digest{
            return Err(invalid("incremental node is not this FILE's original child"));
        }
        match self.applied{
            None if d.original.volume==self.volume()=>Body::open(dir,parent,d),
            Some(m) if m.original.last==Some(d.digest)&&d.seq<m.seq&&d.original.volume==m.original.from=>{
                let n=m.target.incr.as_ref().ok_or_else(||invalid("missing frozen transferred incremental object"))?;
                Body::at_original(dir,parent,d,n)
            },
            Some(m) if m.original.last.is_none()&&m.target.incr.is_none()&&d.seq>m.seq&&d.original.volume==self.volume()=>
                Body::open(dir,parent,d),
            _=>Err(invalid("incremental node is neither the frozen original nor a later decision at this owner")),
        }
    }
}
