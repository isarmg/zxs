//! At most the original FULL and two immediate, ordered formal children.
//! This projection proves ancestry at a prefix. Only the live native cut
//! may decide *where* that selected immutable original can be opened.
use super::{Commit,Delta,Frame,Journal,cycle::Parent};
use crate::{FileId,NodeId,fs::invalid,ordinary::custody::transfer::ChildRoles};
use std::io;

#[derive(Clone,Copy)]
pub(in crate::ordinary) struct Graph<'a>{full:&'a Commit,children:ChildRoles<&'a Delta>}
impl<'a> Graph<'a>{
    pub(in crate::ordinary) fn at(c:&Frame,f:&'a Commit,children:&[&'a Delta])->io::Result<Self>{
        if c.seq>=f.seq||f.original.file!=c.create.file||f.original.history!=c.create.history||f.original.creation!=c.digest{
            return Err(invalid("wrong original FILE or first FULL in retained formal graph"));}
        let items=match children{[]=>ChildRoles::None,[one]=>ChildRoles::One(*one),
            [one,two]=>ChildRoles::Two(*one,*two),_=>return Err(invalid("unsupported depth of retained FILE history"))};
        let(mut seq,mut parent)=(f.seq,Parent::full(f));
        for d in items.iter().copied(){
            if d.seq<=seq||!parent.precedes(d){return Err(invalid("missing, reversed or substituted immediate formal FILE parent"));}
            seq=d.seq;parent=Parent::incremental(d);
        }Ok(Self{full:f,children:items})
    }
    pub(in crate::ordinary) fn full(self)->&'a Commit{self.full}
    pub(in crate::ordinary) fn children(self)->ChildRoles<&'a Delta>{self.children}
    pub(in crate::ordinary) fn tip(self)->Parent{match self.children{
        ChildRoles::None=>Parent::full(self.full),ChildRoles::One(d)|ChildRoles::Two(_,d)=>Parent::incremental(d),
    }}
    pub(in crate::ordinary) fn through(self,node:NodeId)->io::Result<Self>{
        let selected=if node==self.full.original.node{ChildRoles::None}
        else if let Some(n)=self.children.first().copied().filter(|d|d.original.node==node){ChildRoles::One(n)}
        else if self.children.second().is_some_and(|d|d.original.node==node){self.children}
        else{return Err(io::Error::new(io::ErrorKind::NotFound,"no such published node in this original FILE graph"));};
        Ok(Self{full:self.full,children:selected})
    }
}
impl Journal{
    /// The index contains only positions of actual confirmed frames. Do not
    /// consult the pending allocation/seal or treat an unavailable node as F0.
    pub(in crate::ordinary) fn actual_graph(&self,id:FileId)->io::Result<Graph<'_>>{
        let c=self.file(id).ok_or_else(||invalid("missing original FILE birth at formal cutoff"))?;
        let f=self.head(id).ok_or_else(||invalid("no confirmed first FULL at formal cutoff"))?;
        let ids=self.deltas.get(&id).map(Vec::as_slice).unwrap_or(&[]);
        let child=|i:usize|self.decisions.get(i).ok_or_else(||invalid("missing indexed original INCR"))?.delta();
        match ids{
            []=>Graph::at(c,f,&[]),[i]=>Graph::at(c,f,&[child(*i)?]),
            [i,j]=>Graph::at(c,f,&[child(*i)?,child(*j)?]),
            _=>Err(invalid("unsupported number of retained original INCRs")),
        }
    }
}
