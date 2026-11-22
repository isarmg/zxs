//! Same-parent ordinary rename issues one durable ticket for the actual
//! existing FILE. No transfer, new content, replacement, or fallback path.
use super::{Catalog,Connection,InitialReader,Tree,denied,directory,file_create,ordinary_directory::{self,Additional}};
use super::super::{account::share_mode::Setting,file_location::Place,volume::{OrdinaryRoot,OrdinaryView},work_record};
use crate::{FileId,OperationId,Role,VolumeId,display,
    fs::{HeldDir,invalid,directory_birth::DirectoryParent}, hash::Sha256,
    identity::{AccountId,ShareId},ordinary::portable_birth::{Birth,Installed,RenameInstall,RenameStage}};
use std::{collections::BTreeSet,io,os::unix::ffi::OsStrExt,thread,time::Duration};

#[derive(Clone,Copy,Eq,PartialEq)]
pub(super) enum Cut{ParentSampled,Reserved,NameChanged,Confirmed}
#[derive(Clone,Debug,Eq,PartialEq)]
struct Original{
    op:OperationId,from:Place,to_parent:DirectoryParent,name:String,share:ShareId,setting:OperationId,
    auth:OperationId,actor:AccountId,epoch:i64,save:Option<OperationId>,manual:Option<OperationId>,
}
#[derive(Clone,Copy,Eq,PartialEq)]
enum Status{Pending,Confirmed,Aborted}
fn busy()->io::Error{io::Error::new(io::ErrorKind::WouldBlock,"another original change owns the FILE or name")}
pub(super) fn unknown()->io::Error{io::Error::new(io::ErrorKind::ConnectionAborted,"rename outcome is not confirmed on this handle")}

impl Original{
    fn target(&self)->Place{
        Place{op:self.op,generation:self.from.generation+1,parent:self.to_parent,name:self.name.clone(),..self.from.clone()}
    }
    fn digest(&self,b:&Birth)->io::Result<[u8;32]>{
        if b.file!=self.from.file || self.from.role!=Role::File || self.from.generation<1 ||
           self.from.generation>=i64::MAX-1 || self.epoch<=0{
            return Err(invalid("rename has no original authorized placement"));
        }
        if self.op==self.from.op||(self.name==self.from.name&&self.from.parent==self.to_parent)||[self.op.0,self.from.op.0,self.from.volume.0,
            self.share.0,self.setting.0,self.auth.0,self.actor.0].contains(&[0;16]){
            return Err(invalid("rename has an invalid original selector"));
        }
        let mut h=Sha256::new();h.update(b"ZXS-SQLITE-1/ORDINARY-RENAME\0")?;
        for id in [b.pool.0,b.operation.0,b.file.0,b.entry.0,b.history.0,b.branch.0,self.op.0,self.from.op.0,
            self.from.volume.0,self.from.parent.bytes(),self.to_parent.bytes(),self.share.0,self.setting.0,self.auth.0,self.actor.0]{
            h.update(&id)?;
        }
        h.update(&self.from.generation.to_be_bytes())?;h.update(&self.epoch.to_be_bytes())?;
        for name in [&self.from.name,&self.name]{
            let raw=display::component(name.as_bytes(),222)?.as_bytes();
            h.update(&[raw.len() as u8])?;h.update(raw)?;
        }
        for id in [self.from.backing,self.save,self.manual]{
            h.update(&[u8::from(id.is_some())])?;if let Some(id)=id{h.update(&id.0)?;}
        }
        Ok(h.finish())
    }
    fn issue(&self,db:&Connection,birth:&Birth)->io::Result<()>{
        let o=db.prepare(c"INSERT INTO main.file_rename(rename_op,file_id,source_op,source_gen,volume_id,parent_id,
            source_name,target_name,backing_op,source_terminal,target_terminal,share_id,setting_op,auth_op,save_op,manual_op,target_parent)
            VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)")?;
        let f=&self.from;
        for (i,value) in [(1,self.op.0),(2,f.file.0),(3,f.op.0),(5,f.volume.0),(6,f.parent.bytes()),
            (12,self.share.0),(13,self.setting.0),(14,self.auth.0),(17,self.to_parent.bytes())]{o.bind_bytes(i,&value)?;}
        o.bind_integer(4,f.generation)?;o.bind_bytes(7,f.name.as_bytes())?;o.bind_bytes(8,self.name.as_bytes())?;
        if let Some(id)=f.backing{o.bind_bytes(9,&id.0)?;}else{
            o.bind_bytes(10,f.leaf()?.to_bytes())?;o.bind_bytes(11,self.target().leaf()?.to_bytes())?;
        }
        if let Some(id)=self.save{o.bind_bytes(15,&id.0)?;}if let Some(id)=self.manual{o.bind_bytes(16,&id.0)?;}
        o.done()?;drop(o);
        let command=db.prepare(c"INSERT INTO main.operation VALUES(?1,14,?2,?3,0)")?;
        command.bind_bytes(1,&self.op.0)?;command.bind_bytes(2,&self.actor.0)?;
        command.bind_bytes(3,&self.digest(birth)?)?;command.done()
    }
    fn fetch(db:&Connection,birth:&Birth,op:OperationId)->io::Result<Option<(Self,Status)>>{
        let mut q=db.prepare(c"SELECT o.state,o.actor_id,o.request_sha,r.file_id,r.source_op,r.source_gen,r.volume_id,
          r.parent_id,r.source_name,r.target_name,r.backing_op,r.source_terminal,r.target_terminal,
          r.share_id,r.setting_op,r.auth_op,r.save_op,r.manual_op,e.generation,
          EXISTS(SELECT 1 FROM main.file_rename_abort a WHERE a.rename_op=o.op_id),r.target_parent
          FROM main.file_rename r JOIN main.operation o ON o.op_id=r.rename_op AND o.kind=14
          JOIN main.credential_epoch e ON e.role=1 AND e.credential_op=r.auth_op
          JOIN main.account_pair p ON p.singleton=e.slot AND p.smb_id=o.actor_id
          JOIN main.root_share s ON s.share_id=r.share_id AND s.volume_id=r.volume_id
          JOIN main.share_mode m ON m.share_id=r.share_id AND m.change_op=r.setting_op AND m.mode=2
          WHERE r.rename_op=?1")?;
        q.bind_bytes(1,&op.0)?;
        if !work_record::start_one(&q)?{return Ok(None);}
        let status=match (q.byte(0)?,q.byte(19)?){
            (0,0)=>Status::Pending,(1,0)=>Status::Confirmed,(0,1)=>Status::Aborted,
            _=>return Err(invalid("impossible original rename state")),
        };
        let file=FileId(q.array(3)?);
        let row=q.row()?;
        let name=display::component(row.blob(9,222)?,222)?.to_owned();
        let old=display::component(row.blob(8,222)?,222)?.to_owned();
        let backing=Place::read_backing(&q,10,11,file,&old)?;
        if Place::read_backing(&q,10,12,file,&name)?!=backing{return Err(invalid("rename changed physical backing"));}
        let from=Place{file,op:OperationId(q.array(4)?),generation:q.integer(5)?,volume:VolumeId(q.array(6)?),
            parent:DirectoryParent::from_bytes(q.array(7)?),name:old,role:Role::File,backing};
        let row=Self{op,from,to_parent:DirectoryParent::from_bytes(q.array(20)?),name,share:ShareId(q.array(13)?),setting:OperationId(q.array(14)?),
            auth:OperationId(q.array(15)?),actor:AccountId(q.array(1)?),epoch:q.integer(18)?,
            save:work_record::optional(&q,16)?,manual:work_record::optional(&q,17)?};
        if q.array::<32>(2)?!=row.digest(birth)?{return Err(invalid("rename receipt differs from its original request"));}
        q.done()?;Ok(Some((row,status)))
    }
    fn require_exact(&self,db:&Connection,birth:&Birth,status:Status)->io::Result<()>{
        if Self::fetch(db,birth,self.op)?.as_ref()!=Some(&(self.clone(),status)){return Err(denied());}
        Ok(())
    }
    fn live(&self,db:&Connection)->io::Result<()>{
        let q=db.prepare(c"SELECT actor_id FROM main.live_rename WHERE rename_op=?1")?;
        q.bind_bytes(1,&self.op.0)?;
        if !work_record::start_one(&q)?||q.array::<16>(0)?!=self.actor.0{return Err(denied());}
        q.done()
    }
    fn confirm(&self,db:&Connection)->io::Result<()>{
        let q=db.prepare(c"UPDATE main.operation SET state=1 WHERE op_id=?1 AND kind=14 AND state=0 RETURNING op_id")?;
        q.bind_bytes(1,&self.op.0)?;
        if !work_record::start_one(&q)?||q.array::<16>(0)?!=self.op.0{return Err(denied());}
        q.done()
    }
}

fn namespace_lock(original:&HeldDir)->io::Result<HeldDir>{
    let actual=HeldDir::trusted_storage(&original.path)?;actual.exclusive()?;
    if actual.entity!=original.entity||!actual.same_mount(original)?{return Err(invalid("ordinary rename parent changed"));}
    Ok(actual)
}
fn census(parent:&HeldDir)->io::Result<BTreeSet<Vec<u8>>>{Ok(parent.names()?.into_iter().collect())}
fn pocket_shape(private:bool,parent:&HeldDir,body:&HeldDir,b:&Birth)->io::Result<()>{
    if private{
        let leaf=body.path.file_name().ok_or_else(denied)?.as_bytes();
        if census(parent)?!=BTreeSet::from([leaf.to_vec(),b.witness()?.into_bytes()]){
            return Err(invalid("unknown member in the exact existing transfer pocket"));
        }
    }
    Ok(())
}

fn vacant(db:&Connection,c:&Catalog,from:&Place,into:DirectoryParent,title:&str)->io::Result<()>{
    if Place::renaming_name(db,from.volume,into,title)?{return Err(busy());}
    if Place::named(db,c,from.volume,into,title)?.is_some()||
       ordinary_directory::lookup(db,c,from.volume,into,title)?.is_some(){
        return Err(io::ErrorKind::AlreadyExists.into());
    }
    if from.backing.is_none(){
        let leaf=Birth::placed_container(title,from.file)?;
        let native=leaf.to_str().map_err(|_|denied())?;
        if Place::renaming_name(db,from.volume,into,native)?{return Err(busy());}
        if ordinary_directory::lookup(db,c,from.volume,into,native)?.is_some()||
           (native.len()<=222&&Place::named(db,c,from.volume,into,native)?.is_some()){
            return Err(io::ErrorKind::AlreadyExists.into());
        }
    }
    Ok(())
}

#[derive(Clone,Copy,Eq,PartialEq)]
enum Recovery{Continue,Undo{setting:OperationId,auth:OperationId}}
type Allowed<'a>=dyn Fn(&Connection,&Catalog,ShareId,VolumeId)->io::Result<()>+'a;
impl Original{
    fn recovery(&self,db:&Connection,c:&Catalog,birth:&Birth,allowed:&Allowed<'_>)->io::Result<Recovery>{
        self.require_exact(db,birth,Status::Pending)?;
        if Place::current(db,c,self.from.file)?!=self.from{return Err(denied());}
        allowed(db,c,self.share,self.from.volume)?;
        let q=db.prepare(c"SELECT setting_op,auth_op,actor_id FROM main.live_ordinary_writer
            WHERE share_id=?1 AND volume_id=?2")?;
        q.bind_bytes(1,&self.share.0)?;q.bind_bytes(2,&self.from.volume.0)?;
        if !work_record::start_one(&q)?||q.array::<16>(2)?!=self.actor.0{return Err(denied());}
        let setting=OperationId(q.array(0)?);let auth=OperationId(q.array(1)?);q.done()?;
        if (setting,auth)==(self.setting,self.auth){self.live(db)?;Ok(Recovery::Continue)}
        else{Ok(Recovery::Undo{setting,auth})}
    }
    fn abort(&self,db:&Connection,setting:OperationId,auth:OperationId,reason:Abort)->io::Result<()>{
        let q=db.prepare(c"INSERT INTO main.file_rename_abort(rename_op,setting_op,auth_op,reason) VALUES(?1,?2,?3,?4)")?;
        q.bind_bytes(1,&self.op.0)?;q.bind_bytes(2,&setting.0)?;q.bind_bytes(3,&auth.0)?;
        q.bind_integer(4,reason as i64)?;q.done()
    }
}

#[repr(u8)]
enum Abort{Untouched=1,RolledBack=2}

/// A cold recovery owns no native FILE session. It cannot start a change
/// to the new name. It may finish the originally changed name, release an
/// untouched reservation, or put back the same file under a new RW grant.
fn recover(src:&InitialReader,op:OperationId,file:FileId,allowed:&Allowed<'_>,
    alive:&impl Fn()->io::Result<()>)->io::Result<()>{
    let (catalog,original,kind,birth,route,target,members,old_alias,new_alias)=src.at(|db,c|{
        alive()?;
        let birth=file_create::committed_file(db,&c,file)?;
        let (r,state)=Original::fetch(db,&birth,op)?.ok_or_else(denied)?;
        if state!=Status::Pending||r.from.file!=file{return Err(denied());}
        let kind=r.recovery(db,&c,&birth,allowed)?;
        let route=ordinary_directory::Route::at(db,&c,r.from.volume,r.from.parent)?;
        let target=if r.to_parent!=r.from.parent{Some(ordinary_directory::Route::at(db,&c,r.from.volume,r.to_parent)?)}else{None};
        let members=work_record::members(db,&c,&birth)?;
        let old=file_create::display_owner_at(db,&c,r.from.volume,r.from.parent,&r.from.name)?;
        let new=file_create::display_owner_at(db,&c,r.from.volume,r.to_parent,&r.name)?;
        Ok((c,r,kind,birth,route,target,members,old,new))
    })?;
    let f=OrdinaryRoot::at_parent(src,&catalog,&route,None)?;
    if let Some(target)=target{
        let to=OrdinaryRoot::at_parent(src,&catalog,&target,None)?;
        return parent_change::recover(src,&original,kind,&birth,&f,&to,&route,&target,&members,
            old_alias.as_ref(),new_alias.as_ref(),allowed,alive);
    }
    let holder=if let Some(native)=original.from.private_leaf()?{
        Some(f.folder().exact_directory(&native)?.ok_or_else(denied)?)
    }else{None};
    let private=holder.is_some();let parent=holder.as_ref().unwrap_or(f.folder());
    let step=RenameInstall::hold(&birth,parent,&original.from.name,&original.name,RenameStage::Reserved,
        |w|members.require(w))?;
    pocket_shape(private,parent,step.current_directory(),&birth)?;
    let already_new=step.already_renamed();
    let select_new=already_new&&kind==Recovery::Continue;
    let (name,alias)=if select_new{(&original.name,new_alias)}else{(&original.from.name,old_alias)};
    let liveness=||{
        f.require_streaming()?;
        src.at(|db,c|{
            if original.recovery(db,&c,&birth,allowed)?!=kind{return Err(denied());}
            alive()
        })?;
        // The common reader still rechecks the original S after the SQL
        // closure. It may have waited. Do not enter rename after drain.
        alive()
    };
    let terminal=|found:&Installed|->io::Result<()>{
        let _outer=if private{Some(namespace_lock(f.folder())?)}else{None};
        f.require_streaming()?;pocket_shape(private,parent,found.directory(),&birth)?;
        let _alias=directory::check_alias_name(&f,name,alias.as_ref(),None)?;
        let w=Connection::open(&src.dir,true)?;
        src.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
            if original.recovery(db,&c,&birth,allowed)?!=kind||
               file_create::display_owner_at(db,&c,original.from.volume,original.from.parent,name)?!=alias{
                return Err(denied());
            }
            alive()?;
            let position=if select_new{
                original.confirm(db)?;original.target()
            }else{
                let (setting,auth)=match kind{
                    Recovery::Continue=>(original.setting,original.auth),
                    Recovery::Undo{setting,auth}=>(setting,auth),
                };
                original.abort(db,setting,auth,if already_new{Abort::RolledBack}else{Abort::Untouched})?;
                original.from.clone()
            };
            if Place::current(db,&c,file)?!=position{return Err(denied());}
            Ok(())
        })
    };
    liveness()?;
    if already_new&&!select_new{step.restore_old(&liveness,terminal)?;}
    else{step.finish_observed(terminal)?;}
    Ok(())
}

fn recovery_page(db:&Connection,after:OperationId)->io::Result<Vec<(OperationId,FileId)>>{
    let q=db.prepare(c"SELECT r.rename_op,r.file_id
        FROM main.operation o JOIN main.file_rename r ON r.rename_op=o.op_id
        WHERE o.kind=14 AND o.state=0 AND o.op_id>?1
          AND NOT EXISTS(SELECT 1 FROM main.file_rename_abort a WHERE a.rename_op=o.op_id)
        ORDER BY o.op_id LIMIT 16")?;
    q.bind_bytes(1,&after.0)?;
    let mut rows=Vec::with_capacity(16);
    while work_record::start_one(&q)?{rows.push((OperationId(q.array(0)?),FileId(q.array(1)?)));}
    Ok(rows)
}

pub(super) fn recovery_loop(src:&InitialReader,allowed:impl Fn(&Connection,&Catalog,ShareId,VolumeId)->io::Result<()>,
    alive:impl Fn()->io::Result<()>){
    let mut after=OperationId([0;16]);
    while alive().is_ok(){
        let page=src.at(|db,_|recovery_page(db,after));
        let pause=match page{
            Ok(rows) if !rows.is_empty()=>{
                for (op,file) in rows{
                    after=op;
                    if alive().is_err(){break;}
                    let _=recover(src,op,file,&allowed,&alive);
                }
                250
            },
            _=>{after=OperationId([0;16]);1000},
        };
        thread::sleep(Duration::from_millis(pause));
    }
}

#[cfg(test)]
#[test]
fn ordinary_rename_recovery_page_does_not_fill_with_aborted_history(){
    use super::super::{check,zxs_sql_template};
    let mut raw=std::ptr::null_mut();
    // SAFETY: fixed test SQL and an initialized output; no actual pool is opened.
    check(unsafe{zxs_sql_template(c"
        CREATE TABLE operation(op_id BLOB PRIMARY KEY,kind INTEGER,state INTEGER) STRICT, WITHOUT ROWID;
        CREATE TABLE file_rename(rename_op BLOB PRIMARY KEY,file_id BLOB) STRICT, WITHOUT ROWID;
        CREATE TABLE file_rename_abort(rename_op BLOB PRIMARY KEY) STRICT, WITHOUT ROWID;
    ".as_ptr(),&mut raw)}).unwrap();
    let db=Connection(raw);
    for i in 1..=20{
        let op=[i;16];let file=[i+64;16];
        let add=db.prepare(c"INSERT INTO operation VALUES (?1,?2,?3)").unwrap();
        add.bind_bytes(1,&op).unwrap();
        add.bind_integer(2,if i==19{16}else{14}).unwrap();
        add.bind_integer(3,i64::from(i==18)).unwrap();
        add.done().unwrap();
        let rename=db.prepare(c"INSERT INTO file_rename VALUES (?1,?2)").unwrap();
        rename.bind_bytes(1,&op).unwrap();
        rename.bind_bytes(2,&file).unwrap();
        rename.done().unwrap();
        if i<=16{
            let abort=db.prepare(c"INSERT INTO file_rename_abort VALUES (?1)").unwrap();
            abort.bind_bytes(1,&op).unwrap();
            abort.done().unwrap();
        }
    }
    let first=(OperationId([17;16]),FileId([81;16]));
    let second=(OperationId([20;16]),FileId([84;16]));
    assert_eq!(recovery_page(&db,OperationId([0;16])).unwrap(),[first,second]);
    assert_eq!(recovery_page(&db,first.0).unwrap(),[second]);
}

#[cfg(test)]
pub(super) fn recover_for_test(src:&InitialReader,op:OperationId,file:FileId,
    allowed:impl Fn(&Connection,&Catalog,ShareId,VolumeId)->io::Result<()>)->io::Result<()>{
    recover(src,op,file,&allowed,&||Ok(()))
}

impl Tree<'_,'_>{
    /// Only the exact already-open native member calls this. The writer's
    /// independent FILE exclusion is held from preflight through selection;
    /// the old reader stays owned by that caller, not replaced by a reopen.
    pub(super) fn rename_existing(&self,root:&OrdinaryRoot<'_>,expected:&Place,opened:&Installed,
        op:OperationId,name:&str,alive:&impl Fn()->io::Result<()>,
        point:impl Fn(Cut)->io::Result<()>)->io::Result<(Place,Installed)>{
        let source=self.session.source;let owner=&self.session.original;
        let require=|cut:&RenameCut<'_>|{
            debug_assert!(std::ptr::eq(cut.source,source));
            self.require(cut.db,cut.catalog,true)
        };
        RenameAt{source,setting:self.setting,parent:self.parent,
            actor:owner.account,auth:owner.op,epoch:owner.generation,require:&require}
            .rename(root.view(),expected,opened,op,name,alive,point)
            .map(|renamed|(renamed.place,renamed.body))
    }
}

/// All selection and confirmation cuts remain on one original SYSTEM.
/// This short helper does not grant a FILE, a name or a live SMB session.
pub(super) struct RenameCut<'a>{source:&'a InitialReader,db:&'a Connection,catalog:&'a Catalog}
impl<'a> RenameCut<'a>{
    pub(super) fn parts(&self)->(&'a InitialReader,&'a Connection,&'a Catalog){(self.source,self.db,self.catalog)}
}

pub(super) struct ExactRename(Original);
impl ExactRename{
    pub(super) fn require(&self,db:&Connection,birth:&Birth,place:&Place)->io::Result<()>{
        if self.0.target()!=*place{return Err(denied());}
        self.0.require_exact(db,birth,Status::Confirmed)
    }
}
pub(super) struct Renamed{place:Place,body:Installed,proof:ExactRename}
impl Renamed{
    pub(super) fn into_parts(self)->(Place,Installed,ExactRename){(self.place,self.body,self.proof)}
}
struct RenameAt<'s,'q>{
    source:&'s InitialReader,setting:Setting,parent:DirectoryParent,actor:AccountId,auth:OperationId,epoch:i64,
    require:&'q dyn Fn(&RenameCut<'_>)->io::Result<()>,
}
impl RenameAt<'_,'_>{
    fn authorized(&self,db:&Connection,catalog:&Catalog)->io::Result<()>{
        (self.require)(&RenameCut{source:self.source,db,catalog})
    }

    fn rename(&self,root:OrdinaryView<'_>,expected:&Place,opened:&Installed,
        op:OperationId,name:&str,alive:&impl Fn()->io::Result<()>,
        point:impl Fn(Cut)->io::Result<()>)->io::Result<Renamed>{
        display::component(name.as_bytes(),222)?;
        let src=self.source;let grant=self.setting;
        if expected.name==name||expected.role!=Role::File||(expected.volume,expected.parent)!=(grant.volume,self.parent)
            ||!root.for_tree(src,grant.volume)||root.parent()!=self.parent{return Err(denied());}
        let (original,birth,members,alias)=src.at(|db,c|{
            self.authorized(db,&c)?;alive()?;
            let selected=Place::current(db,&c,expected.file)?;
            if &selected!=expected{return Err(denied());}
            Place::require_no_rename(db,expected.file)?;
            vacant(db,&c,expected,expected.parent,name)?;
            let b=selected.bind(db,&c)?.birth;
            let r=c.rows().get(1).filter(|r|r.role==Role::Recycle).ok_or_else(denied)?;
            b.validate_transfer_geometry(name,(&r.path,r.limit),op)?;
            let planned=Original{op,from:expected.clone(),to_parent:expected.parent,name:name.to_owned(),share:grant.share,setting:grant.op,
                auth:self.auth,actor:self.actor,epoch:self.epoch,save:work_record::selected(db,&b)?.map(|v|v.record.key().save),
                manual:work_record::latest_commit(db,&b)?.map(|n|n.0)};
            planned.digest(&b)?;
            let members=work_record::members(db,&c,&b)?;
            let alias=file_create::display_owner_at(db,&c,expected.volume,expected.parent,name)?;
            Ok((planned,b,members,alias))
        })?;
        root.require_streaming()?;
        let parent=opened.containing_parent(root.folder())?;
        let private=expected.backing.is_some();
        let move_one=RenameInstall::hold(&birth,parent,&expected.name,name,RenameStage::BeforeReserve,|w|members.require(w))?;
        move_one.require_original(opened)?;pocket_shape(private,parent,move_one.current_directory(),&birth)?;
        let locked=namespace_lock(root.folder())?;
        let names=census(&locked)?;
        let _alias=directory::check_alias_at(root.folder(),name,alias.as_ref(),Some(&names))?;
        let terminal=original.target().leaf()?;
        root.require_streaming()?;opened.require(root.folder())?;
        let w=Connection::open(&src.dir,true)?;
        let attempt=src.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
            self.authorized(db,&c)?;alive()?;
            if Place::current(db,&c,expected.file)?!=*expected||
               work_record::members(db,&c,&birth)?!=members||
               file_create::display_owner_at(db,&c,expected.volume,expected.parent,name)?!=alias{
                return Err(busy());
            }
            vacant(db,&c,expected,expected.parent,name)?;
            ordinary_directory::budget(db,expected.volume,expected.parent,&names,
                (!private).then_some(Additional::Rename(expected.file,&terminal)))?;
            original.issue(db,&birth)
        });
        drop(w);drop(locked);drop(_alias);
        if let Err(error)=attempt{
            match src.at(|db,_|Original::fetch(db,&birth,op)){
                Ok(None)=>return Err(error),
                Ok(Some((it,Status::Pending))) if it==original=>{},
                _=>return Err(unknown()),
            }
        }
        // From here an exact durable operation may finish independently on
        // recovery. Never call any error a definite rejected network rename.
        let execute=(||->io::Result<_>{
            point(Cut::Reserved)?;
            let check_live=||src.at(|db,c|{
                self.authorized(db,&c)?;
                original.require_exact(db,&birth,Status::Pending)?;
                original.live(db)?;alive()
            });
            let (same,selected)=move_one.install(||{
                root.require_streaming()?;check_live()?;alive()
            },||point(Cut::NameChanged),|actually|{
                // For a flat FILE the installation already holds this
                // namespace lock. A private pocket uses the ordinary
                // parent's own short independent lock for logical names.
                let _outer=if private{Some(namespace_lock(root.folder())?)}else{None};
                root.require_streaming()?;pocket_shape(private,parent,actually.directory(),&birth)?;
                let _alias=directory::check_alias_at(root.folder(),name,alias.as_ref(),None)?;
                let w=Connection::open(&src.dir,true)?;
                let target=original.target();
                let result=src.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
                    self.authorized(db,&c)?;
                    original.require_exact(db,&birth,Status::Pending)?;
                    original.live(db)?;
                    if file_create::display_owner_at(db,&c,expected.volume,expected.parent,name)?!=alias{return Err(busy());}
                    alive()?;original.confirm(db)?;
                    if Place::current(db,&c,expected.file)?!=target{return Err(denied());}
                    Ok(())
                });
                drop(w);
                if result.is_err(){
                    src.at(|db,c|{
                        self.authorized(db,&c)?;original.require_exact(db,&birth,Status::Confirmed)?;
                        if Place::current(db,&c,expected.file)?!=target{return Err(denied());}Ok(())
                    })?;
                }
                Ok(target)
            })?;
            point(Cut::Confirmed)?;
            Ok((selected,same))
        })();
        let (place,body)=execute.map_err(|_|unknown())?;
        Ok(Renamed{place,body,proof:ExactRename(original)})
    }
}

#[path="smb_rust_rename.rs"]
mod rust;
pub(super) use rust::{HeldRenameResult,rename_held,require_writable};

#[path="file_parent_change.rs"]
mod parent_change;
