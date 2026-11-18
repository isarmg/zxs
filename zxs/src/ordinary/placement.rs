//! A FILE birth and its *current native owner at one original prefix*.
//! The constructor is fed only by the authenticated journal index. The MOVE
//! never rewrites an original FILE decision; a later SAVE never amends MOVE.
use super::{custody::{purpose::Purpose,transfer::{ChildRoles,Moved,Origin,Sites}},delta::{types::Delta,physical::Body},
    formal::{types::Commit,physical::HistoryBody},wire::{Frame,NativeObject},material};
use crate::{VolumeId,fs::{HeldDir,invalid,stream::PinnedRegular}};
use std::io;

#[derive(Clone,Copy)]
pub(super) struct Placement<'a>{birth:&'a Frame,applied:Option<&'a Moved>}
impl<'a> Placement<'a>{
    pub(super) fn from_prefix(birth:&'a Frame,applied:Option<&'a Moved>)->io::Result<Self>{
        if let Some(m)=applied{let c=&birth.create;let o=&m.original;
            if m.seq<=birth.seq||o.creation!=birth.digest||o.file!=c.file||o.history!=c.history||o.entry!=c.entry||
                o.uid!=c.uid||o.name!=c.name||o.from==o.to||o.epoch==u64::MAX||
                (o.purpose==Purpose::Ordinary&&(o.from!=c.volume||o.epoch!=0)){
                return Err(invalid("native placement is not this original FILE's admitted handoff"));
            }
        }
        Ok(Self{birth,applied})
    }
    pub(super) fn birth(self)->&'a Frame{self.birth}
    pub(super) fn transfer(self)->Option<&'a Moved>{self.applied}
    pub(super) fn volume(self)->VolumeId{self.applied.map_or(self.birth.create.volume,|m|m.original.to)}
    pub(super) fn epoch(self)->u64{self.applied.map_or(0,|m|m.original.epoch+1)}
    pub(super) fn key(self)->[u8;32]{self.applied.map_or(self.birth.digest,|m|m.digest)}
    pub(super) fn retained(self)->Option<super::journal::credits::Deletion>{
        self.applied.filter(|m|matches!(m.original.purpose,Purpose::Delete{..})).map(|m|
            super::journal::credits::Deletion{file:self.birth.create.file,operation:m.original.op,decision:m.digest})
    }
    /// One previously admitted FILE cutoff, never today's location inferred
    /// from a path. The original register independently authenticates both
    /// endpoint roles. This relation grants no I/O and performs no cleanup.
    pub(super) fn require_next(self,o:&Origin)->io::Result<()>{
        let c=&self.birth.create;
        if o.file!=c.file||o.entry!=c.entry||o.history!=c.history||o.creation!=self.birth.digest||
            o.uid!=c.uid||o.name!=c.name||o.from!=self.volume()||o.epoch!=self.epoch()||o.from==o.to||
            self.applied.is_some_and(|m|m.original.dst_root!=o.src_root){
            return Err(invalid("custody handoff does not start at this FILE's exact prior placement"));
        }
        let allowed=match o.purpose{
            Purpose::Ordinary=>self.applied.is_none(),
            Purpose::Delete{previous,..}=>self.retained().is_none()&&previous==self.key(),
            Purpose::Restore{deleted_by,previous}=>self.retained().is_some_and(|k|k.operation==deleted_by&&k.decision==previous),
        };
        if !allowed{return Err(invalid("wrong previous active site or original retained deletion"));}Ok(())
    }
    /// Every leaf at this exact *source*. If an earlier transfer included
    /// a child, it is its destination leaf; a child authored later must be
    /// the FILE's actual newly published INCR on this active volume instead.
    pub(super) fn sites(self,f:&Commit,d:Option<&Delta>)->io::Result<Sites>{
        let c=self.birth;
        if c.seq>=f.seq||f.original.file!=c.create.file||f.original.creation!=c.digest||
            d.is_some_and(|n|n.seq<=f.seq||n.original.file!=c.create.file||n.original.creation!=c.digest||n.original.parent_decision!=f.digest){
            return Err(invalid("source role set does not belong to the FILE's original FULL and child"));
        }
        let Some(m)=self.applied else{
            if f.original.volume!=self.volume()||d.is_some_and(|x|x.original.volume!=self.volume()){
                return Err(invalid("incorrect source volume for original formal material"));}
            return Ok(Sites::birthplace(c,f,d))
        };
        if m.original.parent!=f.digest||f.seq>=m.seq{return Err(invalid("wrong FULL at prior custody boundary"));}
        if m.original.children.second().is_some()||m.target.children.second().is_some(){return Err(invalid("single-child placement cannot attest a larger original handoff"));}
        let mut sites=m.target.clone();
        match(m.original.children.first().copied(),m.target.children.first(),d){
            (Some(hash),Some(_),Some(n)) if n.digest==hash&&n.seq<m.seq=>{},
            (None,None,None)=>{},
            (None,None,Some(n)) if n.seq>m.seq&&self.retained().is_none()&&n.original.volume==self.volume()=>{
                sites.children=ChildRoles::One(n.material.object.clone());},
            _=>return Err(invalid("missing, extra or unrelated INCR at original custody frontier"))
        }sites.check(&m.original.dst_root)?;Ok(sites)
    }
    pub(super) fn directory(self)->&'a NativeObject{self.applied.map_or(&self.birth.create.directory,|m|&m.target.directory)}
    fn local(self)->&'a NativeObject{self.applied.map_or(&self.birth.create.local,|m|&m.target.local)}
    pub(super) fn file_lease(self,dir:&HeldDir,exclusive:bool)->io::Result<PinnedRegular>{
        if self.retained().is_some(){return Err(invalid("retained FILE is not an ordinary serving volume"));}
        material::verify_header_at(dir,&self.birth.create,self.directory(),self.local())?;
        material::custody_lease_at(dir,&self.birth.create,self.local(),exclusive)
    }
    pub(super) fn full(self,dir:&HeldDir,f:&Commit)->io::Result<HistoryBody>{
        if self.retained().is_some()||f.original.file!=self.birth.create.file||f.original.creation!=self.birth.digest||
            self.applied.is_some_and(|m|m.original.parent!=f.digest||f.seq>=m.seq){
            return Err(invalid("not the original formal FULL of the selected FILE at its placement cutoff"));
        }
        match self.applied{
            None=>HistoryBody::current(dir,f),
            Some(m)=>HistoryBody::at_originals(dir,f,&m.target.node,&m.target.raw)
        }
    }
    pub(super) fn incremental(self,dir:&HeldDir,parent:HistoryBody,d:&Delta)->io::Result<Body>{
        if self.retained().is_some()||d.original.file!=self.birth.create.file||d.original.creation!=self.birth.digest{
            return Err(invalid("incremental node is not this FILE's original child"));
        }
        match self.applied{
            None if d.original.volume==self.volume()=>Body::open(dir,parent,d),
            Some(m) if m.original.children.first()==Some(&d.digest)&&d.seq<m.seq&&
                (m.original.purpose!=Purpose::Ordinary||d.original.volume==m.original.from)=>{
                let n=m.target.children.first().ok_or_else(||invalid("missing frozen transferred incremental object"))?;
                Body::at_original(dir,parent,d,n)
            },
            Some(m) if m.original.children.first().is_none()&&m.target.children.first().is_none()&&d.seq>m.seq&&d.original.volume==self.volume()=>
                Body::open(dir,parent,d),
            _=>Err(invalid("incremental node is neither the frozen original nor a later decision at this owner")),
        }
    }
}
