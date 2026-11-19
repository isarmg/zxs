//! Namespace relationships come only from the ordered, admitted SYSTEM
//! prefix. These projections confer neither physical installation nor writes.
use super::*;

pub(super) type DirectoryRows=BTreeMap<DirectoryId,(usize,fs::PathBudget)>;
fn parent_row<'a>(rows:&DirectoryRows,ds:&'a[Decision],p:DirParent)->io::Result<Option<&'a DirCreated>>{
    let DirParent::Child(volume,id)=p else{return Ok(None)};
    let d=rows.get(&id).and_then(|&(n,_)|ds.get(n)).ok_or_else(||invalid("no confirmed ordinary directory at this parent address"))?.directory()?;
    if d.origin.id!=id||d.origin.parent.address.volume()!=volume{return Err(invalid("original directory is on a different volume"));}
    Ok(Some(d))
}
/// Preflight both the business name and every possible name issued by this
/// ticket. The parent is the original registered root plus its confirmed
/// human-name chain; no per-frame reconstruction of that chain is needed.
pub(super) fn child_budget(rows:&DirectoryRows,o:&DirOrigin,reg:&Allowed<'_>)->io::Result<fs::PathBudget>{
    let parent=match o.parent.address{
        DirParent::Root(v)=>reg.ordinary_path_budget(v,o.uid)?,
        DirParent::Child(_,id)=>rows.get(&id).ok_or_else(||invalid("no earlier directory path for original ticket"))?.1,
    };
    super::super::material::ticket_budget(parent,&o.name,o.ticket)
}
fn new_origin(rows:&DirectoryRows,entries:&BTreeSet<EntryId>,names:&Names,ds:&[Decision],reg:&Allowed<'_>,o:&DirOrigin,
    used:impl Fn(OperationId)->bool)->io::Result<()>{
    o.parent.require(reg,o.uid,parent_row(rows,ds,o.parent.address)?)?;
    child_budget(rows,o,reg)?;
    let init=reg.basis().1;
    if rows.contains_key(&o.id)||entries.contains(&o.entry)||names.get(o.parent.address,&o.name).is_some()||
        o.op==o.ticket||[o.op,o.ticket].into_iter().any(|v|v==init||used(v)){
        return Err(invalid("occupied ordinary name, entry, directory or original operation/ticket"));}
    Ok(())
}
impl OriginalIndex{
    pub(super) fn directory_parent<'a>(&self,ds:&'a[Decision],p:DirParent)->io::Result<Option<&'a DirCreated>>{
        parent_row(&self.directory_rows,ds,p)
    }
    pub(super) fn new_directory(&self,o:&DirOrigin,ds:&[Decision],reg:&Allowed<'_>)->io::Result<()>{
        new_origin(&self.directory_rows,&self.entries,&self.names,ds,reg,o,|v|self.ops.contains(&v)||self.tickets.contains(&v))
    }
}
impl Journal{
    /// Same path, identity and ticket bounds as cold original admission. No
    /// physical parent selection, reservation, or permission is minted here.
    pub(in crate::ordinary) fn new_directory_origin(&self,o:&DirOrigin,reg:&Allowed<'_>)->io::Result<()>{
        if self.genesis!=reg.basis().2{return Err(invalid("not the same original SYSTEM prefix"));}
        new_origin(&self.directory_rows,&self.entries,&self.names,&self.decisions,reg,o,|v|self.ticket_used(v))
    }
    pub(in crate::ordinary) fn directories_at(&self,parent:DirParent)->impl Iterator<Item=&DirCreated>{
        self.directory_rows.values().filter_map(move |&(i,_)|self.decisions.get(i).and_then(|d|d.directory().ok())
            .filter(|d|d.origin.parent.address==parent))
    }
    pub(in crate::ordinary) fn new_directory_slot(&self,o:&DirOrigin,reg:&Allowed<'_>)->io::Result<()>{
        if self.making.is_some()||self.allocated.is_some()||self.saving.is_some()||self.committing.is_some()||
            self.editing.is_some()||self.incrementing.is_some()||self.moving.is_some()||self.pending.is_some()||self.tail!=0{
            return Err(invalid("a different original command occupies this SYSTEM slot"));}
        self.new_directory_origin(o,reg)?;self.room_for(MAX_FRAME)
    }
    /// Root to selected child, bounded before touching any physical child.
    /// An arbitrary decoded Created is not an argument. A still-unconfirmed
    /// final frame was removed from these very indexes before Journal exists.
    pub(in crate::ordinary) fn directory_lineage(&self,p:DirParent)->io::Result<Vec<&DirCreated>>{
        let volume=p.volume();let mut parent=p;let mut rows=Vec::new();let mut after=u64::MAX;
        while let Some(d)=parent_row(&self.directory_rows,&self.decisions,parent)?{
            let o=&d.origin;if rows.len()>=fs::MAX_PATH_DEPTH||d.seq>=after||o.parent.address.volume()!=volume||
                self.names.get(o.parent.address,&o.name)!=Some(Member::Directory{entry:o.entry,directory:o.id}){
                return Err(invalid("invalid depth, chronology or type at this original directory"));}
            rows.push(d);after=d.seq;parent=o.parent.address;
        }
        if parent!=DirParent::Root(volume){return Err(invalid("ordinary directory did not reach the selected registered root"));}
        rows.reverse();Ok(rows)
    }
}

#[cfg(test)]
#[path="../../../tests/unit/directory_prefix.rs"]
mod tests;
