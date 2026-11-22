//! Exact business children, never the host's directory stream. Ordinary
//! directory and FILE rows occupy the same parent-local byte name.
use super::{Catalog,Connection,InitialReader,check,file_create,file_number,ordinary_directory::{self,DirectoryPlace}};
#[cfg(test)]
use super::{Tree,ordinary_directory::Route};
use super::super::{file_location::Placed,volume::{OrdinaryRoot,OrdinaryView},work_record::{self,Members,ReadRecord}};
use crate::{EntryId,FileId,OperationId,PoolId,VolumeId,display,
    fs::{DIRECTORY_NAMES,HeldDir,WitnessRead,invalid,directory_birth::{DirectoryHold,DirectoryParent}},
    ordinary::portable_birth::{Birth,Installed}};
use std::{collections::BTreeSet,ffi::CString,io};

pub(super) const MAX_PAGE:u8=16;
#[derive(Debug,Eq,PartialEq)]
pub(crate) struct Entry{
    pub(crate) name:String,pub(crate) file:FileId,pub(crate) entry:EntryId,
    pub(crate) revision:u64,pub(crate) size:u64,pub(super) number:u64,
    pool:PoolId,volume:VolumeId,placement:OperationId,generation:i64,parent:DirectoryParent,
}
impl Entry{
    pub(super) fn matches(&self,p:&Placed)->bool{
        let b=&p.birth;let at=&p.here;
        (self.pool,self.volume,self.entry,self.file,self.placement,self.generation,self.parent)==
            (b.pool,at.volume,b.entry,b.file,at.op,at.generation,at.parent)&&self.name==at.name
    }
}
pub(super) enum Child{Directory(DirectoryPlace),File(Entry)}
impl Child{
    pub(super) fn name(&self)->&str{match self{Self::Directory(d)=>d.name(),Self::File(f)=>&f.name}}
}
#[derive(Eq,PartialEq)]
struct Plan{
    placed:Placed,members:Members,visible:Option<ReadRecord>,
    display_owner:Option<file_create::Alias>,number:u64,
}
#[derive(Eq,PartialEq)]
enum Pick{Directory(DirectoryPlace),File(Plan)}
impl Pick{
    fn name(&self)->&str{match self{Self::Directory(d)=>d.name(),Self::File(p)=>&p.placed.here.name}}
}
enum Held{Directory(DirectoryHold),File(Installed)}
// In-process SQLite Tree fixture adapter; SMB uses the bound page reader.
#[cfg(test)]
pub(crate) struct RootFiles<'t,'s,'r>{
    tree:&'t Tree<'s,'r>,root:OrdinaryRoot<'r>,after:Vec<u8>,ended:bool,
}
#[cfg(test)]
impl<'s,'r> Tree<'s,'r>{
    pub(crate) fn root_files<'t>(&'t self)->io::Result<RootFiles<'t,'s,'r>>{
        let src=self.session.source;
        let (c,plan)=src.at(|db,c|{self.require(db,&c,false)?;
            let plan=Route::at(db,&c,self.setting.volume,self.parent)?;Ok((c,plan))})?;
        let root=OrdinaryRoot::at_parent(src,&c,&plan,None)?;
        root.require_streaming()?;
        src.at(|db,c|self.require(db,&c,false))?;
        Ok(RootFiles{tree:self,root,after:Vec::new(),ended:false})
    }
}
/// The rare case where one business name is a sibling's physical terminal.
pub(in crate::system::reader) fn check_alias(root:&OrdinaryRoot<'_>,birth:&Birth,owner:Option<&file_create::Alias>,sample:Option<&BTreeSet<Vec<u8>>>)
    ->io::Result<Option<Installed>>{
    check_alias_name(root,&birth.name,owner,sample)
}
pub(in crate::system::reader) fn check_alias_name(root:&OrdinaryRoot<'_>,name:&str,owner:Option<&file_create::Alias>,sample:Option<&BTreeSet<Vec<u8>>>)
    ->io::Result<Option<Installed>>{
    check_alias_at(root.folder(),name,owner,sample)
}
pub(super) fn check_alias_at(parent:&HeldDir,name:&str,owner:Option<&file_create::Alias>,sample:Option<&BTreeSet<Vec<u8>>>)
    ->io::Result<Option<Installed>>{
    if Birth::reserved_spelling(name.as_bytes()){return Ok(None);}
    let leaf=CString::new(name).map_err(|_|invalid("invalid display leaf"))?;
    if !parent.entry_present(&leaf)?{return Ok(None);}
    let owner=owner.ok_or_else(||invalid("unrecognized native occupant of FILE display name"))?;
    let fresh;
    let names=match sample{Some(x)=>x,None=>{fresh=parent.names()?.into_iter().collect();&fresh}};
    owner.check(parent,names).map(Some)
}
// These are catalog coordinates, not an SMB authorization. The owning
// caller verifies its actual live grant on each of the two short cuts.
#[derive(Clone,Copy)]
struct PageAt<'a>{source:&'a InitialReader,volume:VolumeId,parent:DirectoryParent}

// Only Rust SMB's checked pattern selection uses the extra scan budget.
// Start each authoritative SQL cut with its own deterministic matching work.
type NameFilter<'a>=dyn Fn(&str,&mut usize)->io::Result<bool>+'a;
fn select(at:PageAt<'_>,db:&Connection,c:&Catalog,after:&[u8],limit:u8,mixed:bool,
    filter:Option<&NameFilter<'_>>)->io::Result<Vec<Pick>>{
    let mut q=db.prepare(c"SELECT p.display AS name,p.file_id,0 FROM main.file_location p JOIN main.operation o ON o.op_id=p.position_op
        WHERE p.role=2 AND p.volume_id=?1 AND p.parent_id=?2 AND p.display>?3 AND o.state=1
          AND NOT EXISTS(SELECT 1 FROM main.pending_rename r WHERE r.file_id=p.file_id AND r.source_op=p.position_op)
        UNION ALL SELECT d.display,d.directory_id,1 FROM main.directory_slot d
          WHERE ?4=1 AND d.volume_id=?1 AND d.parent_id=?2 AND d.display>?3 AND d.state=1
        ORDER BY name LIMIT ?5")?;
    q.bind_bytes(1,&at.volume.0)?;q.bind_bytes(2,&at.parent.bytes())?;q.bind_bytes(3,after)?;
    q.bind_integer(4,i64::from(u8::from(mixed)))?;
    q.bind_integer(5,if filter.is_some(){(DIRECTORY_NAMES+1) as i64}else{i64::from(limit)})?;
    let mut page:Vec<Pick>=Vec::new();
    let (mut scanned,mut work)=(0usize,0usize);
    let mut last=[0;display::MAX];
    let mut last_len=after.len();
    if filter.is_some(){last[..last_len].copy_from_slice(after);}
    loop{match q.step(){
        101=>return Ok(page),
        100=>{
            let row=q.row()?;
            let name=display::component(row.blob(0,255)?,255)?;
            if let Some(accept)=filter{
                // A limit sentinel is NOT source exhaustion. Compare all
                // scanned keys, including names skipped by the expression.
                if scanned==DIRECTORY_NAMES{
                    return Err(io::Error::new(io::ErrorKind::OutOfMemory,"ordinary directory expression exceeds its name scan limit"));
                }
                if name.as_bytes()<=&last[..last_len]{return Err(invalid("unordered ordinary scan keys"));}
                scanned+=1;
                last_len=name.len();
                last[..last_len].copy_from_slice(name.as_bytes());
                if !accept(name,&mut work)?{continue;}
            }
            let prior=page.last().map_or(after,|p|p.name().as_bytes());
            if page.len()>=usize::from(limit)||name.as_bytes()<=prior{return Err(invalid("unordered or oversized ordinary page"));}
            match row.byte(2)?{
                0=>{
                    let placed=file_create::placed_at(db,c,at.volume,at.parent,name,Some(FileId(row.array(1)?)))?;
                    let birth=&placed.birth;
                    let members=work_record::members(db,c,birth)?;
                    let visible=work_record::selected(db,birth)?;
                    let at=&placed.here;
                    let display_owner=file_create::display_owner_at(db,c,at.volume,at.parent,name)?;
                    let number=file_number::original(db,birth.pool,birth.file)?.words().1;
                    page.push(Pick::File(Plan{placed,members,visible,display_owner,number}));
                },
                1 if mixed=>{
                    let directory=ordinary_directory::lookup(db,c,at.volume,at.parent,name)?
                        .ok_or_else(||invalid("selected directory is no longer published"))?;
                    if directory.birth().id.0!=row.array::<16>(1)?{return Err(invalid("different selected directory"));}
                    page.push(Pick::Directory(directory));
                },
                _=>return Err(invalid("unknown ordinary member type")),
            }
            // Returning this entire matching prefix does not assert
            // exhaustion; a later query can start at its last *sent* key.
            if filter.is_some()&&page.len()==usize::from(limit){return Ok(page);}
        }
        rc=>{check(rc)?;return Err(invalid("could not select ordinary share members"));}
    }}
}
/// A cursor is an ordering key, never a pathname, principal or grant. No
/// physical inode or storage name is returned from either listing route.
fn read_page_bound(at:PageAt<'_>,root:OrdinaryView<'_>,after:&[u8],limit:u8,mixed:bool,filter:Option<&NameFilter<'_>>,
    require:impl Fn(&Connection,&Catalog)->io::Result<()>,at_cut:impl FnOnce()->io::Result<()>)->io::Result<Vec<Child>>{
    if !(1..=MAX_PAGE).contains(&limit){return Err(invalid("invalid ordinary page limit"));}
    if !after.is_empty(){display::component(after,if mixed{255}else{222})?;}
    let src=at.source;
    if !root.for_tree(src,at.volume)||root.parent()!=at.parent{return Err(invalid("different original ordinary parent"));}
    let select_current=|db:&Connection,c:&Catalog|{require(db,c)?;select(at,db,c,after,limit,mixed,filter)};
    let selected=src.at(|db,c|select_current(db,&c))?;
    let parent=root.folder();
    let native_sample=||if selected.is_empty(){Ok(BTreeSet::<Vec<u8>>::new())}
        else{parent.names().map(|v|v.into_iter().collect())};
    let sample=native_sample()?;
    let mut pins=Vec::with_capacity(selected.len());
    for pick in &selected{
        pins.push(match pick{
            Pick::Directory(d)=>Held::Directory(d.open_selected(parent,&sample)?),
            Pick::File(p)=>Held::File(p.placed.open(parent,&sample,false,|scope|p.members.require(scope))?),
        });
    }
    at_cut()?;
    let final_names=native_sample()?;
    let mut aliases=Vec::new();
    for(p,h) in selected.iter().zip(&pins){
        match(p,h){
            (Pick::Directory(_),Held::Directory(dir))=>dir.require_selected(parent,&final_names)?,
            (Pick::File(p),Held::File(file))=>{
                if !p.placed.rooted(&final_names)?{
                    return Err(invalid("an original FILE spelling disappeared during enumeration"));
                }
                p.members.require(&WitnessRead::at(file.directory())?)?;
                file.require(parent)?;
                if let Some(pin)=check_alias_at(parent,&p.placed.here.name,p.display_owner.as_ref(),Some(&final_names))?{aliases.push(pin);}
            },
            _=>return Err(invalid("different original child proof")),
        }
    }
    root.require_streaming()?;
    src.at(|db,c|{
        if select_current(db,&c)?!=selected{
            return Err(io::Error::new(io::ErrorKind::WouldBlock,"the selected directory page changed; retry this cursor"));
        }
        Ok(())
    })?;
    drop(aliases);drop(pins);
    Ok(selected.into_iter().map(|p|match p{
        Pick::Directory(d)=>Child::Directory(d),
        Pick::File(p)=>{
            let visible=p.visible.map(|r|r.record);
            let Placed{birth,here}=p.placed;
            Child::File(Entry{pool:birth.pool,volume:here.volume,name:here.name,file:birth.file,entry:birth.entry,
                placement:here.op,generation:here.generation,parent:here.parent,number:p.number,
                revision:visible.map_or(0,|r|r.key().revision),size:visible.map_or(0,|r|r.logical())})
        }
    }).collect())
}
#[cfg(test)]
fn read_page(tree:&Tree<'_,'_>,root:&OrdinaryRoot<'_>,after:&[u8],limit:u8,mixed:bool,at_cut:impl FnOnce()->io::Result<()>)->io::Result<Vec<Child>>{
    let at=PageAt{source:tree.session.source,volume:tree.setting.volume,parent:tree.parent};
    read_page_bound(at,root.view(),after,limit,mixed,None,|db,c|tree.require(db,c,false),at_cut)
}
#[cfg(test)]
pub(super) fn page(tree:&Tree<'_,'_>,root:&OrdinaryRoot<'_>,after:&[u8],limit:u8,at_cut:impl FnOnce()->io::Result<()>)->io::Result<Vec<Entry>>{
    read_page(tree,root,after,limit,false,at_cut)?.into_iter().map(|item|match item{
        Child::File(f)=>Ok(f),Child::Directory(_)=>Err(invalid("not a FILE listing")),
    }).collect()
}
#[cfg(test)]
pub(super) fn mixed_page(tree:&Tree<'_,'_>,root:&OrdinaryRoot<'_>,after:&[u8],limit:u8,at_cut:impl FnOnce()->io::Result<()>)->io::Result<Vec<Child>>{
    read_page(tree,root,after,limit,true,at_cut)
}
#[cfg(test)]
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

#[path="smb_rust_directory.rs"]
mod rust;
pub(crate) use rust::{RustDirectoryName,RustDirectoryPage,RustDirectoryRename,RustOrdinaryDirectory,RustOrdinaryKind};
pub(super) use rust::RustDirectorySelection;
