//! Original business names, never the host's directory stream. All ordinary
//! root/FILE traversal is outside SYSTEM transactions, including rare names
//! that spell a sibling's physical terminal. Nothing is released on an error.
use super::{Catalog,Connection,Tree,check,file_create,file_number,first_work,sqlite3_step};
use super::super::volume::OrdinaryRoot;
use crate::{EntryId,FileId,OperationId,PoolId,VolumeId,display,fs::{WitnessRead,invalid},
    ordinary::{portable_birth::Birth,working_sql::{Retained,Spec}}};
use std::{collections::BTreeSet,ffi::CString,io};

pub(super) const MAX_PAGE:u8=16;
#[derive(Debug,Eq,PartialEq)]
pub(crate) struct Entry{
    pub(crate) name:String,pub(crate) file:FileId,pub(crate) entry:EntryId,
    pub(crate) revision:u64,pub(crate) size:u64,pub(super) number:u64,
    pool:PoolId,volume:VolumeId,
}
impl Entry{
    pub(super) fn matches(&self,b:&Birth)->bool{
        self.pool==b.pool&&self.volume==b.volume&&self.entry==b.entry&&self.file==b.file&&self.name==b.name
    }
}
#[derive(Eq,PartialEq)]
struct Plan{
    birth:Birth,working:Option<(Spec,bool)>,kept:Retained,
    display_owner:Option<(Birth,Option<(Spec,bool)>,Retained)>,number:u64,
}
pub(crate) struct RootFiles<'t,'s,'r>{
    tree:&'t Tree<'s,'r>,root:OrdinaryRoot<'r>,after:Vec<u8>,ended:bool,
}
impl<'s,'r> Tree<'s,'r>{
    pub(crate) fn root_files<'t>(&'t self)->io::Result<RootFiles<'t,'s,'r>>{
        let src=self.session.source;
        let c=src.at(|db,c|{self.require(db,&c,false)?;Ok(c)})?;
        let root=OrdinaryRoot::for_body(src,&c,self.setting.volume)?;
        root.require_streaming()?;
        src.at(|db,c|self.require(db,&c,false))?;
        Ok(RootFiles{tree:self,root,after:Vec::new(),ended:false})
    }
}
fn select(tree:&Tree<'_,'_>,db:&Connection,c:&Catalog,after:&[u8],limit:u8)->io::Result<Vec<Plan>>{
    tree.require(db,c,false)?;
    let q=db.prepare(c"SELECT e.create_op,e.display,e.file_id FROM main.root_entry e JOIN main.operation o ON o.op_id=e.create_op WHERE e.volume_id=?1 AND e.display>?2 AND o.state=1 ORDER BY e.display LIMIT ?3")?;
    q.bind_bytes(1,&tree.setting.volume.0)?;q.bind_bytes(2,after)?;
    q.bind_integer(3,i64::from(limit))?;let mut page:Vec<Plan>=Vec::new();
    loop{match unsafe{sqlite3_step(q.0)}{
        101=>return Ok(page),
        100=>{
            let name=display::component(q.blob(1,222)?,222)?;
            let prior=page.last().map_or(after,|p|p.birth.name.as_bytes());
            if page.len()>=usize::from(limit)||name.as_bytes()<=prior{return Err(invalid("unordered or oversized FILE page"));}
            let birth=tree.confirmed_birth(db,c,OperationId(q.array(0)?),name,FileId(q.array(2)?))?;
            let(working,kept)=first_work::membership(db,c,&birth)?;
            let display_owner=file_create::display_owner(db,c,&birth)?;
            let number=file_number::original(db,birth.pool,birth.file)?.words().1;
            page.push(Plan{birth,working,kept,display_owner,number});
        }
        rc=>{check(rc)?;return Err(invalid("could not select ordinary share members"));}
    }}
}
/// A cursor is an ordering key, never a pathname, principal or grant. This
/// entry point borrows the already-open original root of this particular tree.
pub(super) fn page(tree:&Tree<'_,'_>,root:&OrdinaryRoot<'_>,after:&[u8],limit:u8,at_cut:impl FnOnce()->io::Result<()>)->io::Result<Vec<Entry>>{
    if !(1..=MAX_PAGE).contains(&limit){return Err(invalid("invalid FILE page limit"));}
    if !after.is_empty(){display::component(after,222)?;}
    let src=tree.session.source;
    if !root.for_tree(src,tree.setting.volume){return Err(invalid("different original tree root"));}
    let selected=src.at(|db,c|select(tree,db,&c,after,limit))?;
    let native_sample=||if selected.is_empty(){Ok(BTreeSet::<Vec<u8>>::new())}
        else{root.root.names().map(|v|v.into_iter().collect())};
    let sample=native_sample()?;
    let mut pins=Vec::with_capacity(selected.len());
    for plan in &selected{
        pins.push(plan.birth.working_at(&root.root,&sample,false,plan.working,&plan.kept)?);
    }
    at_cut()?;
    let final_names=native_sample()?;
    let mut aliases=Vec::new();
    for(p,h) in selected.iter().zip(&pins){
        if !final_names.contains(p.birth.container()?.to_bytes())||!final_names.contains(p.birth.witness()?.to_bytes()){
            return Err(invalid("an original FILE spelling disappeared during enumeration"));
        }
        let scope=WitnessRead::at(h.directory())?;
        match p.working{None=>scope.only_header()?,Some((s,done))=>s.members(&scope,done,&p.kept)?,}
        h.require(&root.root)?;
        if !Birth::reserved_spelling(p.birth.name.as_bytes()){
            let leaf=CString::new(p.birth.name.as_str()).map_err(|_|invalid("invalid display leaf"))?;
            if root.root.entry_present(&leaf)?{
                let (b,work,kept)=p.display_owner.as_ref().ok_or_else(||invalid("unrecognized native occupant of FILE display name"))?;
                aliases.push(b.working_at(&root.root,&final_names,false,*work,kept)?);
            }
        }
    }
    root.require_streaming()?;
    src.at(|db,c|{
        if select(tree,db,&c,after,limit)?!=selected{
            return Err(io::Error::new(io::ErrorKind::WouldBlock,"the selected directory page changed; retry this cursor"));
        }
        Ok(())
    })?;
    drop(aliases);drop(pins);
    Ok(selected.into_iter().map(|p|{
        let visible=p.working.filter(|(_,done)|*done).map(|(s,_)|s);
        Entry{pool:p.birth.pool,volume:p.birth.volume,name:p.birth.name,file:p.birth.file,entry:p.birth.entry,number:p.number,
            revision:u64::from(visible.is_some()),size:visible.map_or(0,|s|s.bytes)}
    }).collect())
}
impl RootFiles<'_,'_,'_>{
    pub(crate) fn next(&mut self,limit:u8)->io::Result<Vec<Entry>>{self.next_after(limit,||Ok(()))}
    pub(in crate::system::reader) fn next_after(&mut self,limit:u8,at_cut:impl FnOnce()->io::Result<()>)->io::Result<Vec<Entry>>{
        if !(1..=MAX_PAGE).contains(&limit){return Err(invalid("invalid FILE page limit"));}
        if self.ended{
            self.root.require_streaming()?;
            self.tree.session.source.at(|db,c|self.tree.require(db,&c,false))?;
            return Ok(Vec::new());
        }
        let rows=page(self.tree,&self.root,&self.after,limit,at_cut)?;
        if let Some(tail)=rows.last(){self.after=tail.name.as_bytes().to_vec();}
        self.ended=rows.len()<usize::from(limit);Ok(rows)
    }
}
