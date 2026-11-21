//! One immutable request for a directory's same-parent name. The original
//! directory marker and all child parent keys continue to identify the birth.
use super::{Additional,Catalog,Connection,DirectoryBirth,DirectoryPlace,DirectoryHold,DirectoryParent,
    OperationId,OrdinaryRoot,Route,Tree,AccountId,ShareId,VolumeId,display,denied,invalid,work_record};
use super::super::super::{InitialReader,volume::measure};
use crate::{fs::directory_birth::{DirectoryRename,DirectoryRenameSide},smb::OrdinaryDirectoryRenameAdmission,
    system::reader::{account::share_mode::{Mode,Setting},smb::runtime::RustRoot}};
use crate::ordinary::portable_birth::Birth;
use crate::hash::Sha256;
use std::{ffi::CString,io,thread,time::Duration};

#[derive(Clone,Debug,Eq,PartialEq)]
pub(super) struct Rename{
    pub(super) op:OperationId,pub(super) birth:DirectoryBirth,pub(super) source:OperationId,
    pub(super) source_gen:i64,pub(super) old:String,pub(super) target:String,
    pub(super) share:ShareId,pub(super) setting:OperationId,pub(super) auth:OperationId,
    pub(super) actor:AccountId,pub(super) epoch:i64,
    pub(super) confirmed:bool,pub(super) aborted:bool,
}
impl Rename{
    pub(super) fn prepare(op:OperationId,from:&DirectoryPlace,target:&str,share:ShareId,setting:OperationId,
                         auth:OperationId,actor:AccountId,epoch:i64)->io::Result<Self>{
        let x=Self{op,birth:from.birth().clone(),source:from.position(),source_gen:from.generation(),old:from.name().into(),
            target:target.into(),share,setting,auth,actor,epoch,confirmed:false,aborted:false};
        x.digest()?;Ok(x)
    }
    fn digest(&self)->io::Result<[u8;32]>{
        let b=&self.birth;b.valid()?;
        if [self.op.0,self.source.0,self.share.0,self.setting.0,self.auth.0,self.actor.0].contains(&[0;16])
            ||self.op.0==[0xff;16]||self.op==self.source||self.op==b.original||self.old==self.target
            || !(1..(i64::MAX-1)).contains(&self.source_gen)||self.epoch<1
            ||(self.source_gen==1&&(self.source!=b.original||self.old!=b.name))
            ||(self.source_gen>1&&self.source==b.original){
            return Err(invalid("directory rename lacks its original source and authority"));
        }
        let mut sha=Sha256::new();sha.update(b"ZXS-SQLITE-1/RENAME-DIRECTORY\0")?;
        for id in [b.pool.0,b.original.0,b.volume.0,b.parent.bytes(),b.id.0,b.entry.0,
                   self.op.0,self.source.0,self.share.0,self.setting.0,self.auth.0,self.actor.0]{
            sha.update(&id)?;
        }
        sha.update(&self.source_gen.to_be_bytes())?;sha.update(&self.epoch.to_be_bytes())?;
        for name in [&self.old,&self.target]{
            let value=display::component(name.as_bytes(),255)?.as_bytes();
            if Birth::reserved_spelling(value){return Err(invalid("a directory cannot take a private name"));}
            sha.update(&[value.len() as u8])?;sha.update(value)?;
        }
        Ok(sha.finish())
    }
    /// A single existing short writer transaction supplies this called lane.
    /// Exact parent/physical vacancy and live handle are independently verified.
    pub(super) fn issue(&self,db:&Connection)->io::Result<()>{
        if self.confirmed||self.aborted{return Err(invalid("not a new directory rename"));}
        let digest=self.digest()?;
        let q=db.prepare(c"INSERT INTO main.directory_rename(rename_op,directory_id,source_op,source_gen,
              source_name,target_name,share_id,setting_op,auth_op) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)")?;
        for(i,x)in [(1,self.op.0),(2,self.birth.id.0),(3,self.source.0),(7,self.share.0),(8,self.setting.0),(9,self.auth.0)]{
            q.bind_bytes(i,&x)?;
        }
        q.bind_integer(4,self.source_gen)?;q.bind_bytes(5,self.old.as_bytes())?;q.bind_bytes(6,self.target.as_bytes())?;q.done()?;drop(q);
        let q=db.prepare(c"INSERT INTO main.operation(op_id,kind,actor_id,request_sha,state) VALUES(?1,16,?2,?3,0)")?;
        q.bind_bytes(1,&self.op.0)?;q.bind_bytes(2,&self.actor.0)?;q.bind_bytes(3,&digest)?;q.done()
    }
    pub(super) fn confirm(&self,db:&Connection,c:&Catalog)->io::Result<()>{
        self.require_pending(db,c)?;
        let q=db.prepare(c"UPDATE main.operation SET state=1 WHERE op_id=?1 AND kind=16 AND state=0 RETURNING op_id")?;
        q.bind_bytes(1,&self.op.0)?;
        if !work_record::start_one(&q)?||q.array::<16>(0)?!=self.op.0{return Err(invalid("no original pending directory rename"));}
        q.done()
    }
    fn require_pending(&self,db:&Connection,c:&Catalog)->io::Result<()>{
        if self.aborted||self.confirmed||selected(db,c,self.op)?.as_ref()!=Some(self){
            return Err(invalid("expected the same exact pending directory rename"));
        }
        Ok(())
    }
}
/// Reads a real receipt, not a permission to write or proof that its media
/// is usable. Its predecessor must be the original birth or an earlier
/// confirmed placement of this exact directory, never another directory ID.
pub(super) fn selected(db:&Connection,c:&Catalog,op:OperationId)->io::Result<Option<Rename>>{
    let q=db.prepare(c"SELECT o.state,o.actor_id,o.request_sha,d.create_op,r.directory_id,r.source_op,r.source_gen,
        r.source_name,r.target_name,r.share_id,r.setting_op,r.auth_op,e.generation,
        EXISTS(SELECT 1 FROM main.directory_rename_abort a WHERE a.rename_op=r.rename_op)
      FROM main.directory_rename r JOIN main.operation o ON o.op_id=r.rename_op AND o.kind=16
      JOIN main.directory_entry d ON d.directory_id=r.directory_id
      JOIN main.operation p ON p.op_id=r.source_op AND p.state=1
      JOIN main.credential_epoch e ON e.role=1 AND e.credential_op=r.auth_op
      JOIN main.account_pair ac ON ac.singleton=e.slot AND ac.smb_id=o.actor_id
      JOIN main.root_share s ON s.share_id=r.share_id AND s.volume_id=d.volume_id
      JOIN main.share_mode m ON m.share_id=r.share_id AND m.change_op=r.setting_op AND m.mode=2
      LEFT JOIN main.directory_rename old ON p.kind=16 AND old.rename_op=p.op_id
      WHERE r.rename_op=?1 AND ((p.kind=13 AND p.op_id=d.create_op AND r.source_gen=1 AND r.source_name=d.display)
          OR (p.kind=16 AND old.directory_id=d.directory_id AND old.target_name=r.source_name
                 AND old.source_gen=r.source_gen-1))")?;
    q.bind_bytes(1,&op.0)?;
    if !work_record::start_one(&q)?{return Ok(None);}
    let op_birth=OperationId(q.array(3)?);
    let birth=super::original(db,c,op_birth)?.filter(|b|b.done&&!b.aborted).ok_or_else(super::denied)?.birth;
    if birth.id.0!=q.array::<16>(4)?{return Err(invalid("receipt differs from its actual directory identity"));}
    let (state,aborted)=(q.byte(0)?,q.byte(13)?);
    if state>1||aborted>1||state+aborted>1{return Err(invalid("impossible directory rename state"));}
    let r=Rename{op,birth,source:OperationId(q.array(5)?),source_gen:q.integer(6)?,
        old:display::component(q.blob(7,255)?,255)?.into(),target:display::component(q.blob(8,255)?,255)?.into(),
        share:ShareId(q.array(9)?),setting:OperationId(q.array(10)?),auth:OperationId(q.array(11)?),
        actor:AccountId(q.array(1)?),epoch:q.integer(12)?,confirmed:state==1,aborted:aborted==1};
    if q.array::<32>(2)?!=r.digest()?{return Err(invalid("directory rename differs from its original request"));}
    q.done()?;Ok(Some(r))
}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::system::reader::smb) enum RelabelCut{Reserved,NameChanged,Confirmed}

fn unknown()->io::Error{
    io::Error::new(io::ErrorKind::ConnectionAborted,"directory rename outcome is unknown on this original handle")
}

impl Rename{
    fn placed_at(&self,db:&Connection,c:&Catalog,done:bool)->io::Result<Route>{
        let id=DirectoryParent::Directory(self.birth.id);
        let route=Route::at(db,c,self.birth.volume,id)?;
        let position=route.selected().ok_or_else(denied)?;
        let (op,generation,name)=if done{(self.op,self.source_gen+1,self.target.as_str())}
            else{(self.source,self.source_gen,self.old.as_str())};
        if position.birth()!=&self.birth||(position.position(),position.generation(),position.name())!=(op,generation,name){
            return Err(denied());
        }
        Ok(route)
    }

    fn live(&self,db:&Connection)->io::Result<()>{
        let q=db.prepare(c"SELECT actor_id FROM main.live_directory_rename WHERE rename_op=?1")?;
        q.bind_bytes(1,&self.op.0)?;
        if !work_record::start_one(&q)?||q.array::<16>(0)?!=self.actor.0{return Err(denied());}
        q.done()
    }

    fn confirmed_position(&self,db:&Connection,c:&Catalog)->io::Result<Route>{
        let mut exact=self.clone();exact.confirmed=true;
        if selected(db,c,self.op)?.as_ref()!=Some(&exact){return Err(denied());}
        self.placed_at(db,c,true)
    }

    fn is_unused(&self,db:&Connection)->io::Result<bool>{
        let q=db.prepare(c"SELECT 1 FROM main.operation WHERE op_id=?1
            UNION ALL SELECT 1 FROM main.directory_rename WHERE rename_op=?1 LIMIT 1")?;
        q.bind_bytes(1,&self.op.0)?;
        Ok(!work_record::start_one(&q)?)
    }
}

impl Tree<'_,'_>{
    /// The directory itself has already been opened by this exact session.
    /// It must be the sole user of its original marker OFD (including all
    /// same-OFD borrows). This consumes it only for the exact same-parent
    /// selection, returning the original read lease re-bound on success.
    pub(in crate::system::reader::smb) fn relabel_directory(&self,target:&str,op:OperationId,old:&Route,
        owner:&mut Option<DirectoryHold>,live:&impl Fn()->io::Result<()>,point:impl Fn(RelabelCut)->io::Result<()>)
        ->io::Result<(Route,DirectoryHold)>{
        let src=self.session.source;let g=self.setting;let a=&self.session.original;
        let require=|cut:&DirectoryRelabelCut<'_>|{
            debug_assert!(std::ptr::eq(cut.source,src));
            self.require(cut.db,cut.catalog,true)
        };
        let original=RelabelAt{source:src,setting:g,directory:self.parent,actor:a.account,
            auth:a.op,epoch:a.generation,require:&require};
        original.relabel(target,op,old,owner,live,point).map(|(route,hold,_)|(route,hold))
    }
}

/// A private borrow of the exact Core reader and its ongoing SQLite cut.
/// Only the common relabel engine can supply one; it grants no handle.
pub(in crate::system::reader::smb) struct DirectoryRelabelCut<'a>{
    source:&'a InitialReader,db:&'a Connection,catalog:&'a Catalog,
}
impl<'a> DirectoryRelabelCut<'a>{
    pub(in crate::system::reader::smb) fn parts(&self)->(&'a InitialReader,&'a Connection,&'a Catalog){
        (self.source,self.db,self.catalog)
    }
}
struct RelabelAt<'a>{
    source:&'a InitialReader,setting:Setting,directory:DirectoryParent,actor:AccountId,auth:OperationId,epoch:i64,
    require:&'a dyn Fn(&DirectoryRelabelCut<'_>)->io::Result<()>,
}
#[must_use]
pub(in crate::system::reader::smb) struct ConfirmedRelabel{original:Rename}
impl ConfirmedRelabel{
    pub(in crate::system::reader::smb) fn require(&self,db:&Connection,c:&Catalog,route:&Route)->io::Result<()>{
        if self.original.confirmed_position(db,c)?!=*route{return Err(denied());}
        Ok(())
    }
}
impl RelabelAt<'_>{
    /// One operation, one existing directory marker. Both SMB families
    /// use these same physical and SQL transitions, but not the authority
    /// that certifies each of their exact active account/setting cuts.
    fn relabel(&self,target:&str,op:OperationId,old:&Route,owner:&mut Option<DirectoryHold>,
        live:&impl Fn()->io::Result<()>,point:impl Fn(RelabelCut)->io::Result<()>)
        ->io::Result<(Route,DirectoryHold,ConfirmedRelabel)>{
        let src=self.source;let g=self.setting;
        if old.volume()!=g.volume||old.parent()!=self.directory||owner.is_none(){return Err(denied());}
        let authorize=|db:&Connection,catalog:&Catalog|(self.require)(&DirectoryRelabelCut{source:src,db,catalog});
        let mut parent=old.clone();
        let previous=parent.steps.pop().ok_or_else(denied)?; // never the enrolled F itself
        let r=Rename::prepare(op,&previous,target,g.share,g.op,self.auth,self.actor,self.epoch)?;
        if old.parent()!=DirectoryParent::Directory(r.birth.id)||parent.parent()!=r.birth.parent{return Err(denied());}
        let exact_parent=|db:&Connection,c:&Catalog|{
            if Route::at(db,c,g.volume,r.birth.parent)?!=parent{return Err(denied());}
            Ok(())
        };
        let exact_old=|db:&Connection,c:&Catalog|{
            exact_parent(db,c)?;
            if r.placed_at(db,c,false)?!=*old||!r.is_unused(db)?{return Err(denied());}
            super::directory_vacant(db,c,&parent,target)
        };
        let new=CString::new(r.target.as_bytes()).map_err(|_|denied())?;
        let catalog=src.at(|db,c|{
            authorize(db,&c)?;exact_old(db,&c)?;live()?;Ok(c)
        })?;
        let ordinary=OrdinaryRoot::at_parent(src,&catalog,&parent,None)?;
        if measure(&ordinary.root)?.read_only{return Err(io::ErrorKind::PermissionDenied.into());}
        ordinary.require_streaming()?;live()?;
        let move_one=match DirectoryRename::hold_live(&r.birth,ordinary.folder(),&r.old,&r.target,owner){
            Err(_) if owner.is_none()=>return Err(unknown()),
            Err(e)=>return Err(e),
            Ok(x)=>x,
        };
        let attempt=(||{
            let census=move_one.reservation_sample()?;
            ordinary.require_streaming()?;
            let out=Connection::open(&src.dir,true)?;
            src.transaction_before_commit(&out,c"BEGIN IMMEDIATE",|db,c|{
                authorize(db,&c)?;exact_old(db,&c)?;
                super::budget(db,g.volume,r.birth.parent,&census,Some(Additional::RenameDirectory(r.birth.id,&new)))?;
                live()?;r.issue(db)
            },live)
        })();
        if let Err(error)=attempt{
            match src.at(|db,c|{
                if r.is_unused(db)?{Ok(false)}else{r.require_pending(db,&c)?;Ok(true)}
            }){
                Ok(false)=>{
                    *owner=move_one.release_unchanged().ok();
                    return Err(if owner.is_some(){error}else{unknown()});
                },
                Ok(true)=>{}, // the actual original request survived the uncertain COMMIT
                Err(_)=>return Err(unknown()),
            }
        }
        let changed=(||{
            point(RelabelCut::Reserved)?;
            let admit=||{
                ordinary.require_streaming()?;
                src.at(|db,c|{
                    authorize(db,&c)?;exact_parent(db,&c)?;r.require_pending(db,&c)?;r.live(db)?;live()
                })?;
                // The transaction may have done additional S-name I/O.
                live()
            };
            let (held,selected)=move_one.settle(DirectoryRenameSide::Destination,admit,||point(RelabelCut::NameChanged),|_|{
                ordinary.require_streaming()?;
                let out=Connection::open(&src.dir,true)?;
                let decided=src.transaction_before_commit(&out,c"BEGIN IMMEDIATE",|db,c|{
                    authorize(db,&c)?;exact_parent(db,&c)?;r.live(db)?;live()?;
                    r.confirm(db,&c)?;r.confirmed_position(db,&c)
                },live);
                drop(out);
                match decided{
                    Ok(new)=>Ok(new),
                    Err(_)=>src.at(|db,c|{authorize(db,&c)?;exact_parent(db,&c)?;r.confirmed_position(db,&c)}),
                }
            })?;
            point(RelabelCut::Confirmed)?;
            live()?;Ok((selected,held))
        })();
        changed.map(|(route,held)|(route,held,ConfirmedRelabel{original:r})).map_err(|_:io::Error|unknown())
    }
}

pub(in crate::system::reader::smb) fn relabel_rust(tree:&RustRoot,ask:&OrdinaryDirectoryRenameAdmission<'_>,label:&str,
    setting:Setting,old:&Route,owner:&mut Option<DirectoryHold>)
    ->io::Result<(Route,DirectoryHold,ConfirmedRelabel)>{
    let DirectoryParent::Directory(directory)=old.parent() else{return Err(denied());};
    let (member,target)=ask.held(tree.pool(),directory)?;
    if !member.is_set_info()||setting.mode!=Mode::ReadWrite||old.volume()!=setting.volume{return Err(denied());}
    let source=tree.ordinary_reader(member)?;
    let (actor,auth,epoch,grant)=source.at(|db,catalog|
        tree.ordinary_directory_relabel_at(&DirectoryRelabelCut{source,db,catalog:&catalog},label))?;
    if grant!=setting{return Err(denied());}
    let require=|cut:&DirectoryRelabelCut<'_>|{
        if tree.ordinary_directory_relabel_at(cut,label)?!=(actor,auth,epoch,setting){return Err(denied());}
        Ok(())
    };
    let original=RelabelAt{source,setting,directory:old.parent(),actor,auth,epoch,require:&require};
    let op=OperationId::new()?;
    original.relabel(target,op,old,owner,&||tree.original_write_live(member),|_|Ok(()))
}

type Allowed<'a>=dyn Fn(&Connection,&Catalog,ShareId,VolumeId)->io::Result<()>+'a;
#[derive(Clone,Copy,Eq,PartialEq)]
struct Recovery{setting:OperationId,auth:OperationId,forward:bool}
impl Rename{
    fn cold(&self,db:&Connection,c:&Catalog,allowed:&Allowed<'_>)->io::Result<Recovery>{
        self.require_pending(db,c)?;
        allowed(db,c,self.share,self.birth.volume)?;
        let q=db.prepare(c"SELECT a.setting_op,a.auth_op,a.actor_id
            FROM main.live_ordinary_writer a JOIN main.directory_selected d ON d.volume_id=a.volume_id
            WHERE a.share_id=?1 AND d.directory_id=?2 AND d.state=1 AND d.position_op=?3 AND d.generation=?4 AND d.display=?5")?;
        q.bind_bytes(1,&self.share.0)?;q.bind_bytes(2,&self.birth.id.0)?;q.bind_bytes(3,&self.source.0)?;
        q.bind_integer(4,self.source_gen)?;q.bind_bytes(5,self.old.as_bytes())?;
        if !work_record::start_one(&q)?||q.array::<16>(2)?!=self.actor.0{return Err(denied());}
        let(setting,auth)=(OperationId(q.array(0)?),OperationId(q.array(1)?));q.done()?;
        let forward=(setting,auth)==(self.setting,self.auth);
        if forward{self.live(db)?;}
        Ok(Recovery{setting,auth,forward})
    }
    fn abort(&self,db:&Connection,c:&Catalog,authority:Recovery,reason:i64)->io::Result<()>{
        self.require_pending(db,c)?;
        let q=db.prepare(c"INSERT INTO main.directory_rename_abort(rename_op,setting_op,auth_op,reason) VALUES(?1,?2,?3,?4)")?;
        q.bind_bytes(1,&self.op.0)?;q.bind_bytes(2,&authority.setting.0)?;q.bind_bytes(3,&authority.auth.0)?;
        q.bind_integer(4,reason)?;q.done()
    }
}

fn recover(src:&InitialReader,op:OperationId,allowed:&Allowed<'_>,alive:&impl Fn()->io::Result<()>)->io::Result<()>{
    let (catalog,original,parent,authority)=src.at(|db,c|{
        let r=selected(db,&c,op)?.ok_or_else(denied)?;
        let authority=r.cold(db,&c,allowed)?;
        let parent=Route::at(db,&c,r.birth.volume,r.birth.parent)?;
        alive()?;Ok((c,r,parent,authority))
    })?;
    let actual=OrdinaryRoot::at_parent(src,&catalog,&parent,None)?;
    if measure(&actual.root)?.read_only{return Err(io::ErrorKind::PermissionDenied.into());}
    let move_one=DirectoryRename::hold(&original.birth,actual.folder(),&original.old,&original.target,None)?;
    let was_new=move_one.side()==DirectoryRenameSide::Destination;
    let advance=was_new&&authority.forward;
    let terminal=if advance{DirectoryRenameSide::Destination}else{DirectoryRenameSide::Original};
    let require=|db:&Connection,c:&Catalog|{
        if original.cold(db,c,allowed)?!=authority||Route::at(db,c,original.birth.volume,original.birth.parent)?!=parent{
            return Err(denied());
        }
        alive()
    };
    let admit=||{actual.require_streaming()?;src.at(|db,c|require(db,&c))?;alive()};
    // Even a cold no-syscall terminal checks the original admission and
    // current mounted scope; the final write repeats the exact SQL cut.
    admit()?;
    move_one.settle(terminal,admit,||Ok(()),|_|{
        actual.require_streaming()?;
        let w=Connection::open(&src.dir,true)?;
        src.transaction_before_commit(&w,c"BEGIN IMMEDIATE",|db,c|{
            require(db,&c)?;
            if advance{
                original.confirm(db,&c)?;original.confirmed_position(db,&c)?;
            }else{
                original.abort(db,&c,authority,if was_new{2}else{1})?;original.placed_at(db,&c,false)?;
            }
            Ok(())
        },alive)
    })?;
    Ok(())
}

pub(in crate::system::reader::smb) fn recovery_loop(src:&InitialReader,allowed:impl Fn(&Connection,&Catalog,ShareId,VolumeId)->io::Result<()>,
    alive:impl Fn()->io::Result<()>){
    let mut after=OperationId([0;16]);
    while alive().is_ok(){
        let rows=src.at(|db,_|{
            let q=db.prepare(c"SELECT o.op_id FROM main.operation o JOIN main.directory_rename r ON r.rename_op=o.op_id
                WHERE o.kind=16 AND o.state=0 AND o.op_id>?1
                  AND NOT EXISTS(SELECT 1 FROM main.directory_rename_abort x WHERE x.rename_op=o.op_id)
                ORDER BY o.op_id LIMIT 16")?;
            q.bind_bytes(1,&after.0)?;
            let mut rows=Vec::with_capacity(16);
            while work_record::start_one(&q)?{rows.push(OperationId(q.array(0)?));}
            Ok(rows)
        });
        let pause=match rows{
            Ok(rows) if !rows.is_empty()=>{
                for op in rows{after=op;if alive().is_err(){break;}let _=recover(src,op,&allowed,&alive);}
                250
            },
            _=>{after=OperationId([0;16]);1000},
        };
        thread::sleep(Duration::from_millis(pause));
    }
}

#[cfg(test)]
mod tests{
    use super::{selected,recover,RelabelCut,Rename,OperationId,Birth,Connection,OrdinaryRoot};
    use crate::system::reader::smb::ordinary_directory::{DirectoryParent,HeldDir,Route};
    use std::io;
    use crate::{fs::directory_birth::{DirectoryRename,DirectoryRenameSide},
        system::reader::{InitialReader,ordinary_pool,smb::SmbSession,account::{history::TimelinePlace,manual_work::PickerItem,share_mode::Mode}},
        test_support::PrivateTemp};
    use std::{cell::Cell,os::unix::fs::{FileExt,MetadataExt,OpenOptionsExt}};
    fn op(x:u8)->OperationId{OperationId([x;16])}
    #[test]
    #[ignore="source-only actual SQLite, same-directory inode and immutable descendant parent, no SMB listener"]
    fn exact_pending_names_and_single_location_survive_real_directory_rename(){
        let tmp=PrivateTemp::under(&std::env::temp_dir());
        let(s,pool,f,rw,_)=ordinary_pool(&tmp);let src=InitialReader::open(&s,pool).unwrap();
        let session=SmbSession::fixture_authenticated(&src,"first.admin").unwrap();let tree=session.connect(rw).unwrap();
        let (_,root)=tree.ordinary_route(&[],true).unwrap();
        let (born,held)=tree.make_ordinary_directory(&root,op(0x61),"Reports",|_|Ok(()),&||Ok(()),|_|Ok(())).unwrap();
        let(catalog,route)=tree.ordinary_route(&["Reports".into()],true).unwrap();
        let (child,next)=tree.make_ordinary_directory(&route,op(0x62),"Notes",|_|Ok(()),&||Ok(()),|_|Ok(())).unwrap();
        let native=std::fs::metadata(&next.directory().path).unwrap().ino();drop(next);
        let occupied=tree.create_file(op(0x63),"Other.txt").unwrap();
        let inside=tree.scoped(DirectoryParent::Directory(child.id));
        let entry=inside.create_file(op(0x69),"Inside.txt").unwrap();
        let reader=InitialReader::open(&s,pool).unwrap();
        let web=reader.authenticate_web("first.admin","correct horse battery staple").unwrap();
        let input=std::fs::OpenOptions::new().read(true).write(true).create_new(true).mode(0o600)
            .open(tmp.0.join("history-input")).unwrap();
        input.write_all_at(b"v",0).unwrap();input.sync_all().unwrap();
        inside.open_writer("Inside.txt",entry.file).unwrap().save(op(0x6b),None,Some(0),1,&input).unwrap();
        let version=web.freeze(op(0x6c),web.manual_target(entry.file).unwrap()).unwrap();
        input.write_all_at(b"v2",0).unwrap();input.sync_all().unwrap();
        inside.open_writer("Inside.txt",entry.file).unwrap().save(op(0x6d),Some(op(0x6b)),Some(0),2,&input).unwrap();
        let previous=web.manual_timeline(entry.file,None).unwrap();
        assert_eq!(previous.place,TimelinePlace::Ordinary{name:"Reports\\Notes\\Inside.txt".into(),volume:born.volume});
        let (draft,_) = web.manual_preview(entry.file).unwrap();
        let raw=Connection::open(&src.dir,true).unwrap();
        let orig=route.selected().unwrap();
        let a=&session.original;
        let request=|n,leaf:&str|Rename::prepare(op(n),orig,leaf,tree.setting.share,tree.setting.op,a.op,a.account,a.generation).unwrap();
        let terminal=Birth::placed_container("Other.txt",occupied.file).unwrap();
        for (op_code,leaf) in [(0x64,"Other.txt"),(0x65,terminal.to_str().unwrap())]{
            let bad=request(op_code,leaf);
            assert!(src.transaction(&raw,c"BEGIN IMMEDIATE",|db,_|bad.issue(db)).is_err());
            assert!(src.at(|db,c|selected(db,&c,bad.op)).unwrap().is_none());
        }
        let unproved=request(0x6a,"Unproved");
        // Neither a deferred orphan nor a width-correct but wrong request
        // digest is a positive pending result. Keep both probes uncommitted.
        let breached=src.transaction(&raw,c"BEGIN IMMEDIATE",|db,c|->io::Result<()>{
            let q=db.prepare(c"INSERT INTO main.directory_rename(rename_op,directory_id,source_op,source_gen,
                   source_name,target_name,share_id,setting_op,auth_op) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)")?;
            for(i,id)in [(1,unproved.op.0),(2,unproved.birth.id.0),(3,unproved.source.0),(7,unproved.share.0),
                        (8,unproved.setting.0),(9,unproved.auth.0)]{q.bind_bytes(i,&id)?;}
            q.bind_integer(4,unproved.source_gen)?;q.bind_bytes(5,unproved.old.as_bytes())?;
            q.bind_bytes(6,unproved.target.as_bytes())?;q.done()?;drop(q);
            let scope=DirectoryParent::Directory(child.id);
            let orphan=Route::settled_at(db,&c,born.volume,scope).unwrap_err();
            assert!(orphan.to_string().contains("does not have its exact original rename"));
            let mut wrong=unproved.digest()?;wrong[0]^=1;
            let q=db.prepare(c"INSERT INTO main.operation(op_id,kind,actor_id,request_sha,state) VALUES(?1,16,?2,?3,0)")?;
            q.bind_bytes(1,&unproved.op.0)?;q.bind_bytes(2,&unproved.actor.0)?;q.bind_bytes(3,&wrong)?;q.done()?;drop(q);
            let wrong=Route::settled_at(db,&c,born.volume,scope).unwrap_err();
            assert!(wrong.to_string().contains("differs from its original request"));
            Err(io::Error::other("rollback malformed picker specimen"))
        }).unwrap_err();
        assert_eq!(breached.to_string(),"rollback malformed picker specimen");
        let r=request(0x66,"reports");
        src.transaction(&raw,c"BEGIN IMMEDIATE",|db,_|r.issue(db)).unwrap();
        assert_eq!(src.at(|db,c|selected(db,&c,r.op)).unwrap(),Some(r.clone()));
        assert!(f.join("Reports").is_dir());
        assert_eq!(OrdinaryRoot::at_parent(&src,&catalog,&route,None).err().unwrap().kind(),io::ErrorKind::WouldBlock);
        let(rows,after)=web.manual_files(None).unwrap();
        assert_eq!(rows.len(),2);assert!(after.is_none());
        assert!(rows.iter().any(|row|matches!(row,PickerItem::ParentPending(id) if *id==entry.file)));
        assert!(rows.iter().any(|row|matches!(row,PickerItem::Named(item) if item.file==occupied.file&&item.name=="Other.txt")));
        let pending=web.manual_timeline(entry.file,None).unwrap();
        assert_eq!((pending.file,pending.place,pending.next),(entry.file,TimelinePlace::ParentPending,None));
        assert!(matches!(&pending.rows[..],[row] if (row.order,row.node,row.version,row.length)==(1,version.node,version.version,1)));
        assert_eq!(web.manual_preview(entry.file).err().unwrap().kind(),io::ErrorKind::WouldBlock);
        assert_eq!(web.freeze(op(0x6e),draft).err().unwrap().kind(),io::ErrorKind::WouldBlock);
        assert!(web.manual_receipt(op(0x6e),draft).unwrap().is_none());
        let conflicting=request(0x66,"Wrong");
        assert!(src.transaction(&raw,c"BEGIN IMMEDIATE",|db,c|conflicting.confirm(db,&c)).is_err());
        assert_eq!(src.at(|db,c|selected(db,&c,r.op)).unwrap(),Some(r.clone()));
        // A pending relabel is not a pending CREATE, even for the old
        // name; the kind-13 abandon path cannot hide the original birth.
        assert!(src.transaction(&raw,c"BEGIN IMMEDIATE",|db,_|{
            let q=db.prepare(c"INSERT INTO main.directory_abort(create_op,setting_op,auth_op,reason) VALUES(?1,?2,?3,2)")?;
            q.bind_bytes(1,&born.original.0)?;q.bind_bytes(2,&r.setting.0)?;q.bind_bytes(3,&r.auth.0)?;q.done()
        }).is_err());
        for name in ["Reports","reports"]{
            assert_eq!(tree.ordinary_kind(&[name.into()]).err().unwrap().kind(),io::ErrorKind::WouldBlock);
            assert!(tree.create_file(op(0x67),name).is_err()); // exact pending directory names also reserve FILE namespace
        }
        assert_eq!(src.at(|db,c|Route::at(db,&c,born.volume,DirectoryParent::Directory(child.id))).err().unwrap().kind(),
            io::ErrorKind::WouldBlock);
        let parent=HeldDir::trusted_storage(&f).unwrap();
        let actual=DirectoryRename::hold(&born,&parent,"Reports","reports",Some(held)).unwrap();
        let (moved,())=actual.settle(DirectoryRenameSide::Destination,||Ok(()),||{
            assert_eq!(tree.ordinary_route(&["reports".into()],false).err().unwrap().kind(),io::ErrorKind::WouldBlock);Ok(())
        },|_|src.transaction(&raw,c"BEGIN IMMEDIATE",|db,c|r.confirm(db,&c))).unwrap();
        assert_eq!(moved.directory().path,f.join("reports"));drop(moved);
        assert_eq!(tree.ordinary_route(&["Reports".into()],false).err().unwrap().kind(),io::ErrorKind::NotFound);
        let (_,route)=tree.ordinary_route(&["reports".into(),"Notes".into()],false).unwrap();
        assert!(route.selected().unwrap().is_birth(&child));
        let(_,now)=tree.ordinary_route(&["reports".into()],false).unwrap();
        let now=now.selected().unwrap();
        assert_eq!(now.birth(),&born);
        assert_eq!((now.position(),now.generation()),(r.op,2));
        assert_eq!(std::fs::metadata(f.join("reports/Notes")).unwrap().ino(),native);
        let(rows,_)=web.manual_files(None).unwrap();
        assert!(rows.iter().any(|row|matches!(row,PickerItem::Named(item)
            if item.file==entry.file && item.name=="reports\\Notes\\Inside.txt")));
        let after=web.manual_timeline(entry.file,None).unwrap();
        assert_eq!(after.place,TimelinePlace::Ordinary{name:"reports\\Notes\\Inside.txt".into(),volume:born.volume});
        assert!(matches!(&after.rows[..],[row] if (row.node,row.version,row.length)==(version.node,version.version,1)));
        assert_eq!(web.manual_preview(entry.file).unwrap().0,draft);
        assert!(src.transaction(&raw,c"BEGIN IMMEDIATE",|db,c|r.confirm(db,&c)).is_err());
        let (separate,_)=tree.make_ordinary_directory(&root,op(0x68),"Reports",|_|Ok(()),&||Ok(()),|_|Ok(())).unwrap();
        assert_ne!(separate.id,born.id);
    }

    #[test]
    #[ignore="source-only original SMB tree, real filesystem cut and independent SQLite writer; no listener"]
    fn the_hot_operation_and_cold_exact_recovery_do_not_invent_a_new_directory(){
        let tmp=PrivateTemp::under(&std::env::temp_dir());
        let(s,pool,f,rw,_)=ordinary_pool(&tmp);let src=InitialReader::open(&s,pool).unwrap();
        let another=InitialReader::open(&s,pool).unwrap();
        let session=SmbSession::fixture_authenticated(&src,"first.admin").unwrap();let tree=session.connect(rw).unwrap();
        let (_,root)=tree.ordinary_route(&[],true).unwrap();
        let (born,open)=tree.make_ordinary_directory(&root,op(0x71),"Draft",|_|Ok(()),&||Ok(()),|_|Ok(())).unwrap();
        let mark=std::fs::metadata(&open.directory().path).unwrap().ino();
        let(_,old)=tree.ordinary_route(&["Draft".into()],true).unwrap();
        let child=tree.scoped(DirectoryParent::Directory(born.id));
        tree.create_file(op(0x72),"Occupied").unwrap();
        let mut saved=Some(open);
        assert_eq!(child.relabel_directory("Occupied",op(0x73),&old,&mut saved,&||Ok(()),|_|Ok(())).err().unwrap().kind(),
            io::ErrorKind::AlreadyExists);
        assert!(saved.is_some());
        let reached=Cell::new(false);
        let allow=|_:&Connection,_:&super::Catalog,share,volume| if (share,volume)==(rw,born.volume){Ok(())}
            else{Err(io::ErrorKind::PermissionDenied.into())};
        assert_eq!(child.relabel_directory("Final",op(0x74),&old,&mut saved,&||Ok(()),|cut|{
            if cut==RelabelCut::Reserved{reached.set(true);return Err(io::ErrorKind::Interrupted.into());}Ok(())
        }).err().unwrap().kind(),io::ErrorKind::ConnectionAborted);
        assert!(reached.get()&&saved.is_none()&&f.join("Draft").is_dir()&&!f.join("Final").exists());
        assert!(another.at(|db,c|selected(db,&c,op(0x74))).unwrap().is_some_and(|r|!r.aborted&&!r.confirmed));
        recover(&another,op(0x74),&allow,&||Ok(())).unwrap();
        assert!(another.at(|db,c|selected(db,&c,op(0x74))).unwrap().unwrap().aborted);
        assert!(f.join("Draft").is_dir()&&!f.join("Final").exists());

        let parent=HeldDir::trusted_storage(&f).unwrap();saved=Some(born.open(&parent).unwrap());
        reached.set(false);
        assert_eq!(child.relabel_directory("Final",op(0x75),&old,&mut saved,&||Ok(()),|cut|{
            if cut==RelabelCut::NameChanged{reached.set(true);return Err(io::ErrorKind::Interrupted.into());}Ok(())
        }).err().unwrap().kind(),io::ErrorKind::ConnectionAborted);
        assert!(reached.get()&&saved.is_none()&&!f.join("Draft").exists()&&f.join("Final").is_dir());
        let web=another.authenticate_web("first.admin","correct horse battery staple").unwrap();
        let active=web.share_setting(rw).unwrap();
        web.set_share_mode(op(0x76),rw,active.op,Mode::ReadOnly).unwrap();
        assert!(recover(&another,op(0x75),&allow,&||Ok(())).is_err());
        assert!(f.join("Final").is_dir()&&!f.join("Draft").exists());
        web.set_share_mode(op(0x77),rw,op(0x76),Mode::ReadWrite).unwrap();
        recover(&another,op(0x75),&allow,&||Ok(())).unwrap();
        assert!(another.at(|db,c|selected(db,&c,op(0x75))).unwrap().unwrap().aborted);
        let again=session.connect(rw).unwrap();
        let (_,restored)=again.ordinary_route(&["Draft".into()],true).unwrap();
        assert_eq!(restored,old);
        assert_eq!(std::fs::metadata(f.join("Draft")).unwrap().ino(),mark);

        saved=Some(born.open(&parent).unwrap());
        let exact=again.scoped(DirectoryParent::Directory(born.id));
        reached.set(false);
        assert!(exact.relabel_directory("Accepted",op(0x78),&restored,&mut saved,&||Ok(()),|at|{
            if at==RelabelCut::NameChanged{reached.set(true);return Err(io::ErrorKind::Interrupted.into());}Ok(())
        }).is_err());
        assert!(reached.get());recover(&another,op(0x78),&allow,&||Ok(())).unwrap();
        let(_,winner)=again.ordinary_route(&["Accepted".into()],false).unwrap();
        assert_eq!(winner.selected().unwrap().birth(),&born);
        assert_eq!(std::fs::metadata(f.join("Accepted")).unwrap().ino(),mark);
    }
}
