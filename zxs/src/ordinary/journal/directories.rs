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
    fn unoccupied_ordinary_slot(&self)->io::Result<()>{
        if self.making.is_some()||self.allocated.is_some()||self.saving.is_some()||self.committing.is_some()||
            self.editing.is_some()||self.incrementing.is_some()||self.moving.is_some()||self.pending.is_some()||self.tail!=0{
            return Err(invalid("a different original command occupies this SYSTEM slot"));}
        Ok(())
    }
    pub(in crate::ordinary) fn new_directory_slot(&self,o:&DirOrigin,reg:&Allowed<'_>)->io::Result<()>{
        self.unoccupied_ordinary_slot()?;self.new_directory_origin(o,reg)?;self.room_for(MAX_FRAME)
    }
    /// Project the original ordinary parent from THIS registration and
    /// confirmed prefix, never from a physical path or a supplied mkdir
    /// row. This is not an installation witness or an authorization to write.
    pub(in crate::ordinary) fn file_parent_at(&self,address:DirParent,uid:u32,r:&Allowed<'_>)->io::Result<super::super::file_parent::BirthParent>{
        use super::super::{directory::types::Basis,file_parent::BirthParent};
        if self.genesis!=r.basis().2{return Err(invalid("not this SYSTEM original for FILE parent selection"));}
        r.ordinary_path_budget(address.volume(),uid)?;
        let Some(d)=parent_row(&self.directory_rows,&self.decisions,address)? else{return Ok(BirthParent::Root)};
        let o=&d.origin;
        if self.making.as_ref().is_some_and(|a|a.origin.id==o.id)||
            self.names.get(o.parent.address,&o.name)!=Some(Member::Directory{entry:o.entry,directory:o.id}){
            return Err(invalid("ordinary FILE parent is uninstalled or not its exact directory entry"));}
        let b=Basis{address,anchor:d.digest,object:d.object.clone()};b.require(r,uid,Some(d))?;
        Ok(BirthParent::Child{directory:o.id,decision:d.digest,object:d.object.clone()})
    }
    /// Only the parent part of prospective FILE birth admission. Proves
    /// exact pool, parent, next sequence and worst reachable parent paths;
    /// no FILE allocation, current physical grant or business decision.
    pub(in crate::ordinary) fn check_next_file_parent(&self,r:&Allowed<'_>,f:&Frame)->io::Result<()>{
        let(prev,head)=self.frontier();let c=&f.create;let address=c.parent.address(c.volume);
        if Frame::read(r.basis().0,&f.bytes)?!=*f||prev.checked_add(1)!=Some(f.seq)||head!=f.previous||
            c.parent!=self.file_parent_at(address,c.uid,r)?{
            return Err(invalid("prospective FILE does not bind this confirmed parent and original prefix"));}
        match address{
            DirParent::Root(v)=>{super::super::material::ticket_budget(r.ordinary_path_budget(v,c.uid)?,&c.name,c.attempt)?;},
            DirParent::Child(_,id)=>{self.child_file_geometry(id,&c.name,c.attempt)?;},
        }Ok(())
    }
    fn child_file_geometry(&self,id:DirectoryId,name:&str,ticket:OperationId)->io::Result<()>{
        let b=self.directory_rows.get(&id).ok_or_else(||invalid("lost confirmed FILE parent path"))?.1;
        super::super::material::child_residence_budget(b,name,ticket)?;Ok(())
    }
    /// This is an original prefix check for a NEW prospective child birth.
    /// Not for resuming an existing operation or issuing physical material.
    pub(in crate::ordinary) fn prospective_child_file(&self,t:&allocation::Terms<'_>,r:&Allowed<'_>)->io::Result<()>{
        let address=t.parent.address(t.volume);let DirParent::Child(_,id)=address else{return Err(invalid("not a child FILE request"));};
        let(n,prev)=self.frontier();let(pool,init,_)=r.basis();self.unoccupied_ordinary_slot()?;
        if t.pool!=pool||n.checked_add(1)!=Some(t.seq)||t.previous!=prev||*t.parent!=self.file_parent_at(address,t.uid,r)?||
            !r.file(t.volume,t.uid,t.root.entity,&t.root.birth,true)||self.names.get(address,t.name).is_some()||
            self.entry_used(t.entry)||self.file(t.file).is_some()||self.on_all().any(|f|f.create.history==t.history)||
            t.op==t.attempt||[t.op,t.attempt].into_iter().any(|v|v==init||self.ticket_used(v)){
            return Err(invalid("different original parent, occupied entry, identity, operation or private ticket"));}
        self.child_file_geometry(id,t.name,t.attempt)?;self.room_for(MAX_FRAME)
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
