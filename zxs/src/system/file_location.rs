//! SQLite current placement, separate from original birth/receipt identity.
//! Neither a name nor an old DeleteOp is proof that this FILE still lives
//! on a volume. The actual selected root and its role are checked outside SQL.
use super::{Catalog,Connection,Statement,smb::file_create,sqlite3_column_type,work_record};
use crate::{FileId,OperationId,Role,VolumeId,display,fs::{HeldDir,invalid,directory_birth::DirectoryParent},
    ordinary::portable_birth::{Birth,Installed}};
use std::{collections::BTreeSet,ffi::CString,io};

#[derive(Clone,Debug,Eq,PartialEq)]
pub(super) struct Place{
    pub(super) file:FileId,pub(super) op:OperationId,pub(super) generation:i64,
    pub(super) volume:VolumeId,pub(super) role:Role,pub(super) name:String,pub(super) parent:DirectoryParent,
    /// None means flat, as at birth. Otherwise points to the actual
    /// physical transfer that installed this same private parent.
    pub(super) backing:Option<OperationId>,
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(super) struct Placed {pub(super) birth:Birth,pub(super) here:Place}
impl Place{
    fn decode(q:&Statement<'_>,c:&Catalog)->io::Result<(Self,bool)>{
        let file=FileId(q.array(0)?);let op=OperationId(q.array(1)?);let generation=q.integer(2)?;
        let volume=VolumeId(q.array(3)?);let role=match q.byte(4)?{
            1=>Role::Recycle,2=>Role::File,_=>return Err(invalid("invalid FILE placement role")),
        };
        let name=display::component(q.blob(5,222)?,222)?.to_owned();let kind=q.byte(6)?;let state=q.byte(7)?;
        let parent=DirectoryParent::from_bytes(q.array(8)?);
        let backing=Self::read_backing(q,9,10,file,&name)?;
        let placed_receipt=match kind{
            12=>backing==Some(op),
            14=>role==Role::File&&backing!=Some(op),
            _=>false,
        };
        if file.0==[0;16]||op.0==[0;16]||generation<1||
           !c.rows().iter().any(|v|v.volume==volume&&v.role==role)||
           (generation==1&&(kind!=5||role!=Role::File||backing.is_some()))||
           (generation>1&&(state!=1||!placed_receipt))||state>1||
           (role==Role::Recycle&&parent!=DirectoryParent::Root){
            return Err(invalid("current location disagrees with the original role or receipt"));
        }
        Ok((Self{file,op,generation,volume,role,name,parent,backing},state==1))
    }
    pub(super) fn read_backing(q:&Statement<'_>,slot:i32,terminal:i32,file:FileId,name:&str)->io::Result<Option<OperationId>>{
        let selected=work_record::optional(q,slot)?;
        if let Some(op)=selected{
            Birth::placement_parent(op)?;
            if unsafe{sqlite3_column_type(q.0,terminal)}!=5{return Err(invalid("private FILE backing has a flat name"));}
        }else if Birth::placed_container(name,file)?.to_bytes()!=q.blob(terminal,255)?{
            return Err(invalid("flat FILE terminal differs from the exact name and FileId"));
        }
        Ok(selected)
    }
    pub(super) fn current(db:&Connection,c:&Catalog,file:FileId)->io::Result<Self>{
        let q=db.prepare(c"SELECT l.file_id,l.position_op,l.generation,l.volume_id,l.role,l.display,o.kind,o.state,l.parent_id,l.backing_op,l.flat_terminal
          FROM main.file_location l JOIN main.operation o ON o.op_id=l.position_op WHERE l.file_id=?1")?;
        q.bind_bytes(1,&file.0)?;
        if !work_record::start_one(&q)?{return Err(io::ErrorKind::NotFound.into());}
        let (p,done)=Self::decode(&q,c)?;q.done()?;
        if !done||p.file!=file{return Err(io::ErrorKind::NotFound.into());}
        Ok(p)
    }
    pub(super) fn named(db:&Connection,c:&Catalog,volume:VolumeId,parent:DirectoryParent,name:&str)->io::Result<Option<Self>>{
        display::component(name.as_bytes(),222)?;
        if Self::renaming_name(db,volume,parent,name)?{
            return Err(io::Error::new(io::ErrorKind::WouldBlock,"this exact FILE name belongs to an unconfirmed rename"));
        }
        let q=db.prepare(c"SELECT l.file_id,l.position_op,l.generation,l.volume_id,l.role,l.display,o.kind,o.state,l.parent_id,l.backing_op,l.flat_terminal
          FROM main.file_location l JOIN main.operation o ON o.op_id=l.position_op
          WHERE l.role=2 AND l.volume_id=?1 AND l.display=?2 AND l.parent_id=?3")?;
        q.bind_bytes(1,&volume.0)?;q.bind_bytes(2,name.as_bytes())?;q.bind_bytes(3,&parent.bytes())?;
        if work_record::start_one(&q)?{
            let (p,done)=Self::decode(&q,c)?;q.done()?;
            if !done{return Err(io::Error::new(io::ErrorKind::WouldBlock,"this exact FILE name belongs to an unconfirmed birth"));}
            if p.role!=Role::File||p.volume!=volume||p.parent!=parent||p.name!=name{return Err(invalid("ordinary name is not the original placement"));}
            return Ok(Some(p));
        }
        let q=db.prepare(c"SELECT transfer_op FROM main.pending_transfer
          WHERE direction=2 AND target_id=?1 AND target_name=?2 AND target_parent=?3")?;
        q.bind_bytes(1,&volume.0)?;q.bind_bytes(2,name.as_bytes())?;q.bind_bytes(3,&parent.bytes())?;
        if work_record::start_one(&q)?{
            q.done()?;return Err(io::Error::new(io::ErrorKind::WouldBlock,"this exact FILE name belongs to an unconfirmed restore"));
        }
        Ok(None)
    }
    pub(super) fn renaming_name(db:&Connection,volume:VolumeId,parent:DirectoryParent,name:&str)->io::Result<bool>{
        let q=db.prepare(c"SELECT 1 FROM main.pending_rename WHERE volume_id=?1 AND target_parent=?2 AND target_name=?3
            UNION ALL SELECT 1 FROM main.pending_rename WHERE volume_id=?1 AND target_parent=?2 AND target_terminal=?3
            UNION ALL SELECT 1 FROM main.file_location l JOIN main.pending_rename r ON r.file_id=l.file_id AND r.source_op=l.position_op
              WHERE l.role=2 AND l.volume_id=?1 AND l.parent_id=?2 AND l.display=?3 LIMIT 1")?;
        q.bind_bytes(1,&volume.0)?;q.bind_bytes(2,&parent.bytes())?;q.bind_bytes(3,name.as_bytes())?;
        if !work_record::start_one(&q)?{return Ok(false);}
        q.done()?;Ok(true)
    }
    pub(super) fn require_no_rename(db:&Connection,file:FileId)->io::Result<()>{
        let q=db.prepare(c"SELECT 1 FROM main.pending_rename WHERE file_id=?1 LIMIT 1")?;
        q.bind_bytes(1,&file.0)?;
        if work_record::start_one(&q)?{return Err(io::Error::new(io::ErrorKind::WouldBlock,"this original FILE has an unfinished rename"));}
        Ok(())
    }
    pub(super) fn retained(db:&Connection,c:&Catalog,volume:VolumeId,deleted:OperationId,name:&str)->io::Result<Self>{
        display::component(name.as_bytes(),222)?;
        let q=db.prepare(c"SELECT l.file_id,l.position_op,l.generation,l.volume_id,l.role,l.display,o.kind,o.state,l.parent_id,l.backing_op,l.flat_terminal
          FROM main.file_location l JOIN main.operation o ON o.op_id=l.position_op
          JOIN main.file_transfer t ON t.transfer_op=l.position_op AND t.direction=1 AND t.file_id=l.file_id
          WHERE l.role=1 AND l.volume_id=?1 AND l.position_op=?2 AND l.display=?3")?;
        q.bind_bytes(1,&volume.0)?;q.bind_bytes(2,&deleted.0)?;q.bind_bytes(3,name.as_bytes())?;
        if !work_record::start_one(&q)?{return Err(io::ErrorKind::NotFound.into());}
        let (p,done)=Self::decode(&q,c)?;q.done()?;
        if !done||p.role!=Role::Recycle||p.op!=deleted||p.volume!=volume||p.name!=name{return Err(io::ErrorKind::NotFound.into());}
        Ok(p)
    }
    pub(super) fn bind(self,db:&Connection,c:&Catalog)->io::Result<Placed>{
        let (b,birth_parent)=file_create::committed_origin(db,c,self.file)?;
        if self.file!=b.file||(self.generation==1&&(self.op,self.volume,self.parent,self.name.as_str())!=(b.operation,b.volume,birth_parent,b.name.as_str())){
            return Err(invalid("selected position and immutable birth disagree"));
        }
        Ok(Placed{birth:b,here:self})
    }
    pub(super) fn origin(b:&Birth,parent:DirectoryParent)->Self{
        Self{file:b.file,op:b.operation,generation:1,volume:b.volume,role:Role::File,name:b.name.clone(),parent,backing:None}
    }
    pub(super) fn private_leaf(&self)->io::Result<Option<CString>>{
        self.backing.map(Birth::placement_parent).transpose()
    }
    pub(super) fn leaf(&self)->io::Result<CString>{
        Birth::placed_container(&self.name,self.file)
    }
}
impl Placed{
    pub(super) fn rooted(&self,sample:&BTreeSet<Vec<u8>>)->io::Result<bool>{
        if let Some(leaf)=self.here.private_leaf()?{
            Ok(sample.contains(leaf.to_bytes()))
        }else{
            Ok(sample.contains(self.here.leaf()?.to_bytes())&&sample.contains(self.birth.witness()?.to_bytes()))
        }
    }
    pub(super) fn open(&self,root:&HeldDir,sample:&BTreeSet<Vec<u8>>,write:bool,
        members:impl FnOnce(&crate::fs::WitnessRead<'_>)->io::Result<()>)->io::Result<Installed>{
        if !self.rooted(sample)?{return Err(invalid("missing current FILE placement"));}
        if let Some(leaf)=self.here.private_leaf()?{
            let parent=root.directory_in_sample(&leaf,sample)?.ok_or_else(||invalid("current private FILE parent is not installed"))?;
            let names=parent.names()?.into_iter().collect();
            let body=self.here.leaf()?;
            // A move parent is permanently private, with no user path,
            // other FILE or alternate witness to adopt.
            if names!=BTreeSet::from([body.to_bytes().to_vec(),self.birth.witness()?.to_bytes().to_vec()]){
                return Err(invalid("unknown material in the selected placement parent"));
            }
            self.birth.working_records_in(&parent,&names,&body,write,members)?.with_placement_parent(root,parent)
        }else{
            self.birth.working_records_in(root,sample,&self.here.leaf()?,write,members)
        }
    }
}
