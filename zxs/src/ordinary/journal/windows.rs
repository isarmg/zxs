//! Exact working ordinals between two original formal boundaries. Indexes
//! refer only to admitted SYSTEM frames and are never a second durable store.
use super::{cycle::Parent,Decision,Delta,Journal,OriginalIndex,Saved};
use crate::{FileId,OperationId,fs::invalid};
use std::{collections::BTreeMap,io};

fn positions(steps:&[usize],after:usize,end:usize)->&[usize]{
    // after is the zero-based formal frame; end is the exclusive upper edge.
    let lo=steps.partition_point(|&i|i<=after);let hi=steps.partition_point(|&i|i<end);
    &steps[lo..hi]
}
pub(super) fn hide_last(rows:&mut BTreeMap<FileId,Vec<usize>>,file:FileId,index:usize)->io::Result<()>{
    let list=rows.get_mut(&file).ok_or_else(||invalid("unconfirmed formal child has no FILE index"))?;
    if list.last()!=Some(&index){return Err(invalid("unconfirmed formal child is not this FILE's last original"));}
    list.pop();if list.is_empty(){rows.remove(&file);}Ok(())
}
#[derive(Clone,Copy)]
pub(in crate::ordinary) struct WorkWindow<'a>{parent:Parent,frames:&'a[Decision],ids:&'a[usize]}
impl<'a> WorkWindow<'a>{
    pub(in crate::ordinary) fn parent(self)->Parent{self.parent}
    pub(in crate::ordinary) fn rows(self)->impl DoubleEndedIterator<Item=&'a Saved>+Clone+'a{
        let frames=self.frames;self.ids.iter().map(move|&i|match &frames[i]{
            Decision::Span(s)=>s,_=>unreachable!("only admitted span positions are indexed"),
        })
    }
    pub(in crate::ordinary) fn last(self)->Option<&'a Saved>{self.rows().next_back()}
    fn new(file:FileId,ds:&'a[Decision],end:usize,first:usize,children:&[usize],work:&'a[usize])->io::Result<Self>{
        if end>ds.len()||first>=end{return Err(invalid("formal baseline not present at original working cutoff"));}
        let at=children[..children.partition_point(|&i|i<end)].last().copied().unwrap_or(first);
        let parent=match ds.get(at){Some(Decision::Formal(f)) if at==first=>Parent::full(f),
            Some(Decision::Delta(d)) if at>first=>Parent::incremental(d),_=>return Err(invalid("wrong original formal index at working cutoff"))};
        if parent.file!=file{return Err(invalid("working cycle belongs to another FILE"));}
        Ok(Self{parent,frames:ds,ids:positions(work,at,end)})
    }
}
impl OriginalIndex{
    pub(super) fn work<'a>(&'a self,file:FileId,ds:&'a[Decision])->io::Result<WorkWindow<'a>>{
        let first=*self.formal.get(&file).ok_or_else(||invalid("no original FULL at working cutoff"))?;
        WorkWindow::new(file,ds,ds.len(),first,self.deltas.get(&file).map(Vec::as_slice).unwrap_or(&[]),
            self.spans.get(&file).map(Vec::as_slice).unwrap_or(&[]))
    }
}
impl Journal{
    pub(super) fn work_at(&self,file:FileId,end:usize)->io::Result<WorkWindow<'_>>{
        let first=*self.formal.get(&file).ok_or_else(||invalid("FILE has no confirmed FULL"))?;
        WorkWindow::new(file,&self.decisions,end,first,self.deltas.get(&file).map(Vec::as_slice).unwrap_or(&[]),
            self.spans.get(&file).map(Vec::as_slice).unwrap_or(&[]))
    }
    pub(in crate::ordinary) fn active_work(&self,file:FileId)->io::Result<WorkWindow<'_>>{self.work_at(file,self.decisions.len())}
    pub(in crate::ordinary) fn published_work(&self,d:&Delta)->io::Result<WorkWindow<'_>>{
        let at=usize::try_from(d.seq).ok().and_then(|i|i.checked_sub(1)).ok_or_else(||invalid("invalid submitted sequence"))?;
        if !matches!(self.decisions.get(at),Some(Decision::Delta(original)) if original==d){
            return Err(invalid("a reservation, unconfirmed suffix or unrelated child cannot select spent working material"));
        }
        let v=self.work_at(d.original.file,at)?;
        let last=v.last().ok_or_else(||invalid("original formal submission lacks its input edge"))?;
        if !v.parent().child(d,last){return Err(invalid("formal submission does not identify the working input of this prefix"));}
        Ok(v)
    }
    pub(in crate::ordinary) fn submission_input(&self,file:FileId,op:OperationId)->io::Result<WorkWindow<'_>>{
        match self.decided(op){
            Some(d)=>{let original=d.delta()?;if original.original.file!=file{return Err(invalid("formal operation belongs to another FILE"));}
                self.published_work(original)}
            None=>self.active_work(file),
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/attached/ordinary/journal/windows/tests.rs"]
mod tests;
