//! Original business names, never the host's directory stream. All ordinary
//! root/FILE traversal is outside SYSTEM transactions, including rare names
//! that spell a sibling's physical terminal. Nothing is released on an error.
use super::{Catalog,Connection,Tree,check,file_create,file_number,sqlite3_step};
use super::super::{file_location::Placed,volume::OrdinaryRoot,work_record::{self,Members,ReadRecord}};
use crate::{EntryId,FileId,OperationId,PoolId,VolumeId,display,fs::{WitnessRead,invalid},
    ordinary::portable_birth::Birth};
use std::{collections::BTreeSet,ffi::CString,io};

pub(super) const MAX_PAGE:u8=16;
#[derive(Debug,Eq,PartialEq)]
pub(crate) struct Entry{
    pub(crate) name:String,pub(crate) file:FileId,pub(crate) entry:EntryId,
    pub(crate) revision:u64,pub(crate) size:u64,pub(super) number:u64,
    pool:PoolId,volume:VolumeId,placement:OperationId,generation:i64,
}
impl Entry{
    pub(super) fn matches(&self,p:&Placed)->bool{
        let b=&p.birth;let at=&p.here;
        (self.pool,self.volume,self.entry,self.file,self.placement,self.generation)==
            (b.pool,at.volume,b.entry,b.file,at.op,at.generation)&&self.name==at.name
    }
}
#[derive(Eq,PartialEq)]
struct Plan{
    placed:Placed,members:Members,visible:Option<ReadRecord>,
    display_owner:Option<file_create::Alias>,number:u64,
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
/// The rare case where one business name is a sibling's physical terminal.
pub(in crate::system::reader) fn check_alias(root:&OrdinaryRoot<'_>,birth:&Birth,owner:Option<&file_create::Alias>,sample:Option<&BTreeSet<Vec<u8>>>)
    ->io::Result<Option<crate::ordinary::portable_birth::Installed>>{
    check_alias_name(root,&birth.name,owner,sample)
}
pub(in crate::system::reader) fn check_alias_name(root:&OrdinaryRoot<'_>,name:&str,owner:Option<&file_create::Alias>,sample:Option<&BTreeSet<Vec<u8>>>)
    ->io::Result<Option<crate::ordinary::portable_birth::Installed>>{
    if Birth::reserved_spelling(name.as_bytes()){return Ok(None);}
    let leaf=CString::new(name).map_err(|_|invalid("invalid display leaf"))?;
    if !root.root.entry_present(&leaf)?{return Ok(None);}
    let owner=owner.ok_or_else(||invalid("unrecognized native occupant of FILE display name"))?;
    let fresh;
    let names=match sample{Some(x)=>x,None=>{fresh=root.root.names()?.into_iter().collect();&fresh}};
    owner.check(&root.root,names).map(Some)
}
fn select(tree:&Tree<'_,'_>,db:&Connection,c:&Catalog,after:&[u8],limit:u8)->io::Result<Vec<Plan>>{
    tree.require(db,c,false)?;
    let q=db.prepare(c"SELECT p.display,p.file_id FROM main.file_location p JOIN main.operation o ON o.op_id=p.position_op WHERE p.role=2 AND p.volume_id=?1 AND p.display>?2 AND o.state=1 ORDER BY p.display LIMIT ?3")?;
    q.bind_bytes(1,&tree.setting.volume.0)?;q.bind_bytes(2,after)?;
    q.bind_integer(3,i64::from(limit))?;let mut page:Vec<Plan>=Vec::new();
    loop{match unsafe{sqlite3_step(q.0)}{
        101=>return Ok(page),
        100=>{
            let name=display::component(q.blob(0,222)?,222)?;
            let prior=page.last().map_or(after,|p|p.placed.here.name.as_bytes());
            if page.len()>=usize::from(limit)||name.as_bytes()<=prior{return Err(invalid("unordered or oversized FILE page"));}
            let placed=tree.placed(db,c,name,Some(FileId(q.array(1)?)))?;
            let birth=&placed.birth;
            let members=work_record::members(db,c,birth)?;
            let visible=work_record::selected(db,birth)?;
            let display_owner=file_create::display_owner_at(db,c,placed.here.volume,name)?;
            let number=file_number::original(db,birth.pool,birth.file)?.words().1;
            page.push(Plan{placed,members,visible,display_owner,number});
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
        pins.push(plan.placed.open(&root.root,&sample,false,|scope|plan.members.require(scope))?);
    }
    at_cut()?;
    let final_names=native_sample()?;
    let mut aliases=Vec::new();
    for(p,h) in selected.iter().zip(&pins){
        if !p.placed.rooted(&final_names)?{
            return Err(invalid("an original FILE spelling disappeared during enumeration"));
        }
        let scope=WitnessRead::at(h.directory())?;
        p.members.require(&scope)?;
        h.require(&root.root)?;
        if let Some(pin)=check_alias_name(root,&p.placed.here.name,p.display_owner.as_ref(),Some(&final_names))?{aliases.push(pin);}
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
        let visible=p.visible.map(|r|r.record);
        let Placed{birth,here}=p.placed;
        Entry{pool:birth.pool,volume:here.volume,name:here.name,file:birth.file,entry:birth.entry,placement:here.op,generation:here.generation,number:p.number,
            revision:visible.map_or(0,|r|r.key().revision),size:visible.map_or(0,|r|r.logical())}
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
