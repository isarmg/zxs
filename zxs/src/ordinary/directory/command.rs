use super::{inventory,operand::Operand,path::HeldParent,prepare,types::{self,Created,Parent}};
use crate::{fs::{self,invalid},ordinary::{self,allocation,Cut,FileInfo,journal::{self,Decision,Journal},material},
    pool::lease::{Access,LocalPool},DirectoryId,EntryId,OperationId};
use std::{io,path::Path};

#[derive(Clone,Debug,Eq,PartialEq)]
pub struct DirectoryInfo{pub directory:DirectoryId,pub entry:EntryId,pub parent:Parent,pub name:String,
    pub created_by:OperationId,pub sequence:u64}
impl DirectoryInfo{
    pub(in crate::ordinary) fn original(d:&Created)->Self{let o=&d.origin;
        Self{directory:o.id,entry:o.entry,parent:o.parent.address,name:o.name.clone(),created_by:o.op,sequence:d.seq}}
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub enum DirectoryEntry{File(FileInfo),Directory(DirectoryInfo)}
impl DirectoryEntry{
    pub fn name(&self)->&str{match self{Self::File(f)=>&f.name,Self::Directory(d)=>&d.name}}
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub struct DirectoryListing{
    pub entries:Vec<DirectoryEntry>,pub reserved_private_entries:usize,
    pub pending_moves:Vec<ordinary::MoveReceipt>,pub pending_lifecycle:Vec<ordinary::RecycleReceipt>,
}
/// An original receipt after cleanup is not a current availability report.
pub fn make_directory(system:&Path,op:OperationId,address:Parent,name:&str)->io::Result<DirectoryInfo>{
    make(system,op,address,name,|_|Ok(()))
}
pub(super) fn make(system:&Path,op:OperationId,address:Parent,name:&str,mut hook:impl FnMut(Cut)->io::Result<()>)->io::Result<DirectoryInfo>{
    let(p,_,s)=LocalPool::with_snapshot(system,Access::Write)?;p.require_business_op(op)?;
    let req=types::request(p.id(),op,fs::local_uid(),address,name)?;
    if let Some(d)=s.decided(op){let row=d.directory()?;
        if row.request!=req{return Err(invalid("different request for an original mkdir"));}
        if !s.making.as_ref().is_some_and(|a|a.origin.op==op){return Ok(DirectoryInfo::original(row));}
    }
    if let Some(a)=&s.making{
        if a.origin.op!=op||a.request!=req{return Err(invalid("another original mkdir or different original request"));}
    }
    let volume=p.ordinary(address.volume())?;let parent=HeldParent::open(&volume,address)?;
    if s.making.is_none(){prepare::issue(&parent,op,name)?;}drop(s);hook(Cut::Reservation)?;
    let a=prepare::stage(&parent,op,req,&mut hook)?;
    let(_,s)=p.snapshot()?;
    if let Some(d)=s.decided(op){
        return finish(&parent,&a,d.directory()?,s.pending.as_ref(),&mut hook);
    }
    if s.pending.is_none(){
        let st=a.stage.as_ref().ok_or_else(||invalid("cannot seal unbound mkdir"))?;
        let f=Created::new(p.id(),a.seq,a.previous,a.origin.clone(),a.digest,st.slot,st.object.clone())?;
        s.room_for(f.bytes.len())?;hook(Cut::BeforeIntent)?;
        let(_,now)=p.snapshot()?;before_floor(&parent,&now,&a,&f)?;
        journal::Intent::seal(&p.system,&f.bytes)?;
    }
    drop(s);hook(Cut::Intent)?;
    let exact=|s:&Journal,d:&Decision|before_floor(&parent,s,&a,d.directory()?);
    let sealed=ordinary::publish_sealed_checked(&p,op,req,|d|{
        let(_,s)=p.snapshot()?;exact(&s,d)
    },Some(&exact),&mut hook)?;
    finish(&parent,&a,sealed.frame.directory()?,Some(&sealed),&mut hook)
}
fn before_floor(parent:&HeldParent<'_,'_>,s:&Journal,a:&Operand,d:&Created)->io::Result<()>{
    let p=parent.volume().pool;let current=s.making.as_ref().ok_or_else(||invalid("lost original mkdir before confirmation"))?;
    if current.stamp!=a.stamp||current.bytes!=a.bytes||!current.matches(d)||!a.matches(d)||s.decided(d.origin.op).is_some()||
        s.frontier()!=(d.seq-1,d.previous)||s.pending.as_ref().is_some_and(|i|i.frame.bytes()!=d.bytes){
        return Err(invalid("different mkdir reservation, frame or source frontier before confirmation"));}
    s.new_directory_origin(&d.origin,&p.journal_registration())?;
    if parent.basis()!=&d.origin.parent{return Err(invalid("mkdir moved between parent selections"));}
    let raw=inventory::census(parent,s)?;material::check_leaf(parent.directory(),&raw.into_iter().collect::<Vec<_>>(),&inventory::human(&d.origin.name)?,false)?;
    let h=parent.directory().child_directory(&material::staging(d.origin.ticket,d.slot))?.ok_or_else(||invalid("original empty directory not staged"))?;
    inventory::same_child(&h,&d.object)?;h.only(None)?;parent.require()
}
/// Post-decision: no new issuer or rollback. Check the complete original
/// directory is at exactly one name, while the original allocation still
/// prevents children or unrelated new publications.
fn ready(parent:&HeldParent<'_,'_>,a:&Operand,d:&Created,intent:Option<&journal::Intent>)->io::Result<bool>{
    let(_,s)=parent.volume().pool.snapshot()?;let now=s.making.as_ref().ok_or_else(||invalid("lost original mkdir before release"))?;
    let same_intent=match(s.pending.as_ref(),intent){
        (Some(i),Some(original))=>i.stamp==original.stamp&&i.bytes==original.bytes,(None,None)=>true,_=>false,
    };
    if !same_intent||!s.decided(d.origin.op).is_some_and(|row|row.bytes()==d.bytes)||!now.matches(d)||now.stamp!=a.stamp||now.bytes!=a.bytes||
        parent.basis()!=&d.origin.parent||s.pending.as_ref().is_some_and(|i|i.frame.bytes()!=d.bytes){
        return Err(invalid("not this exact published directory, parent or installation ticket"));}
    let raw=inventory::census(parent,&s)?;
    material::check_leaf(parent.directory(),&raw.iter().cloned().collect::<Vec<_>>(),&inventory::human(&d.origin.name)?,true)?;
    Ok(raw.contains(d.origin.name.as_bytes()))
}
fn finish(parent:&HeldParent<'_,'_>,a:&Operand,d:&Created,intent:Option<&journal::Intent>,hook:&mut impl FnMut(Cut)->io::Result<()>)->io::Result<DirectoryInfo>{
    let p=parent.volume().pool;let dir=parent.directory();hook(Cut::Decision)?;
    let name=inventory::human(&d.origin.name)?;let private=material::staging(d.origin.ticket,d.slot);
    if !ready(parent,a,d,intent)?{
        dir.handoff_directory(&private,dir,&name,d.object.entity,&d.object.birth,||hook(Cut::DirectoryRenamed))?;
    }
    if !ready(parent,a,d,intent)?{return Err(invalid("original mkdir is not installed at its final name"));}
    dir.file.sync_all()?;if !ready(parent,a,d,intent)?{return Err(invalid("original directory not installed after parent synchronization"));}
    hook(Cut::Installation)?;
    if !ready(parent,a,d,intent)?{return Err(invalid("original mkdir changed before intent release"));}
    if let Some(i)=intent{p.system.remove_original_record(journal::INTENT,&i.stamp,&i.bytes)?;}
    hook(Cut::IntentReleased)?;
    if !ready(parent,a,d,None)?{return Err(invalid("original mkdir changed before reservation release"));}
    p.system.remove_original_record(allocation::NAME,&a.stamp,&a.bytes)?;hook(Cut::Release)?;
    Ok(DirectoryInfo::original(d))
}
pub fn list_directory(system:&Path,address:Parent)->io::Result<DirectoryListing>{
    let cut=ordinary::formal::ParentCut::open(system,Access::Read)?;
    let(r,folders)=ordinary::parent_listing(&cut,address)?;
    let mut entries:Vec<_>=r.files.into_iter().map(DirectoryEntry::File).chain(folders.into_iter().map(DirectoryEntry::Directory)).collect();
    entries.sort_by(|l,r|l.name().cmp(r.name()));
    Ok(DirectoryListing{entries,reserved_private_entries:r.reserved_private_entries,pending_moves:r.pending_moves,pending_lifecycle:r.pending_lifecycle})
}

#[cfg(test)]
#[path="../../../tests/unit/ordinary_directories.rs"]
mod tests;
