//! One true SQLite -> ordinary FILE -> SQLite working revision. Not a
//! protocol login, not a confirmed formal version. All selected roots/IDs
//! come from current business membership, never a pathname from the client.
use super::{Catalog,Connection,InitialReader,Tree,check,sqlite3_step,file_create};
use super::super::{file_location::{Placed,Place},volume::{OrdinaryRoot,OrdinaryView},sqlite3_column_type,work_record::{self,Members,ReadRecord}};
use crate::ordinary::working_round::{End,Selected,Used};
use crate::{FileId,OperationId,VolumeId,display,fs::{HeldDir,invalid,directory_birth::DirectoryParent},hash::Sha256,identity::{AccountId,ShareId},
    ordinary::{portable_birth::{Birth,Installed},working_sql::{self,Incoming,Retained,Spec}}};
use std::{cell::Cell,fs::File,io::{self,Write}};

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(crate) struct WorkReceipt{pub(crate) file:FileId,pub(crate) save:OperationId,pub(crate) revision:u64,
    pub(crate) bytes:u64,pub(crate) sha256:[u8;32]}
impl From<Spec> for WorkReceipt{fn from(x:Spec)->Self{Self{file:x.file,save:x.op,revision:1,bytes:x.bytes,sha256:x.content_sha}}}
#[derive(Clone,Copy,Eq,PartialEq)]
struct Row{done:bool,actor:AccountId,auth:OperationId,generation:i64,share:ShareId,mode:OperationId,spec:Spec,attempt:i64,previous:Option<OperationId>,mask:u8}
/// Only the live writer below, after the reservation has actually committed,
/// can grant its two exact role pairs to the native writer.
pub(crate) struct Reserved<'a>{installed:&'a Installed,spec:Spec,kept:&'a Retained}
impl Reserved<'_>{pub(crate) fn retained(&self)->&Retained{self.kept}
    pub(crate) fn parts(&self)->(&Installed,Spec){(self.installed,self.spec)}}

fn request(b:&Birth,x:Row)->io::Result<[u8;32]>{
    if x.generation<1||b.pool!=x.spec.pool||b.file!=x.spec.file{return Err(invalid("working bytes do not belong to this FILE"));}
    let mut h=Sha256::new();h.update(b"ZXS-SQLITE-1/FIRST-WORK\0")?;
    for id in [b.pool.0,x.spec.op.0,x.actor.0,x.auth.0]{h.update(&id)?;}h.update(&x.generation.to_be_bytes())?;
    for id in [x.share.0,x.mode.0,b.volume.0,b.operation.0,b.entry.0,b.file.0,b.history.0,b.branch.0]{h.update(&id)?;}
    h.update(&0u64.to_be_bytes())?; // exact unborn business revision; all attempts here are manual
    h.update(&x.attempt.to_be_bytes())?;h.update(&[u8::from(x.previous.is_some())])?;
    if let Some(prior)=x.previous{h.update(&prior.0)?;}h.update(&[x.mask])?;
    h.update(&x.spec.bytes.to_be_bytes())?;h.update(&x.spec.content_sha)?;h.update(&x.spec.directory_sha)?;Ok(h.finish())
}
fn original(db:&Connection,b:&Birth,op:OperationId)->io::Result<Option<Row>>{
    let s=db.prepare(c"SELECT o.kind,o.state,o.actor_id,o.request_sha,w.file_id,w.share_id,w.setting_op,w.auth_op,w.logical_bytes,w.content_sha,w.directory_sha,r.volume_id,m.share_id,m.mode,a.smb_id,k.credential_op,w.attempt_no,w.previous_save,w.retained_mask,k.generation FROM main.operation o LEFT JOIN main.first_work w ON w.save_op=o.op_id LEFT JOIN main.root_share r ON r.share_id=w.share_id LEFT JOIN main.share_mode m ON m.change_op=w.setting_op LEFT JOIN main.credential_epoch k ON k.role=1 AND k.credential_op=w.auth_op LEFT JOIN main.account_pair a ON a.singleton=k.slot WHERE o.op_id=?1")?;
    s.bind_bytes(1,&op.0)?;
    match unsafe{sqlite3_step(s.0)}{101=>return Ok(None),100=>{},rc=>{check(rc)?;return Err(invalid("cannot select working original"));}}
    if s.integer(0)?!=6||s.array::<16>(4)?!=b.file.0{return Err(invalid("operation does not belong to this working FILE"));}
    let done=match s.byte(1)?{0=>false,1=>true,_=>return Err(invalid("unknown working state"))};
    let len=u64::try_from(s.integer(8)?).map_err(|_|invalid("negative working extent"))?;
    let x=Row{done,actor:AccountId(s.array(2)?),auth:OperationId(s.array(7)?),generation:s.integer(19)?,share:ShareId(s.array(5)?),
        mode:OperationId(s.array(6)?),attempt:s.integer(16)?,previous:if unsafe{sqlite3_column_type(s.0,17)}==5{None}else{Some(OperationId(s.array(17)?))},mask:s.byte(18)?,spec:Spec::checked(b.pool,b.file,op,len,s.array(9)?,s.array(10)?)?};
    if x.attempt<1||x.mask>15||(x.attempt==1)!=(x.previous.is_none())||(x.previous.is_none()&&x.mask!=0)||s.array::<16>(11)?!=b.volume.0||s.array::<16>(12)?!=x.share.0||s.byte(13)?!=2||
        s.array::<16>(14)?!=x.actor.0||s.array::<16>(15)?!=x.auth.0||request(b,x)?!=s.array::<32>(3)?{
        return Err(invalid("working original does not match its exact birth, writer and input"));}
    s.done()?;Ok(Some(x))
}
pub(in crate::system::reader) fn confirmed_record(db:&Connection,b:&Birth,op:OperationId)->io::Result<crate::ordinary::working_round::Record>{
    let first=original(db,b,op)?.filter(|r|r.done).ok_or_else(||invalid("no original confirmed initial working material"))?;
    crate::ordinary::working_round::Record::first(first.spec)
}
fn by_file(db:&Connection,b:&Birth)->io::Result<Option<Row>>{
    let s=db.prepare(c"SELECT save_op FROM main.first_work WHERE file_id=?1 ORDER BY attempt_no DESC LIMIT 1")?;s.bind_bytes(1,&b.file.0)?;
    match unsafe{sqlite3_step(s.0)}{101=>return Ok(None),100=>{},rc=>{check(rc)?;return Err(invalid("cannot select FILE working head"));}}
    let op=OperationId(s.array(0)?);s.done()?;
    Ok(Some(original(db,b,op)?.ok_or_else(||invalid("missing issued working operation"))?))
}
fn retained(db:&Connection,b:&Birth)->io::Result<Retained>{
    // Each row here preserves at least one charged name. Repeated interruptions
    // before any local bytes create no rows in this index.
    let s=db.prepare(c"SELECT n.previous_save,n.retained_mask,p.file_id,o.state FROM main.first_work n JOIN main.first_work p ON p.save_op=n.previous_save JOIN main.operation o ON o.op_id=p.save_op WHERE n.file_id=?1 AND n.retained_mask<>0")?;
    s.bind_bytes(1,&b.file.0)?;let mut names=Retained::new();
    loop{match unsafe{sqlite3_step(s.0)}{101=>return Ok(names),100=>{
        if s.array::<16>(2)?!=b.file.0||s.byte(3)?!=0{return Err(invalid("retained names do not belong to an unconfirmed attempt of this FILE"));}
        working_sql::include_retained(&mut names,OperationId(s.array(0)?),s.byte(1)?)?;
    },rc=>{check(rc)?;return Err(invalid("cannot read retained working names"));}}}
}
pub(in crate::system::reader) fn membership(db:&Connection,c:&Catalog,b:&Birth)->io::Result<(Option<(Spec,bool)>,Retained)>{
    let Some(row)=by_file(db,b)? else{return Ok((None,Retained::new()));};
    if Place::current(db,c,b.file)?.backing.is_some(){
        return Ok((row.done.then_some((row.spec,true)),Retained::new()));
    }
    Ok((Some((row.spec,row.done)),retained(db,b)?))
}
fn replaceable(db:&Connection,c:&Catalog,x:Row)->io::Result<bool>{
    Ok(!x.done&&(super::share_mode::current(db,c,x.share)?.op!=x.mode||super::Credential::current(db,c)?.op!=x.auth))
}

#[derive(Clone,Copy,PartialEq,Eq)]
pub(in crate::system::reader) enum Cut{Reserved,BytesSynced}
impl Tree<'_,'_>{
    fn writer_row(&self,spec:Spec)->Row{
        Row{done:false,actor:self.session.original.account,auth:self.session.original.op,generation:self.session.original.generation,
            share:self.setting.share,mode:self.setting.op,spec,attempt:1,previous:None,mask:0}
    }
    fn match_input(&self,seen:Row,spec:Spec)->io::Result<Row>{
        if (Row{done:seen.done,attempt:seen.attempt,previous:seen.previous,mask:seen.mask,..self.writer_row(spec)})!=seen{return Err(invalid("same SAVE operation differs in input, FILE or authority"));}
        Ok(seen)
    }
    fn current_birth(&self,db:&Connection,c:&Catalog,b:&Birth,write:bool)->io::Result<()>{
        self.require(db,c,write)?;
        if self.birth(db,c,&b.name,b.file)?!=*b{return Err(invalid("FILE changed since selection"));}Ok(())
    }
    /// A secret-free original result, not a current FILE health check or a
    /// way to reissue from a new share/grant. No retransmission is required.
    pub(crate) fn first_save_receipt(&self,op:OperationId,name:&str,id:FileId)->io::Result<Option<WorkReceipt>>{
        let src=self.session.source;src.at(|db,c|{
            self.require(db,&c,true)?;let b=self.birth(db,&c,name,id)?;
            original(db,&b,op)?.map(|x|{
                if self.match_input(x,x.spec)?.done{Ok(Some(x.spec.into()))}else{Ok(None)}
            }).transpose().map(Option::flatten)
        })
    }
    pub(crate) fn save_first(&self,op:OperationId,name:&str,id:FileId,sealed_input:&File)->io::Result<WorkReceipt>{
        self.save_first_after(op,name,id,sealed_input,|_|Ok(()))
    }
    pub(in crate::system::reader) fn save_first_after(&self,op:OperationId,name:&str,id:FileId,input:&File,
        mut point:impl FnMut(Cut)->io::Result<()>)->io::Result<WorkReceipt>{
        display::component(name.as_bytes(),222)?;if op.0==[0;16]{return Err(invalid("missing working operation"));}
        let src=self.session.source;
        let(c,b,seen,kept,route)=src.at(|db,c|{
            self.require(db,&c,true)?;let b=self.birth(db,&c,name,id)?;let existing=by_file(db,&b)?;
            if let Some(row)=existing{if row.spec.op!=op&&!replaceable(db,&c,row)?{
                return Err(invalid("another live or committed first working attempt owns this FILE"));}}
            if let Some(same)=original(db,&b,op)?{if Some(same)!=existing{return Err(invalid("this exact SAVE has already been superseded"));}}
            let kept=retained(db,&b)?;let route=super::ordinary_directory::Route::at(db,&c,b.volume,self.parent)?;
            Ok((c,b,existing,kept,route))
        })?;
        // Input hashing holds neither SQL nor a shared volume/FILE namespace lock.
        let incoming=Incoming::observe(b.pool,b.file,op,input)?;let spec=incoming.spec();
        if let Some(x)=seen.filter(|x|x.spec.op==op){if self.match_input(x,spec)?.done{
            return self.first_save_receipt(op,name,id)?.ok_or_else(||invalid("lost committed working original"));}}
        let f=OrdinaryRoot::at_parent(src,&c,&route,None)?;
        let sample=f.folder().names()?.into_iter().collect();
        let held=b.working_at(f.folder(),&sample,true,seen.map(|x|(x.spec,x.done)),&kept)?;
        if seen.is_none_or(|x|x.spec.op!=op){
            // Count the actual retained name slots, but never read or modify
            // those files. No published working revision is being replaced.
            if f.space()?.available_bytes<spec.bytes.checked_add(spec.table_len()?).ok_or_else(||invalid("working capacity overflow"))?{
                return Err(io::Error::new(io::ErrorKind::StorageFull,"insufficient space for the first working material and its checks"));}
            let w=Connection::open(&src.dir,true)?;
            let attempt=src.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
                self.current_birth(db,&c,&b,true)?;
                let latest=by_file(db,&b)?;
                if let Some(old)=original(db,&b,op)?{
                    if latest!=Some(old){return Err(invalid("this SAVE is no longer the selected attempt"));}
                    return self.match_input(old,spec).map(|_|());}
                if latest!=seen{return Err(invalid("different first working reservation"));}
                file_create::display_free(db,&c,f.folder(),&b,self.parent)?;held.require(f.folder())?;f.require()?;
                let mut declared=retained(db,&b)?;let mut proposed=self.writer_row(spec);
                let scope=crate::fs::WitnessRead::at(held.directory())?;
                if let Some(old)=latest{
                    if !replaceable(db,&c,old)?{return Err(invalid("cannot replace a live or committed working operation"));}
                    old.spec.members(&scope,false,&declared)?;
                    proposed.previous=Some(old.spec.op);proposed.attempt=old.attempt.checked_add(1).ok_or_else(||invalid("working attempts exhausted"))?;
                    proposed.mask=scope.present_work_roles(&old.spec.names()?);
                    working_sql::include_retained(&mut declared,old.spec.op,proposed.mask)?;
                }else{scope.only_header()?;}
                working_sql::room(&declared,spec)?;
                let s=db.prepare(c"INSERT INTO main.first_work(save_op,file_id,share_id,setting_op,auth_op,logical_bytes,content_sha,directory_sha,attempt_no,previous_save,retained_mask) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)")?;
                for(i,id)in [op.0,b.file.0,proposed.share.0,proposed.mode.0,proposed.auth.0].iter().enumerate(){s.bind_bytes(i as i32+1,id)?;}
                s.bind_integer(6,spec.bytes as i64)?;s.bind_bytes(7,&spec.content_sha)?;s.bind_bytes(8,&spec.directory_sha)?;
                s.bind_integer(9,proposed.attempt)?;if let Some(old)=proposed.previous{s.bind_bytes(10,&old.0)?;} // unbound = SQL NULL
                s.bind_integer(11,i64::from(proposed.mask))?;s.done()?;drop(s);
                let o=db.prepare(c"INSERT INTO main.operation(op_id,kind,actor_id,request_sha,state) VALUES(?1,6,?2,?3,0)")?;
                o.bind_bytes(1,&op.0)?;o.bind_bytes(2,&proposed.actor.0)?;o.bind_bytes(3,&request(&b,proposed)?)?;o.done()
            });
            drop(w);
            if let Err(e)=attempt{match src.at(|db,c|{self.current_birth(db,&c,&b,true)?;original(db,&b,op)}){
                Ok(Some(x))=>{self.match_input(x,spec)?;},Ok(None)=>return Err(e),
                Err(_)=>return Err(invalid("SAVE reservation outcome unknown; query the exact operation")),}}
        }
        point(Cut::Reserved)?;
        let(x,kept)=src.at(|db,c|{
            self.current_birth(db,&c,&b,true)?;file_create::display_free(db,&c,f.folder(),&b,self.parent)?;
            let x=by_file(db,&b)?.ok_or_else(||invalid("missing current SAVE reservation"))?;
            Ok((self.match_input(x,spec)?,retained(db,&b)?))
        })?;
        if x.done{return Ok(spec.into());}
        f.require()?;held.require(f.folder())?;incoming.unchanged()?;
        let proof=incoming.prepare(&Reserved{installed:&held,spec,kept:&kept})?;point(Cut::BytesSynced)?;
        let w=Connection::open(&src.dir,true)?;
        let committed=src.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
            self.current_birth(db,&c,&b,true)?;file_create::display_free(db,&c,f.folder(),&b,self.parent)?;
            let x=self.match_input(by_file(db,&b)?.ok_or_else(||invalid("no current first working original"))?,spec)?;
            if !x.done{
                let s=db.prepare(c"UPDATE main.operation SET state=1 WHERE op_id=?1 AND kind=6 AND state=0 RETURNING op_id")?;
                s.bind_bytes(1,&op.0)?;match unsafe{sqlite3_step(s.0)}{100 if s.array::<16>(0)?==op.0=>s.done()?,rc=>{check(rc)?;return Err(invalid("SAVE did not confirm its one pending row"));}}
            }
            spec.members(&crate::fs::WitnessRead::at(held.directory())?,true,&kept)?;
            f.require()?;held.require(f.folder())?;proof.require(held.directory())?;Ok(WorkReceipt::from(spec))
        });
        drop(w);
        match committed{Ok(x)=>Ok(x),Err(e)=>match self.first_save_receipt(op,name,id){Ok(Some(x))if x==WorkReceipt::from(spec)=>Ok(x),
            Ok(_)=>Err(e),Err(_)=>Err(invalid("SAVE confirmation unknown or unauthorized; do not retry under a new operation"))}}
    }

}

/// A current working handle opens by exact business name, not by a guessed
/// FILE ID. Its read authority never becomes a write/other-share capability.
/// Drop raw/table and FILE before its registered parent and service lease.
enum MemberRoot<'t,'r>{Owned(OrdinaryRoot<'r>),Tree(&'t OrdinaryRoot<'r>)}
impl<'r> std::ops::Deref for MemberRoot<'_,'r>{
    type Target=OrdinaryRoot<'r>;
    fn deref(&self)->&Self::Target{match self{Self::Owned(r)=>r,Self::Tree(r)=>r}}
}
pub(crate) struct Current<'t,'s,'r>{original:ReadFile,root:MemberRoot<'t,'r>,tree:&'t Tree<'s,'r>}

/// The one physical read view and its exact logical placement. Credentials,
/// protocol routes and the reader's current short authorization cut are
/// separate. The body and container still retire before the physical parent.
struct ReadFile{body:Option<Selected>,container:Installed,birth:Birth,place:Place,live:Option<ReadRecord>}
impl ReadFile{
    fn open(placed:Placed,selected:&ReadCut,folder:&HeldDir)->io::Result<Self>{
        let sample=folder.names()?.into_iter().collect();
        let container=placed.open(folder,&sample,false,|scope|selected.members.require(scope))?;
        let live=selected.selected.map(|(selected,_)|selected);
        let body=selected.selected.map(|(r,physical)|Selected::open(container.directory(),r.record,physical)).transpose()?;
        let Placed{birth,here:place}=placed;
        Ok(Self{body,container,birth,place,live})
    }
    fn size(&self)->u64{self.live.map_or(0,|r|r.record.logical())}
}
/// This is a short use of the actual authority and held parent, not a
/// constructor for an authenticated session or for a valid file path.
struct ReadAt<'a>{
    file:&'a ReadFile,root:OrdinaryView<'a>,source:&'a InitialReader,
    volume:VolumeId,parent:DirectoryParent,require:&'a dyn Fn(&Connection,&Catalog)->io::Result<()>,
}
#[derive(Eq,PartialEq)]
struct ReadCut{selected:Option<(ReadRecord,End)>,members:Members,alias:Option<file_create::Alias>}
impl ReadCut{
    fn gather(db:&Connection,c:&Catalog,b:&Birth,at:&Place,chosen:Option<ReadRecord>)->io::Result<Self>{
        let selected=chosen.map(|r|work_record::ceiling(db,b,r).map(|end|(r,end))).transpose()?;
        Ok(Self{selected,members:work_record::members(db,c,b)?,alias:file_create::display_owner_at(db,c,at.volume,at.parent,&at.name)?})
    }
}
impl<'s,'r> Tree<'s,'r>{
    pub(super) fn reader_from_newborn<'t>(&'t self,born:super::file_create::NativeBorn<'r>)->io::Result<Current<'t,'s,'r>>{
        let (root,birth,container)=born.parts(false)?;
        let place=Place::origin(&birth,root.parent());
        let original=ReadFile{body:None,container,birth,place,live:None};
        let handle=Current{original,root:MemberRoot::Owned(root),tree:self};
        handle.require()?;Ok(handle)
    }
    pub(crate) fn open_current<'t>(&'t self,name:&str)->io::Result<Current<'t,'s,'r>>{
        self.open_member(name,None,None,None,|_,_|Ok(())).map(|(h,_)|h)
    }
    pub(super) fn open_current_root<'t>(&'t self,root:&'t OrdinaryRoot<'r>,name:&str)
        ->io::Result<(Current<'t,'s,'r>,super::file_number::NativeKey)>{
        self.open_member(name,None,Some(root),None,|db,b|super::file_number::original(db,b.pool,b.file))
    }
    /// Reopen the member, not the revision cached during enumeration.
    pub(crate) fn open_listed<'t>(&'t self,entry:&super::directory::Entry)->io::Result<Current<'t,'s,'r>>{
        self.open_member(&entry.name,Some(entry),None,None,|_,_|Ok(())).map(|(h,_)|h)
    }
    pub(crate) fn open_version<'t>(&'t self,name:&str,version:crate::NodeId)->io::Result<Current<'t,'s,'r>>{
        self.open_member(name,None,None,Some(version),|_,_|Ok(())).map(|(h,_)|h)
    }
    fn open_member<'t,V>(&'t self,name:&str,expected:Option<&super::directory::Entry>,pinned:Option<&'t OrdinaryRoot<'r>>,version:Option<crate::NodeId>,
        result:impl FnOnce(&Connection,&Birth)->io::Result<V>)->io::Result<(Current<'t,'s,'r>,V)>{
        display::component(name.as_bytes(),222)?;let src=self.session.source;
        if pinned.is_some_and(|r|!r.for_tree(src,self.setting.volume)||r.parent()!=self.parent){return Err(invalid("different native tree root"));}
        let(c,placed,selected,route)=src.at(|db,c|{
            self.require(db,&c,false)?;
            let placed=self.placed(db,&c,name,None)?;
            if expected.is_some_and(|e|!e.matches(&placed)){return Err(invalid("listed FILE no longer names this selected placement"));}
            let b=&placed.birth;
            let chosen=match version{Some(n)=>Some(work_record::version(db,b,n)?),None=>work_record::selected(db,b)?};
            let selected=ReadCut::gather(db,&c,b,&placed.here,chosen)?;
            let route=if pinned.is_none(){Some(super::ordinary_directory::Route::at(db,&c,placed.here.volume,placed.here.parent)?)}else{None};
            Ok((c,placed,selected,route))
        })?;
        let root=match pinned{Some(r)=>MemberRoot::Tree(r),None=>MemberRoot::Owned(OrdinaryRoot::at_parent(src,&c,&route.ok_or_else(||invalid("lost selected FILE parent"))?,None)?)};
        let original=ReadFile::open(placed,&selected,root.folder())?;
        let h=Current{original,root,tree:self};
        let v=h.with_read(|reader|reader.checked(&selected,||Ok(()),|db|result(db,&h.original.birth)))?;
        Ok((h,v))
    }
}
impl ReadAt<'_>{
    fn still_selected(&self,db:&Connection,c:&Catalog)->io::Result<()>{
        (self.require)(db,c)?;
        let p=file_create::placed_at(db,c,self.volume,self.parent,&self.file.place.name,Some(self.file.birth.file))?;
        if p.here!=self.file.place||p.birth!=self.file.birth{return Err(invalid("FILE reader lost its original placement"));}
        if let Some(old)=self.file.live{
            if work_record::confirmed(db,&self.file.birth,old.record.key().save)?!=old{
                return Err(invalid("the held original work revision changed"));
            }
        }
        Ok(())
    }
    fn selected(&self,db:&Connection,c:&Catalog)->io::Result<ReadCut>{
        self.still_selected(db,c)?;ReadCut::gather(db,c,&self.file.birth,&self.file.place,self.file.live)
    }
    /// Native I/O (including a slow/missing F) is bracketed by short,
    /// current SQL selections. No claim that separate disks are atomic.
    fn checked<T>(&self,x:&ReadCut,at_f:impl FnOnce()->io::Result<()>,finish:impl FnOnce(&Connection)->io::Result<T>)->io::Result<T>{
        let alias=super::directory::check_alias_at(self.root.folder(),&self.file.place.name,x.alias.as_ref(),None)?;
        x.members.require(&crate::fs::WitnessRead::at(self.file.container.directory())?)?;
        self.root.require_streaming_at(at_f)?;self.file.container.require(self.root.folder())?;
        if let Some(h)=&self.file.body{h.require(self.file.container.directory())?;}
        let result=self.source.at(|db,c|{
            if self.selected(db,&c)?!=*x{return Err(io::Error::new(io::ErrorKind::WouldBlock,"FILE selection changed during physical check"));}
            finish(db)
        });drop(alias);result
    }
    fn check_with<T>(&self,at_f:impl FnOnce()->io::Result<()>,finish:impl FnOnce(&Connection)->io::Result<T>)->io::Result<T>{
        let x=self.source.at(|db,c|self.selected(db,&c))?;self.checked(&x,at_f,finish)
    }
    fn finish_sources(&self,used:&Used)->io::Result<()>{
        // Native disk checks fit between the existing short SQL cuts.
        self.check_with(||used.require(self.file.container.directory()),|_|Ok(()))
    }
    fn range_evidence(&self,offset:u64,count:u64,mut out:impl Write)->io::Result<Used>{
        if offset.checked_add(count).is_none_or(|end|end>self.file.size()){return Err(invalid("range outside this confirmed working EOF"));}
        let seen=Cell::new(None);
        let gate=||->io::Result<()>{
            let before=super::smb_buffer::data_version(self.source)?;
            if seen.get()!=Some(before){
                self.check_with(||Ok(()),|_|Ok(()))?;
                if super::smb_buffer::data_version(self.source)?!=before{return Err(io::Error::new(io::ErrorKind::WouldBlock,"changed SQLite cut; retry this range"));}
                seen.set(Some(before));
            }
            self.root.require_streaming()?;self.file.container.require(self.root.folder())?;
            if let Some(h)=&self.file.body{h.require(self.file.container.directory())?;}Ok(())
        };
        gate()?;
        let used=if let (Some(held),Some(cut))=(&self.file.body,self.file.live){
            held.range_with(self.file.container.directory(),offset,count,|need|{
                let mut result=Vec::new();result.try_reserve_exact(need.len()).map_err(|_|invalid("work origin request exceeds memory"))?;
                for batch in need.chunks(64){
                    self.source.at(|db,c|{
                        self.still_selected(db,&c)?;
                        for n in batch{result.push(work_record::origin(db,&self.file.birth,cut,*n)?);}Ok(())
                    })?;
                }
                Ok(result)
            },&gate,out)?
        }else{out.flush()?;Used::default()};
        self.finish_sources(&used)?;Ok(used)
    }
}

impl Current<'_,'_,'_>{
    fn with_read<T>(&self,use_original:impl FnOnce(ReadAt<'_>)->T)->T{
        let require=|db:&Connection,c:&Catalog|self.tree.require(db,c,false);
        use_original(ReadAt{file:&self.original,root:self.root.view(),source:self.tree.session.source,
            volume:self.tree.setting.volume,parent:self.tree.parent,require:&require})
    }
    pub(crate) fn file(&self)->FileId{self.original.birth.file}
    pub(super) fn native_placement(&self)->&Place{&self.original.place}
    pub(super) fn rename_native(&mut self,op:OperationId,name:&str,alive:&impl Fn()->io::Result<()>,
        point:impl Fn(super::file_rename::Cut)->io::Result<()>)->io::Result<()>{
        self.require()?;
        let (selected,body)=self.tree.rename_existing(&self.root,&self.original.place,&self.original.container,
            op,name,alive,point)?;
        self.original.container.rebind_renamed(self.root.folder(),&body).map_err(|_|super::file_rename::unknown())?;
        self.original.place=selected;
        drop(body); // no FILE write lock escapes to the original read handle
        self.require().and_then(|_|alive()).map_err(|_|super::file_rename::unknown())
    }
    pub(crate) fn size(&self)->u64{self.original.size()}
    pub(crate) fn working_original(&self)->Option<OperationId>{self.original.live.map(|r|r.record.key().save)}
    pub(crate) fn working_revision(&self)->u64{self.original.live.map(|r|r.record.key().revision)}
    pub(super) fn original_ids(&self)->(crate::PoolId,crate::VolumeId,crate::EntryId,FileId){
        let file=&self.original;
        (file.birth.pool,file.place.volume,file.birth.entry,file.birth.file)
    }
    pub(crate) fn native_key(&self)->io::Result<super::file_number::NativeKey>{
        self.with_read(|reader|reader.check_with(||Ok(()),|db|super::file_number::original(db,self.original.birth.pool,self.file())))
    }
    #[cfg(test)]
    pub(in crate::system::reader) fn check_at_file(&self,cut:impl FnOnce()->io::Result<()>)->io::Result<()>{
        self.with_read(|reader|reader.check_with(cut,|_|Ok(())))
    }
    fn require(&self)->io::Result<()>{
        self.with_read(|reader|reader.check_with(||Ok(()),|_|Ok(())))
    }
    /// Buffer the exact original native reply. The retained reader shares
    /// this delivery envelope but never gains FILE write or F authority.
    pub(crate) fn smb_read_window<T>(&self,offset:u64,count:u32,consume:impl FnOnce(&[u8])->io::Result<T>)->io::Result<T>{
        self.with_read(|reader|super::smb_buffer::deliver(offset,count,self.size(),
            |at,n,b|reader.range_evidence(at,n,b),consume,|x|reader.finish_sources(x)))
    }
    pub(crate) fn range_to(&self,offset:u64,count:u64,out:impl Write)->io::Result<()>{
        self.range_evidence(offset,count,out).map(|_|())
    }
    pub(super) fn finish_sources(&self,used:&Used)->io::Result<()>{
        self.with_read(|reader|reader.finish_sources(used))
    }
    pub(super) fn range_evidence(&self,offset:u64,count:u64,out:impl Write)->io::Result<Used>{
        self.with_read(|reader|reader.range_evidence(offset,count,out))
    }
}
