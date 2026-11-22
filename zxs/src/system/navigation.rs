//! Management's logical ordinary namespace, from one held SYSTEM read cut.
use super::{WebAdmin,WebCurrent,Connection,Catalog,InitialReader,check,share_mode::{self,Mode,Setting}};
use super::super::{Db,file_location::Place,smb::ordinary_directory::Route,work_record};
use crate::{DirectoryId,FileId,OperationId,PoolId,Role,display,fs::{DIRECTORY_NAMES,invalid,directory_birth::DirectoryParent},
    hash::Sha256,identity::ShareId};
use std::{cell::Cell,ffi::{c_int,c_void},io};

const PAGE:usize=32;
const INSTRUCTIONS_PER_VOTE:c_int=1000;
const VOTES:u32=4000;

#[derive(Debug,Eq,PartialEq)]
pub(crate) struct Directory{pub(crate) id:DirectoryId,pub(crate) name:String}
#[derive(Debug,Eq,PartialEq)]
pub(crate) enum Entry{Directory(Directory),File{id:FileId,name:String}}
pub(crate) struct Page<'s>{
    pub(crate) ancestors:Vec<Directory>,
    pub(crate) entries:Vec<Entry>,
    pub(crate) next:Option<Cursor<'s>>,
}
/// A caller retains this on the original Web session. Each page is live:
/// a changed parent, grant or final entry requires restarting the branch.
pub(crate) struct Cursor<'s>{original:&'s InitialReader,continuation:Continuation}
pub(in crate::system::reader) struct Continuation{scope:[u8;32],last:Key}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
enum Member{Directory(DirectoryId),File(FileId)}
#[derive(Clone,Debug,Eq,PartialEq)]
struct Key{member:Member,name:String,position:OperationId,generation:i64}
impl Key{
    fn require(&self,db:&Connection,c:&Catalog,parent:&Route)->io::Result<()>{
        match self.member{
            Member::Directory(id)=>{
                let (name,position,generation)=parent.visible_child(db,c,id)?.ok_or_else(stale)?;
                if (name.as_str(),position,generation)!=(self.name.as_str(),self.position,self.generation){return Err(stale());}
            }
            Member::File(id)=>{
                let selected=Place::named(db,c,parent.volume(),parent.parent(),&self.name)?.ok_or_else(stale)?.bind(db,c)?;
                let at=selected.here;
                if (selected.birth.file,at.file,at.role,at.op,at.generation)!=(id,id,Role::File,self.position,self.generation){return Err(stale());}
            }
        }
        Ok(())
    }
    fn entry(self)->Entry{
        match self.member{
            Member::Directory(id)=>Entry::Directory(Directory{id,name:self.name}),
            Member::File(id)=>Entry::File{id,name:self.name},
        }
    }
}
fn stale()->io::Error{io::Error::new(io::ErrorKind::WouldBlock,"ordinary navigation changed; start a new page")}
fn unavailable()->io::Error{io::ErrorKind::NotFound.into()}

unsafe extern "C"{
    fn sqlite3_progress_handler(db:*mut Db,steps:c_int,callback:Option<unsafe extern "C" fn(*mut c_void)->c_int>,data:*mut c_void);
}
struct Progress<'a>{db:&'a Connection,remaining:&'a Cell<u32>}
impl<'a> Progress<'a>{
    fn install(db:&'a Connection,remaining:&'a Cell<u32>)->Self{
        // SAFETY: This synchronous read-only cut owns the one handler.
        // The guard borrows its original connection and live callback data.
        unsafe{sqlite3_progress_handler(db.0,INSTRUCTIONS_PER_VOTE,Some(navigation_vote),std::ptr::from_ref(remaining).cast_mut().cast());}
        Self{db,remaining}
    }
}
impl Drop for Progress<'_>{
    fn drop(&mut self){
        // SAFETY: The original connection remains live while clearing the handler.
        unsafe{sqlite3_progress_handler(self.db.0,0,None,std::ptr::null_mut());}
    }
}
unsafe extern "C" fn navigation_vote(data:*mut c_void)->c_int{
    // SAFETY: navigation_cut keeps this exact Cell alive on the read
    // connection's owning thread until the registered callback is removed.
    let left=unsafe{&*data.cast::<Cell<u32>>()};
    let v=left.get().saturating_sub(1);left.set(v);c_int::from(v==0)
}
fn navigation_cut<T>(db:&Connection,votes:u32,read:impl FnOnce()->io::Result<T>)->io::Result<T>{
    let left=Cell::new(votes);
    let guard=Progress::install(db,&left);
    let result=read();
    let exhausted=guard.remaining.get()==0;
    drop(guard);
    if exhausted{return Err(io::Error::new(io::ErrorKind::WouldBlock,"ordinary navigation exceeded its read budget"));}
    result
}
fn browseable(db:&Connection,c:&Catalog,share:ShareId)->io::Result<Setting>{
    share_mode::installed(db,c,share)?.filter(|s|s.mode!=Mode::Disabled).ok_or_else(unavailable)
}

const CANDIDATES:&std::ffi::CStr=c"SELECT
    (SELECT count(*) FROM (SELECT 1 FROM main.directory_entry INDEXED BY ordinary_directory_name
         WHERE volume_id=?1 AND parent_id=?2 LIMIT ?3))+
    (SELECT count(*) FROM (SELECT 1 FROM main.file_location INDEXED BY current_ordinary_name
         WHERE role=2 AND volume_id=?1 AND parent_id=?2 LIMIT ?3))";
// Scan just the indexed parent. A direct correlated test avoids materializing
// the pool's directory_slot rename view. A pending source is not a tree item.
const CHILDREN:&std::ffi::CStr=c"SELECT p.display AS name,p.file_id,0,p.position_op,p.generation
    FROM main.file_location p INDEXED BY current_ordinary_name JOIN main.operation o ON o.op_id=p.position_op
    WHERE p.role=2 AND p.volume_id=?1 AND p.parent_id=?2 AND p.display>?3 AND o.state=1
      AND NOT EXISTS(SELECT 1 FROM main.pending_rename r WHERE r.file_id=p.file_id AND r.source_op=p.position_op)
    UNION ALL SELECT d.display,d.directory_id,1,d.position_op,d.generation FROM main.directory_selected d
    WHERE d.volume_id=?1 AND d.parent_id=?2 AND d.display>?3 AND d.state=1
      AND NOT EXISTS(SELECT 1 FROM main.pending_directory_rename r WHERE r.directory_id=d.directory_id AND r.source_op=d.position_op)
    ORDER BY name LIMIT ?4";
fn candidates(db:&Connection,parent:&Route,after:&[u8])
    ->io::Result<Vec<Key>>{
    let parent_id=parent.parent().bytes();let volume=parent.volume();
    let count=db.prepare(CANDIDATES)?;
    count.bind_bytes(1,&volume.0)?;count.bind_bytes(2,&parent_id)?;count.bind_integer(3,(DIRECTORY_NAMES+1) as i64)?;
    if !work_record::start_one(&count)?{return Err(invalid("missing ordinary parent count"));}
    let n=count.integer(0)?;
    count.done()?;drop(count);
    if !(0..=DIRECTORY_NAMES as i64).contains(&n){return Err(invalid("ordinary parent exceeds the navigation row limit"));}

    let mut q=db.prepare(CHILDREN)?;
    q.bind_bytes(1,&volume.0)?;q.bind_bytes(2,&parent_id)?;q.bind_bytes(3,after)?;q.bind_integer(4,(PAGE+1) as i64)?;
    let mut rows=Vec::<Key>::with_capacity(PAGE+1);
    loop{match q.step(){
        101=>return Ok(rows),
        100=>{
            let id=q.array::<16>(1)?;let position=OperationId(q.array(3)?);let generation=q.integer(4)?;
            let (member,max)=match q.byte(2)?{
                0=>(Member::File(FileId(id)),222),
                1=>(Member::Directory(DirectoryId(id)),255),
                _=>return Err(invalid("unknown navigation member kind")),
            };
            let name=display::component(q.row()?.blob(0,max)?,max)?.to_owned();
            if [id,position.0].contains(&[0;16])||generation<1||rows.len()>PAGE||
                name.as_bytes()<=rows.last().map_or(after,|p|p.name.as_bytes()){
                return Err(invalid("invalid original child in navigation page"));
            }
            rows.push(Key{member,name,position,generation});
        },
        rc=>{check(rc)?;return Err(invalid("unexpected navigation selection result"));}
    }}
}
fn scope_identity(pool:PoolId,admin:&WebCurrent,setting:Setting,parent:&Route)->io::Result<[u8;32]>{
    let mut h=Sha256::new();h.update(b"ZXS-WEB-NAVIGATION-1\0")?;
    for id in [pool.0,admin.account.0,admin.canonical.0,admin.op.0,setting.share.0,setting.volume.0,setting.op.0,parent.parent().bytes()]{
        h.update(&id)?;
    }
    h.update(&admin.generation.to_be_bytes())?;
    h.update(&setting.revision.to_be_bytes())?;h.update(&[setting.mode as u8])?;
    for (id,op,generation,name) in parent.logical_positions(){
        h.update(&id.0)?;h.update(&op.0)?;h.update(&generation.to_be_bytes())?;
        h.update(&[name.len() as u8])?;h.update(name.as_bytes())?;
    }
    Ok(h.finish())
}
fn select_page(db:&Connection,c:&Catalog,identity:&WebCurrent,share:ShareId,parent:Option<DirectoryId>,after:Option<&Continuation>)
    ->io::Result<(Vec<Directory>,Vec<Entry>,Option<Continuation>)>{
    let setting=browseable(db,c,share)?;
    let route=Route::at(db,c,setting.volume,parent.map_or(DirectoryParent::Root,DirectoryParent::Directory))?;
    let scope=scope_identity(c.birth.pool,identity,setting,&route)?;
    let after=if let Some(p)=after{
        if p.scope!=scope{return Err(stale());}
        p.last.require(db,c,&route)?;
        p.last.name.as_bytes()
    }else{b""};
    let mut rows=candidates(db,&route,after)?;
    for key in &rows{key.require(db,c,&route)?;}
    let next=if rows.len()>PAGE{
        rows.truncate(PAGE);
        rows.last().cloned().map(|last|Continuation{scope,last})
    }else{None};
    let ancestors=route.logical_positions().map(|(id,_,_,name)|Directory{id,name:name.into()}).collect();
    Ok((ancestors,rows.into_iter().map(Key::entry).collect(),next))
}
impl<'s> WebAdmin<'s>{
    pub(crate) fn navigation(&self,share:ShareId,parent:Option<DirectoryId>,after:Option<&Cursor<'s>>)->io::Result<Page<'s>>{
        self.source.at(|db,c|navigation_cut(db,VOTES,||{
            let identity=self.current(db,&c)?;
            if after.is_some_and(|p|!std::ptr::eq(p.original,self.source)){return Err(stale());}
            let (ancestors,entries,next)=select_page(db,&c,&identity,share,parent,after.map(|p|&p.continuation))?;
            Ok(Page{ancestors,entries,next:next.map(|continuation|Cursor{original:self.source,continuation})})
        }))
    }
}


#[cfg(test)]
#[path = "../../tests/unit/attached/system/navigation/tests.rs"]
mod tests;

#[cfg(test)]
include!("../../tests/unit/attached/system/navigation/items.rs");
