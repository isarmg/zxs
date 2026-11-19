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
/// Collect the original directory proof from the *given* admitted prefix.
fn file_anchor(rows:&DirectoryRows,ds:&[Decision],names:&Names,address:DirParent,uid:u32,reg:&Allowed<'_>)->io::Result<super::super::file_parent::BirthParent>{
    use super::super::{directory::types::Basis,file_parent::BirthParent};
    reg.ordinary_path_budget(address.volume(),uid)?;
    let Some(d)=parent_row(rows,ds,address)? else{return Ok(BirthParent::Root)};
    let o=&d.origin;
    if names.get(o.parent.address,&o.name)!=Some(Member::Directory{entry:o.entry,directory:o.id}){
        return Err(invalid("ordinary directory is not the exact original member at this prefix"));}
    Basis{address,anchor:d.digest,object:d.object.clone()}.require(reg,uid,Some(d))?;
    Ok(BirthParent::Child{directory:o.id,decision:d.digest,object:d.object.clone()})
}
/// Only a prefix relation, not a physical grant. A RESTORE never looks up
/// the old deletion site's volume or timestamps to select a new destination.
pub(super) fn handoff_parents(rows:&DirectoryRows,ds:&[Decision],names:&Names,reg:&Allowed<'_>,before:Placement<'_>,o:&MoveOrigin)->io::Result<()>{
    let fixed=o.parent_choice().bind(o.purpose.call(),o.from,o.to,before.retained_from()?,
        |p|file_anchor(rows,ds,names,p,o.uid,reg))?;
    if fixed!=o.parents{return Err(invalid("custody source or native ordinary directory differs from its original prefix"));}
    // Use only the two endpoints of *this* original operation. An explicit
    // restore does not consult the old deletion site's physical directory.
    use super::super::custody::transfer::ParentEnd;
    let resident=super::super::material::resident(&o.name,o.file,o.ticket)?;
    let(from,to)=o.ends()?;
    for end in [from,to]{
        let b=match end{
            ParentEnd::Ordinary(parent)=>reg.ordinary_names(parent.volume(),o.uid,parent_budget(rows,reg,parent,o.uid)?)?,
            ParentEnd::Recycle(_)=>reg.recycle_names(o.uid)?,
        };resident.at(b)?;
    }
    Ok(())
}
fn parent_budget(rows:&DirectoryRows,reg:&Allowed<'_>,p:DirParent,uid:u32)->io::Result<fs::PathBudget>{
    match p{DirParent::Root(v)=>reg.ordinary_path_budget(v,uid),
        DirParent::Child(_,id)=>rows.get(&id).map(|x|x.1).ok_or_else(||invalid("missing original ordinary parent path"))}
}
/// Preflight both the business name and every possible name issued by this
/// ticket. The parent is the original registered root plus its confirmed
/// human-name chain; no per-frame reconstruction of that chain is needed.
pub(super) fn child_budget(rows:&DirectoryRows,o:&DirOrigin,reg:&Allowed<'_>)->io::Result<fs::PathBudget>{
    let parent=parent_budget(rows,reg,o.parent.address,o.uid)?;
    super::super::material::directory_budget(reg.ordinary_names(o.parent.address.volume(),o.uid,parent)?,&o.name,o.ticket)
}
fn new_origin(rows:&DirectoryRows,entries:&BTreeSet<EntryId>,names:&Names,ds:&[Decision],reg:&Allowed<'_>,o:&DirOrigin,
    used:impl Fn(OperationId)->bool)->io::Result<()>{
    o.parent.require(reg,o.uid,parent_row(rows,ds,o.parent.address)?)?;
    child_budget(rows,o,reg)?;
    names.vacancy(o.parent.address,&o.name,Member::Directory{entry:o.entry,directory:o.id})?;
    let init=reg.basis().1;
    if rows.contains_key(&o.id)||entries.contains(&o.entry)||names.get(o.parent.address,&o.name).is_some()||
        o.op==o.ticket||[o.op,o.ticket].into_iter().any(|v|v==init||used(v)){
        return Err(invalid("occupied ordinary name, entry, directory or original operation/ticket"));}
    Ok(())
}
impl OriginalIndex{
    /// A new FILE entry belongs to one prior, installed namespace, not just
    /// a volume number. The same admission is used for its bound frame and
    /// its unconfirmed reservation; neither check itself changes a name.
    fn birth_namespace(&self,parent:&super::super::file_parent::BirthParent,volume:VolumeId,uid:u32,
        name:&str,file:FileId,entry:EntryId,history:HistoryId,op:OperationId,attempt:OperationId,
        ds:&[Decision],reg:&Allowed<'_>)->io::Result<()>{
        let address=parent.address(volume);let used=|id|self.ops.contains(&id)||self.tickets.contains(&id);
        if *parent!=file_anchor(&self.directory_rows,ds,&self.names,address,uid,reg)?||
            self.files.contains_key(&file)||self.entries.contains(&entry)||self.histories.contains(&history)||
            op==attempt||used(op)||used(attempt){return Err(invalid("wrong original FILE parent, reused identity or operation/ticket"));}
        self.names.vacancy(address,name,Member::File{entry,file})?;
        let path=parent_budget(&self.directory_rows,reg,address,uid)?;
        super::super::material::birth_geometry(reg,volume,uid,path,name,file,attempt)?;Ok(())
    }
    pub(super) fn birth_at_parent(&self,f:&Frame,ds:&[Decision],reg:&Allowed<'_>)->io::Result<()>{
        let c=&f.create;
        self.birth_namespace(&c.parent,c.volume,c.uid,&c.name,c.file,c.entry,c.history,c.op,c.attempt,ds,reg)?;
        for n in [&c.directory,&c.local]{
            if self.native.contains(&n.birth)||!reg.directory_child(c.volume,c.uid,n.entity,&n.birth){
                return Err(invalid("FILE must own two previously unused native objects on its ordinary volume"));}}
        Ok(())
    }
    pub(super) fn birth_reservation(&self,a:&Allocation,ds:&[Decision],reg:&Allowed<'_>)->io::Result<()>{
        self.birth_namespace(&a.parent,a.volume,a.uid,&a.name,a.file,a.entry,a.history,a.op,a.attempt,ds,reg)?;
        if a.stage.as_ref().is_some_and(|st|self.native.contains(&st.object.birth)||
            !reg.directory_child(a.volume,a.uid,st.object.entity,&st.object.birth)){
            return Err(invalid("reserved FILE stage is not a new original on its ordinary volume"));}Ok(())
    }
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
        if self.genesis!=r.basis().2{return Err(invalid("not this SYSTEM original for FILE parent selection"));}
        if matches!(address,DirParent::Child(_,id) if self.making.as_ref().is_some_and(|a|a.origin.id==id)){
            return Err(invalid("ordinary FILE parent installation is not finished"));}
        file_anchor(&self.directory_rows,&self.decisions,&self.names,address,uid,r)
    }
    /// Only the parent part of prospective FILE birth admission. Proves
    /// exact pool, parent, next sequence and worst reachable parent paths;
    /// no FILE allocation, current physical grant or business decision.
    pub(in crate::ordinary) fn check_next_file_parent(&self,r:&Allowed<'_>,f:&Frame)->io::Result<()>{
        let(prev,head)=self.frontier();let c=&f.create;let address=c.parent.address(c.volume);
        if Frame::read(r.basis().0,&f.bytes)?!=*f||prev.checked_add(1)!=Some(f.seq)||head!=f.previous||
            c.parent!=self.file_parent_at(address,c.uid,r)?{
            return Err(invalid("prospective FILE does not bind this confirmed parent and original prefix"));}
        self.file_geometry(r,address,c.uid,&c.name,c.file,c.attempt)?;Ok(())
    }
    pub(in crate::ordinary) fn ordinary_parent_names(&self,r:&Allowed<'_>,address:DirParent,uid:u32)->io::Result<fs::NameBudget>{
        if self.genesis!=r.basis().2{return Err(invalid("different original SYSTEM namespace"));}
        r.ordinary_names(address.volume(),uid,parent_budget(&self.directory_rows,r,address,uid)?)
    }
    fn file_geometry(&self,r:&Allowed<'_>,address:DirParent,uid:u32,name:&str,file:FileId,ticket:OperationId)->io::Result<()>{
        let b=parent_budget(&self.directory_rows,r,address,uid)?;
        super::super::material::birth_geometry(r,address.volume(),uid,b,name,file,ticket)?;Ok(())
    }
    /// This is an original prefix check for a NEW prospective child birth.
    /// Not for resuming an existing operation or issuing physical material.
    pub(in crate::ordinary) fn prospective_child_file(&self,t:&allocation::Terms<'_>,r:&Allowed<'_>)->io::Result<()>{
        let address=t.parent.address(t.volume);let DirParent::Child(_, _)=address else{return Err(invalid("not a child FILE request"));};
        let(n,prev)=self.frontier();let(pool,init,_)=r.basis();self.unoccupied_ordinary_slot()?;
        if t.pool!=pool||n.checked_add(1)!=Some(t.seq)||t.previous!=prev||*t.parent!=self.file_parent_at(address,t.uid,r)?||
            !r.file(t.volume,t.uid,t.root.entity,&t.root.birth,true)||self.names.get(address,t.name).is_some()||
            self.entry_used(t.entry)||self.file(t.file).is_some()||self.on_all().any(|f|f.create.history==t.history)||
            t.op==t.attempt||[t.op,t.attempt].into_iter().any(|v|v==init||self.ticket_used(v)){
            return Err(invalid("different original parent, occupied entry, identity, operation or private ticket"));}
        self.names.vacancy(address,t.name,Member::File{entry:t.entry,file:t.file})?;
        self.file_geometry(r,address,t.uid,t.name,t.file,t.attempt)?;self.room_for(MAX_FRAME)
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
