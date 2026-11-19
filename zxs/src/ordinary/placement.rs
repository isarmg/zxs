//! A FILE birth and its *current native owner at one original prefix*.
//! The constructor is fed only by the authenticated journal index. The MOVE
//! never rewrites an original FILE decision; a later SAVE never amends MOVE.
use super::{custody::{purpose::Purpose,transfer::{ChildRoles,Children,Moved,Origin,Sites}},
    formal::{types::Commit,physical::HistoryBody},journal::Graph,wire::{Frame,NativeObject},material};
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
    /// A retained FILE has no ordinary name, and an old birth is not an
    /// address for a later MOVE/restore. Both facts come from real decisions.
    pub(in crate::ordinary) fn ordinary_parent(self)->io::Result<Option<super::DirectoryParent>>{
        match self.applied{None=>Ok(Some(self.birth.create.parent.address(self.birth.create.volume))),
            Some(m)=>Ok(m.original.ends()?.1.ordinary())}
    }
    pub(in crate::ordinary) fn retained_from(self)->io::Result<Option<super::DirectoryParent>>{
        match self.applied{Some(m) if matches!(m.original.purpose,Purpose::Delete{..})=>Ok(m.original.ends()?.0.ordinary()),
            _=>Ok(None)}
    }
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
            o.ends()?.0.ordinary()!=self.ordinary_parent()?||
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
    pub(super) fn sites(self,f:&'a Commit,d:Children<'a>)->io::Result<Sites>{
        self.graph_sites(Graph::from_roles(self.birth,f,d)?)
    }
    /// Return *all* retained native objects of this prefix, not just the
    /// latest edited version. The caller cannot hide a role transported by
    /// a preceding decision or give a later native child that old ticket.
    pub(in crate::ordinary) fn graph_sites(self,g:Graph<'a>)->io::Result<Sites>{
        let c=self.birth;let f=g.full();if c.seq>=f.seq||f.original.file!=c.create.file||f.original.creation!=c.digest{
            return Err(invalid("wrong original FILE/FULL at custody prefix"));}
        let mut sites=if let Some(m)=self.applied{
            if m.original.parent!=f.digest||f.seq>=m.seq||m.original.children.len()>g.children().len()||
                m.original.children.len()!=m.target.children.len(){return Err(invalid("cannot drop a frozen source role or substitute its original FULL"));}
            m.target.clone()
        }else{
            if f.original.volume!=self.volume(){return Err(invalid("wrong original FULL birthplace"));}
            Sites::birthplace(c,f,ChildRoles::None)
        };
        sites.children=match g.children(){
            ChildRoles::None=>ChildRoles::None,ChildRoles::One(_)=>ChildRoles::One(self.child_at(g,0)?.clone()),
            ChildRoles::Two(..)=>ChildRoles::Two(self.child_at(g,0)?.clone(),self.child_at(g,1)?.clone()),
        };
        if let Some(m)=self.applied{sites.check(&m.original.dst_root)?;}Ok(sites)
    }
    pub(super) fn directory(self)->&'a NativeObject{self.applied.map_or(&self.birth.create.directory,|m|&m.target.directory)}
    pub(in crate::ordinary) fn local(self)->&'a NativeObject{self.applied.map_or(&self.birth.create.local,|m|&m.target.local)}
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
    /// An original child decision at the *current* placement. A handoff
    /// carries only its frozen prefix; later local children use their own
    /// named birth. No lookup by current FILE name or content can add roles.
    pub(in crate::ordinary) fn active_child(self,g:super::journal::Graph<'a>,i:usize)->io::Result<&'a NativeObject>{
        if self.retained().is_some(){return Err(invalid("retained FILE is not an ordinary serving site"));}
        self.child_at(g,i)
    }
    fn child_at(self,g:Graph<'a>,i:usize)->io::Result<&'a NativeObject>{
        if g.full().original.file!=self.birth.create.file||g.full().original.creation!=self.birth.digest{
            return Err(invalid("not this active FILE's original graph"));}
        let ds=g.children();let d=match i{0=>ds.first(),1=>ds.second(),_=>None}
            .copied().ok_or_else(||invalid("missing exact ordinal of the selected original child"))?;
        if let Some(m)=self.applied{
            if m.original.parent!=g.full().digest||g.full().seq>=m.seq||m.original.children.len()!=m.target.children.len(){
                return Err(invalid("different frozen original FULL or physical role set at this placement"));}
            let roots=&m.original.children;let at=&m.target.children;
            if d.seq<m.seq{
                let(hash,n)=if i==0{(roots.first(),at.first())}else{(roots.second(),at.second())};
                if hash!=Some(&d.digest){return Err(invalid("wrong original child at this handoff ordinal"));}
                return n.ok_or_else(||invalid("lost frozen target object of a retained formal child"));
            }
            if d.seq==m.seq||i<roots.len(){return Err(invalid("later formal child conflicts with a frozen transfer role"));}
        }
        if self.retained().is_some()||d.original.volume!=self.volume(){return Err(invalid("later child was not authored on the actual active FILE owner"));}
        Ok(&d.material.object)
    }
}
