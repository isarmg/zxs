//! Page a registered ordinary share by *business* names, not host readdir
//! names. No SQLite transaction, root namespace lock or material FD lives
//! between pages. The handle does retain the selected root, marker and S.
use super::{Catalog,Connection,Tree,check,file_create,first_work,sqlite3_step};
use super::super::volume::OrdinaryRoot;
use crate::{EntryId,FileId,OperationId,PoolId,VolumeId,display,fs::{WitnessRead,invalid},
    ordinary::{portable_birth::Birth,working_sql::{Retained,Spec}}};
use std::{collections::BTreeSet,io};

const MAX_PAGE:u8=16;
#[derive(Debug,Eq,PartialEq)]
pub(crate) struct Entry{
    pub(crate) name:String,pub(crate) file:FileId,pub(crate) entry:EntryId,
    pub(crate) revision:u64,pub(crate) size:u64,
    // Proven selection from the SQL cut and the actual F/FILE. A listing
    // is not a capability: a new OPEN must prove its own Tree and member.
    pool:PoolId,volume:VolumeId,
}
impl Entry{
    pub(super) fn matches(&self,b:&Birth)->bool{
        self.pool==b.pool&&self.volume==b.volume&&self.entry==b.entry&&self.file==b.file&&self.name==b.name
    }
}
#[derive(Eq,PartialEq)]
struct Plan{birth:Birth,working:Option<(Spec,bool)>,kept:Retained}
pub(crate) struct RootFiles<'t,'s,'r>{
    tree:&'t Tree<'s,'r>,root:OrdinaryRoot<'r>,after:Vec<u8>,ended:bool,
}
impl<'s,'r> Tree<'s,'r>{
    pub(crate) fn root_files<'t>(&'t self)->io::Result<RootFiles<'t,'s,'r>>{
        let src=self.session.source;
        let c=src.at(|db,c|{self.require(db,&c,false)?;Ok(c)})?;
        let root=OrdinaryRoot::for_body(src,&c,self.setting.volume)?;
        src.at(|db,c|{self.require(db,&c,false)?;root.require_streaming()})?;
        Ok(RootFiles{tree:self,root,after:Vec::new(),ended:false})
    }
}
impl RootFiles<'_,'_,'_>{
    // SQL orders BLOBs by the exact stored bytes. No OFFSET rescans previous
    // pages and an unconfirmed (even physically installed) CREATE is absent.
    fn select(&self,db:&Connection,c:&Catalog,limit:u8)->io::Result<Vec<Plan>>{
        self.tree.require(db,c,false)?;
        let q=db.prepare(c"SELECT e.create_op,e.display,e.file_id FROM main.root_entry e JOIN main.operation o ON o.op_id=e.create_op WHERE e.volume_id=?1 AND e.display>?2 AND o.state=1 ORDER BY e.display LIMIT ?3")?;
        q.bind_bytes(1,&self.tree.setting.volume.0)?;q.bind_bytes(2,&self.after)?;
        q.bind_integer(3,i64::from(limit))?;let mut page:Vec<Plan>=Vec::new();
        loop{match unsafe{sqlite3_step(q.0)}{
            101=>return Ok(page),
            100=>{
                let name=display::component(q.blob(1,222)?,222)?;
                let prior=page.last().map_or(self.after.as_slice(),|p|p.birth.name.as_bytes());
                if page.len()>=usize::from(limit)||name.as_bytes()<=prior{return Err(invalid("unordered or oversized FILE page"));}
                let birth=self.tree.confirmed_birth(db,c,OperationId(q.array(0)?),name,FileId(q.array(2)?))?;
                let(working,kept)=first_work::membership(db,c,&birth)?;
                page.push(Plan{birth,working,kept});
            }
            rc=>{check(rc)?;return Err(invalid("could not select ordinary share members"));}
        }}
    }
    /// A live keyset cursor for this one original tree. Each successful page
    /// is a cut, not a snapshot pretending to span future database commits.
    /// New names at or before an emitted key require a new enumeration.
    pub(crate) fn next(&mut self,limit:u8)->io::Result<Vec<Entry>>{
        self.next_after(limit,||Ok(()))
    }
    pub(in crate::system::reader) fn next_after(&mut self,limit:u8,at_cut:impl FnOnce()->io::Result<()>)->io::Result<Vec<Entry>>{
        if !(1..=MAX_PAGE).contains(&limit){return Err(invalid("invalid FILE page limit"));}
        let src=self.tree.session.source;
        if self.ended{
            src.at(|db,c|{self.tree.require(db,&c,false)?;self.root.require_streaming()})?;
            return Ok(Vec::new());
        }
        let selected=src.at(|db,c|self.select(db,&c,limit))?;
        // At most two retained FDs per admitted entry: actual FILE directory
        // and original linked HEADER. No raw body, table or index is opened.
        let native_sample=||if selected.is_empty(){Ok(BTreeSet::<Vec<u8>>::new())}
            else{self.root.root.names().map(|v|v.into_iter().collect())};
        let sample=native_sample()?;
        let mut pins=Vec::with_capacity(selected.len());
        for plan in &selected{
            pins.push(plan.birth.working_at(&self.root.root,&sample,false,plan.working,&plan.kept)?);
        }
        at_cut()?;
        // The original physical spellings are checked together before
        // disclosure, including files whose displayed name looks like a
        // sibling's storage leaf. No parent rescan separately per child.
        let final_names=native_sample()?;
        for(p,h) in selected.iter().zip(&pins){
            if !final_names.contains(p.birth.container()?.to_bytes())||!final_names.contains(p.birth.witness()?.to_bytes()){
                return Err(invalid("an original FILE spelling disappeared during enumeration"));
            }
            let scope=WitnessRead::at(h.directory())?;
            match p.working{None=>scope.only_header()?,Some((s,done))=>s.members(&scope,done,&p.kept)?,}
            h.require(&self.root.root)?;
        }
        src.at(|db,c|{
            if self.select(db,&c,limit)?!=selected{
                return Err(io::Error::new(io::ErrorKind::WouldBlock,"the selected directory page changed; retry this cursor"));
            }
            for(p,h) in selected.iter().zip(&pins){
                file_create::display_free_in_sample(db,&c,&self.root.root,&p.birth,Some(&final_names))?;
                h.require(&self.root.root)?;
            }
            self.root.require_streaming()
        })?;
        // Nothing is visible and the original cursor is unchanged on any
        // error above. Only verified business metadata escapes, not paths.
        let rows:Vec<_>=selected.into_iter().map(|p|{
            let visible=p.working.filter(|(_,done)|*done).map(|(s,_)|s);
            Entry{pool:p.birth.pool,volume:p.birth.volume,name:p.birth.name,file:p.birth.file,entry:p.birth.entry,
                revision:u64::from(visible.is_some()),size:visible.map_or(0,|s|s.bytes)}
        }).collect();
        if let Some(tail)=rows.last(){self.after=tail.name.as_bytes().to_vec();}
        self.ended=rows.len()<usize::from(limit);Ok(rows)
    }
}
