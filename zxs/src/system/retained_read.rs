//! Read only the confirmed retained generation, qualified by the active
//! native SMB account and by the actual dedicated R. A FILE's old DeleteOp
//! never opens a subsequent retained generation or a different volume.
use super::{InitialReader,SmbSession,file_number::{self,NativeKey},smb_buffer};
use super::super::{Catalog,Connection,file_location::{Place,Placed},volume::{RecycleMedia,RecycleRoot},
    work_record::{self,Members,ReadRecord}};
use crate::{FileId,OperationId,Role,fs::{WitnessRead,invalid},
    ordinary::{portable_birth::Installed,working_round::{End,Selected,Used}}};
use std::{cell::Cell,io::{self,Write}};

pub(super) struct Drive<'s,'r>{session:&'s SmbSession<'r>,root:RecycleRoot<'r>}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(super) struct Item{
    pub(super) op:OperationId,pub(super) name:String,pub(super) file:FileId,
}
#[derive(Clone,Copy,Eq,PartialEq)]
pub(super) enum KeepReader{WhileRetained,ShareDelete}
#[derive(PartialEq,Eq)]
struct Cut {place:Placed,members:Members,selected:Option<(ReadRecord,End)>}
pub(super) struct Reader<'t,'s,'r>{drive:&'t Drive<'s,'r>,original:RetainedFile}
// No borrowed protocol session or self reference in the physical hold.
// That same caller's authority is applied anew at each real read cut.
struct RetainedFile{
    place:Placed,members:Members,view:Option<ReadRecord>,body:Option<Selected>,hold:Installed,keep:KeepReader,
}
struct RetainedAt<'a>{
    source:&'a InitialReader,media:&'a RecycleMedia,
    require:&'a dyn Fn(&Connection,&Catalog)->io::Result<()>,
}
struct ReadAt<'a>{site:RetainedAt<'a>,file:&'a RetainedFile}
impl<'r> SmbSession<'r>{
    pub(super) fn retained<'s>(&'s self)->io::Result<Drive<'s,'r>>{
        let source=self.source;let c=source.at(|db,c|{self.require_current(db,&c)?;Ok(c)})?;
        let root=RecycleRoot::at(source,&c)?;
        source.at(|db,c|self.require_current(db,&c))?;
        Ok(Drive{session:self,root})
    }
}
impl<'s,'r> Drive<'s,'r>{
    pub(super) fn volume(&self)->crate::VolumeId{self.root.volume()}
    fn with_site<T>(&self,act:impl FnOnce(RetainedAt<'_>)->io::Result<T>)->io::Result<T>{
        let require=|db:&Connection,c:&Catalog|self.session.require_current(db,c);
        act(RetainedAt{source:self.session.source,media:self.root.media(),require:&require})
    }
    pub(super) fn page(&self,after:Option<OperationId>,limit:u8)->io::Result<Vec<Item>>{
        self.with_site(|at|at.page(after,limit))
    }
    pub(super) fn deletion(&self,op:OperationId)->io::Result<Item>{
        self.with_site(|at|at.deletion(op))
    }
    pub(super) fn open<'t>(&'t self,deleted:OperationId,name:&str,keep:KeepReader)
        ->io::Result<(Reader<'t,'s,'r>,NativeKey)>{
        let (original,key)=self.with_site(|at|at.open(deleted,name,keep))?;
        Ok((Reader{drive:self,original},key))
    }
}
impl RetainedAt<'_>{
    fn select(&self,db:&Connection,c:&Catalog)->io::Result<()>{
        (self.require)(db,c)?;
        if !c.rows().get(1).is_some_and(|r|r.role==Role::Recycle&&r.volume==self.media.volume()){
            return Err(invalid("selected R is not the original retained volume"));
        }
        Ok(())
    }
    fn page_at(&self,db:&Connection,c:&Catalog,after:Option<OperationId>,limit:u8)->io::Result<Vec<Item>>{
        self.select(db,c)?;
        let q=db.prepare(c"SELECT l.position_op,l.display,l.file_id FROM main.file_location l
          JOIN main.operation o ON l.position_op=o.op_id AND o.kind=12 AND o.state=1
          JOIN main.file_transfer m ON m.transfer_op=l.position_op AND m.file_id=l.file_id AND m.direction=1
          WHERE l.volume_id=?1 AND l.role=1 AND l.position_op>?2 ORDER BY l.position_op LIMIT ?3")?;
        q.bind_bytes(1,&self.media.volume().0)?;q.bind_bytes(2,&after.unwrap_or(OperationId([0;16])).0)?;
        q.bind_integer(3,i64::from(limit))?;let mut rows=Vec::new();
        while work_record::start_one(&q)?{
            rows.push(Item{op:OperationId(q.array(0)?),name:crate::display::component(q.blob(1,222)?,222)?.to_owned(),
                file:FileId(q.array(2)?)});
        }
        Ok(rows)
    }
    fn page(&self,after:Option<OperationId>,limit:u8)->io::Result<Vec<Item>>{
        if limit>16{return Err(invalid("invalid retained page bound"));}
        let src=self.source;
        let start=src.at(|db,c|self.page_at(db,&c,after,limit))?;
        self.media.require(src)?;
        src.at(|db,c|{
            if self.page_at(db,&c,after,limit)?!=start{return Err(io::ErrorKind::WouldBlock.into());}
            Ok(start)
        })
    }
    /// One virtual deletion directory names exactly one retained generation.
    fn deletion(&self,op:OperationId)->io::Result<Item>{
        let cut=|db:&Connection,c:&Catalog|->io::Result<Item>{
            self.select(db,c)?;
            let q=db.prepare(c"SELECT l.display,l.file_id FROM main.file_location l
               JOIN main.file_transfer m ON m.transfer_op=l.position_op AND m.direction=1 AND m.file_id=l.file_id
               JOIN main.operation o ON o.op_id=m.transfer_op AND o.kind=12 AND o.state=1
               WHERE l.volume_id=?1 AND l.role=1 AND l.position_op=?2")?;
            q.bind_bytes(1,&self.media.volume().0)?;q.bind_bytes(2,&op.0)?;
            if !work_record::start_one(&q)?{return Err(io::ErrorKind::NotFound.into());}
            let item=Item{op,name:crate::display::component(q.blob(0,222)?,222)?.to_owned(),file:FileId(q.array(1)?)};
            q.done()?;Ok(item)
        };
        let src=self.source;let start=src.at(|db,c|cut(db,&c))?;
        self.media.require(src)?;
        src.at(|db,c|if cut(db,&c)?==start{Ok(start)}else{Err(io::ErrorKind::WouldBlock.into())})
    }
    fn pick(&self,db:&Connection,c:&Catalog,deleted:OperationId,name:&str)->io::Result<Cut>{
        self.select(db,c)?;
        choose_file_at(db,c,self.media.volume(),deleted,name)
    }
    fn open(&self,deleted:OperationId,name:&str,keep:KeepReader)->io::Result<(RetainedFile,NativeKey)>{
        let src=self.source;let first=src.at(|db,c|self.pick(db,&c,deleted,name))?;
        self.open_selected(deleted,name,keep,first)
    }
    fn open_selected(&self,deleted:OperationId,name:&str,keep:KeepReader,first:Cut)->io::Result<(RetainedFile,NativeKey)>{
        let src=self.source;
        let root=self.media.root_for_read();
        let hold=first.place.open(root,&root.names()?.into_iter().collect(),false,|scope|first.members.require(scope))?;
        let body=first.selected.map(|(r,_)|Selected::open_settled(hold.directory(),r.record)).transpose()?;
        self.media.require(src)?;hold.require(root)?;
        if let Some(view)=&body{view.require(hold.directory())?;}
        let key=src.at(|db,c|{
            if self.pick(db,&c,deleted,name)?!=first{return Err(io::ErrorKind::WouldBlock.into());}
            file_number::original(db,first.place.birth.pool,first.place.birth.file)
        })?;
        Ok((RetainedFile{place:first.place,members:first.members,view:first.selected.map(|x|x.0),body,hold,keep},key))
    }
}
fn choose_file_at(db:&Connection,c:&Catalog,volume:crate::VolumeId,deleted:OperationId,name:&str)->io::Result<Cut>{
    let place=Place::retained(db,c,volume,deleted,name)?.bind(db,c)?;
    let members=work_record::members(db,c,&place.birth)?;
    let selected=work_record::selected(db,&place.birth)?.map(|r|(r,r.record.end()));
    Ok(Cut{place,members,selected})
}
impl Reader<'_,'_,'_>{
    pub(super) fn place(&self)->&Place{&self.original.place.here}
    pub(super) fn identifiers(&self)->[[u8;16];4]{
        let b=&self.original.place.birth;[b.pool.0,self.original.place.here.volume.0,b.entry.0,b.file.0]
    }
    pub(super) fn size(&self)->u64{self.original.view.map_or(0,|r|r.record.logical())}
    pub(super) fn revision(&self)->u64{self.original.view.map_or(0,|r|r.record.key().revision)}
    fn with_read<T>(&self,action:impl FnOnce(ReadAt<'_>)->io::Result<T>)->io::Result<T>{
        self.drive.with_site(|site|action(ReadAt{site,file:&self.original}))
    }
    pub(super) fn range_evidence(&self,offset:u64,n:u64,out:impl Write)->io::Result<Used>{
        self.with_read(|read|read.range_evidence(offset,n,out))
    }
    pub(super) fn smb_read_window<T>(&self,at:u64,n:u32,consume:impl FnOnce(&[u8])->io::Result<T>)->io::Result<T>{
        self.with_read(|read|smb_buffer::deliver(at,n,self.size(),|at,n,buf|read.range_evidence(at,n,buf),consume,|x|read.finish(x)))
    }
}
impl ReadAt<'_>{
    fn check_at(&self,db:&Connection,c:&Catalog)->io::Result<()>{
        self.site.select(db,c)?;
        let old=&self.file.place.here;
        match Place::retained(db,c,old.volume,old.op,&old.name){
            Ok(current)if current==*old=>{},
            Err(e)if e.kind()==io::ErrorKind::NotFound&&self.file.keep==KeepReader::ShareDelete=>{
                // Only this already-held lease may use the old R bytes.
                // A new open or a restore must still select the current R row.
                let q=db.prepare(c"SELECT 1 FROM main.file_transfer m
                  JOIN main.operation o ON o.op_id=m.transfer_op AND o.kind=12 AND o.state=1
                  WHERE m.direction=2 AND m.file_id=?1 AND m.source_op=?2 AND m.source_gen=?3 AND m.source_id=?4 AND m.source_name=?5")?;
                q.bind_bytes(1,&old.file.0)?;q.bind_bytes(2,&old.op.0)?;q.bind_integer(3,old.generation)?;
                q.bind_bytes(4,&old.volume.0)?;q.bind_bytes(5,old.name.as_bytes())?;
                if !work_record::start_one(&q)?{return Err(io::ErrorKind::NotFound.into());}q.done()?;
            }
            Err(e)=>return Err(e),
            _=>return Err(invalid("retained reader no longer selects its original FILE")),
        }
        if let Some(view)=self.file.view{
            if work_record::confirmed(db,&self.file.place.birth,view.record.key().save)?!=view{
                return Err(invalid("retained reader lost its exact confirmed view"));
            }
        }
        Ok(())
    }
    fn physical(&self,full:bool)->io::Result<()>{
        self.site.media.require(self.site.source)?;self.file.hold.require(self.site.media.root_for_read())?;
        if full{self.file.members.require(&WitnessRead::at(self.file.hold.directory())?)?;}
        if let Some(v)=&self.file.body{v.require(self.file.hold.directory())?;}Ok(())
    }
    fn source(&self)->&InitialReader{self.site.source}
    fn finish(&self,children:&Used)->io::Result<()>{
        let src=self.source();src.at(|db,c|self.check_at(db,&c))?;
        self.physical(true)?;children.require(self.file.hold.directory())?;
        src.at(|db,c|self.check_at(db,&c))
    }
    fn range_evidence(&self,offset:u64,n:u64,mut out:impl Write)->io::Result<Used>{
        if offset.checked_add(n).is_none_or(|end|end>self.file.view.map_or(0,|r|r.record.logical())){return Err(invalid("range outside exact retained EOF"));}
        let src=self.source();let observed=Cell::new(None);
        let gate=||->io::Result<()>{
            let version=smb_buffer::data_version(src)?;
            if observed.get()!=Some(version){
                self.finish(&Used::default())?;
                if smb_buffer::data_version(src)?!=version{return Err(io::ErrorKind::WouldBlock.into());}
                observed.set(Some(version));
            }
            self.physical(false)
        };
        gate()?;
        let used=match (&self.file.body,self.file.view){
            (Some(body),Some(view))=>body.range_with(self.file.hold.directory(),offset,n,|needs|{
                let mut origins=Vec::new();
                origins.try_reserve_exact(needs.len()).map_err(|_|invalid("retained origin list exceeds available memory"))?;
                for batch in needs.chunks(64){
                    src.at(|db,c|{
                        self.check_at(db,&c)?;
                        for n in batch{origins.push(work_record::origin(db,&self.file.place.birth,view,*n)?);}Ok(())
                    })?;
                }
                Ok(origins)
            },&gate,out)?,
            (None,None)=>{out.flush()?;Used::default()},
            _=>return Err(invalid("retained view lost its original body")),
        };
        self.finish(&used)?;Ok(used)
    }
}
