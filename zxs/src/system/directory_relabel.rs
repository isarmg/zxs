//! One immutable request to change the selected position of a directory.
//! Its marker and all child parent keys still identify the original birth.
use super::{Additional,Catalog,Connection,DirectoryBirth,DirectoryPlace,DirectoryHold,DirectoryParent,
    OperationId,OrdinaryRoot,Route,Tree,AccountId,ShareId,VolumeId,display,denied,invalid,work_record};
use super::super::super::{InitialReader,volume::measure};
use crate::{fs::directory_birth::{DirectoryRename,DirectoryRenameSide,parent_move::DirectoryParentMove},smb::OrdinaryDirectoryRenameAdmission,system::reader::{account::share_mode::{Mode,Setting},smb::runtime::RustRoot}};
use crate::ordinary::portable_birth::{Birth,ParentMoveStep};
use crate::hash::Sha256;
use std::{collections::BTreeSet,ffi::{CStr,CString},io,thread,time::Duration};

#[derive(Clone,Debug,Eq,PartialEq)]
pub(super) struct Rename{
    pub(super) op:OperationId,pub(super) birth:DirectoryBirth,pub(super) source:OperationId,
    pub(super) source_gen:i64,pub(super) old:String,pub(super) target:String,
    source_parent:DirectoryParent,target_parent:DirectoryParent,
    pub(super) share:ShareId,pub(super) setting:OperationId,pub(super) auth:OperationId,
    pub(super) actor:AccountId,pub(super) epoch:i64,
    pub(super) confirmed:bool,pub(super) aborted:bool,
}
impl Rename{
    pub(super) fn source_parent(&self)->DirectoryParent{self.source_parent}
    pub(super) fn target_parent(&self)->DirectoryParent{self.target_parent}
    pub(super) fn prepare(op:OperationId,from:&DirectoryPlace,target:&str,share:ShareId,setting:OperationId,
                         auth:OperationId,actor:AccountId,epoch:i64)->io::Result<Self>{
        Self::prepare_at(op,from,from.containing(),target,share,setting,auth,actor,epoch)
    }
    fn prepare_at(op:OperationId,from:&DirectoryPlace,to:DirectoryParent,target:&str,share:ShareId,setting:OperationId,
                         auth:OperationId,actor:AccountId,epoch:i64)->io::Result<Self>{
        let x=Self{op,birth:from.birth().clone(),source:from.position(),source_gen:from.generation(),old:from.name().into(),
            target:target.into(),source_parent:from.containing(),target_parent:to,share,setting,auth,actor,epoch,confirmed:false,aborted:false};
        x.digest()?;Ok(x)
    }
    fn digest(&self)->io::Result<[u8;32]>{
        let b=&self.birth;b.valid()?;
        if [self.op.0,self.source.0,self.share.0,self.setting.0,self.auth.0,self.actor.0].contains(&[0;16])
            ||self.op.0==[0xff;16]||self.op==self.source||self.op==b.original
            ||(self.source_parent==self.target_parent&&self.old==self.target)
            ||[self.source_parent,self.target_parent].into_iter().any(|p|matches!(p,DirectoryParent::Directory(id) if id==b.id||id.0==[0;16]))
            || !(1..(i64::MAX-1)).contains(&self.source_gen)||self.epoch<1
            ||(self.source_gen==1&&(self.source!=b.original||self.old!=b.name||self.source_parent!=b.parent))
            ||(self.source_gen>1&&self.source==b.original){
            return Err(invalid("directory rename lacks its original source and authority"));
        }
        let mut sha=Sha256::new();sha.update(b"ZXS-SQLITE-1/PLACE-DIRECTORY\0")?;
        for id in [b.pool.0,b.original.0,b.volume.0,b.parent.bytes(),b.id.0,b.entry.0,
                   self.op.0,self.source.0,self.source_parent.bytes(),self.target_parent.bytes(),
                   self.share.0,self.setting.0,self.auth.0,self.actor.0]{
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
              source_parent,target_parent,source_name,target_name,share_id,setting_op,auth_op)
              VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)")?;
        for(i,x)in [(1,self.op.0),(2,self.birth.id.0),(3,self.source.0),(5,self.source_parent.bytes()),(6,self.target_parent.bytes()),
            (9,self.share.0),(10,self.setting.0),(11,self.auth.0)]{
            q.bind_bytes(i,&x)?;
        }
        q.bind_integer(4,self.source_gen)?;q.bind_bytes(7,self.old.as_bytes())?;q.bind_bytes(8,self.target.as_bytes())?;q.done()?;drop(q);
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
    let mut q=db.prepare(c"SELECT o.state,o.actor_id,o.request_sha,d.create_op,r.directory_id,r.source_op,r.source_gen,
        r.source_parent,r.target_parent,r.source_name,r.target_name,r.share_id,r.setting_op,r.auth_op,e.generation,
        EXISTS(SELECT 1 FROM main.directory_rename_abort a WHERE a.rename_op=r.rename_op)
      FROM main.directory_rename r JOIN main.operation o ON o.op_id=r.rename_op AND o.kind=16
      JOIN main.directory_entry d ON d.directory_id=r.directory_id
      JOIN main.operation p ON p.op_id=r.source_op AND p.state=1
      JOIN main.credential_epoch e ON e.role=1 AND e.credential_op=r.auth_op
      JOIN main.account_pair ac ON ac.singleton=e.slot AND ac.smb_id=o.actor_id
      JOIN main.root_share s ON s.share_id=r.share_id AND s.volume_id=d.volume_id
      JOIN main.share_mode m ON m.share_id=r.share_id AND m.change_op=r.setting_op AND m.mode=2
      LEFT JOIN main.directory_rename old ON p.kind=16 AND old.rename_op=p.op_id
      WHERE r.rename_op=?1 AND ((p.kind=13 AND p.op_id=d.create_op AND r.source_gen=1 AND r.source_name=d.display
                                   AND r.source_parent=d.parent_id)
          OR (p.kind=16 AND old.directory_id=d.directory_id AND old.target_name=r.source_name
                 AND old.source_gen=r.source_gen-1 AND old.target_parent=r.source_parent))")?;
    q.bind_bytes(1,&op.0)?;
    if !work_record::start_one(&q)?{return Ok(None);}
    let op_birth=OperationId(q.array(3)?);
    let birth=super::original(db,c,op_birth)?.filter(|b|b.done&&!b.aborted).ok_or_else(super::denied)?.birth;
    if birth.id.0!=q.array::<16>(4)?{return Err(invalid("receipt differs from its actual directory identity"));}
    let (state,aborted)=(q.byte(0)?,q.byte(15)?);
    if state>1||aborted>1||state+aborted>1{return Err(invalid("impossible directory rename state"));}
    let r=Rename{op,birth,source:OperationId(q.array(5)?),source_gen:q.integer(6)?,
        source_parent:DirectoryParent::from_bytes(q.array(7)?),target_parent:DirectoryParent::from_bytes(q.array(8)?),
        old:display::component(q.row()?.blob(9,255)?,255)?.into(),target:display::component(q.row()?.blob(10,255)?,255)?.into(),
        share:ShareId(q.array(11)?),setting:OperationId(q.array(12)?),auth:OperationId(q.array(13)?),
        actor:AccountId(q.array(1)?),epoch:q.integer(14)?,confirmed:state==1,aborted:aborted==1};
    if q.array::<32>(2)?!=r.digest()?{return Err(invalid("directory rename differs from its original request"));}
    q.done()?;Ok(Some(r))
}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::system::reader::smb) enum RelabelCut{Reserved,OuterMoved,NameChanged,Confirmed}

fn unknown()->io::Error{
    io::Error::new(io::ErrorKind::ConnectionAborted,"directory rename outcome is unknown on this original handle")
}

impl Rename{
    fn placed_at(&self,db:&Connection,c:&Catalog,done:bool)->io::Result<Route>{
        let id=DirectoryParent::Directory(self.birth.id);
        let route=Route::at(db,c,self.birth.volume,id)?;
        let position=route.selected().ok_or_else(denied)?;
        let (op,generation,parent,name)=if done{(self.op,self.source_gen+1,self.target_parent(),self.target.as_str())}
            else{(self.source,self.source_gen,self.source_parent(),self.old.as_str())};
        if position.birth()!=&self.birth||(position.position(),position.generation(),position.containing(),position.name())!=(op,generation,parent,name){
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

/// The two checked branches are authority only under the importing caller's
/// current grant. Holding a path in the other F never substitutes for it.
struct HeldParents<'a>{from:OrdinaryRoot<'a>,to:Option<OrdinaryRoot<'a>>}
impl<'a> HeldParents<'a>{
    fn open(src:&'a InitialReader,c:&Catalog,from:&Route,to:&Route)->io::Result<Self>{
        if from.volume()!=to.volume()||(from.parent()==to.parent()&&from!=to){return Err(denied());}
        let roots=Self{from:OrdinaryRoot::at_parent(src,c,from,None)?,
            to:if from==to{None}else{Some(OrdinaryRoot::at_parent(src,c,to,None)?)}};
        if measure(&roots.from.root)?.read_only{return Err(io::ErrorKind::PermissionDenied.into());}
        roots.require()?;Ok(roots)
    }
    fn require(&self)->io::Result<()>{
        self.from.require_streaming()?;
        if let Some(to)=&self.to{
            to.require_streaming()?;
            if self.from.root.entity!=to.root.entity||!self.from.root.same_mount(&to.root)?{
                return Err(denied());
            }
        }
        Ok(())
    }
}

type Names=BTreeSet<Vec<u8>>;
enum Reservation{Within(Names),Across(Names,Names)}
impl Reservation{
    fn charge(&self,db:&Connection,r:&Rename,end:&CStr)->io::Result<()>{
        let volume=r.birth.volume;
        match self{
            Self::Within(names)=>super::budget(db,volume,r.target_parent(),names,Some(Additional::RenameDirectory(r.birth.id,end))),
            Self::Across(old,new)=>{
                super::budget(db,volume,r.source_parent(),old,None)?;
                super::budget(db,volume,r.target_parent(),new,Some(Additional::IncomingDirectory(&r.birth,end)))
            }
        }
    }
}

enum MovingDirectory<'a>{Within(DirectoryRename<'a>),Across(DirectoryParentMove<'a>)}
impl<'a> MovingDirectory<'a>{
    fn live(r:&'a Rename,parents:&'a HeldParents<'_>,owner:&mut Option<DirectoryHold>)->io::Result<Self>{
        match &parents.to{
            None=>DirectoryRename::hold_live(&r.birth,parents.from.folder(),&r.old,&r.target,owner).map(Self::Within),
            Some(to)=>DirectoryParentMove::hold_live(&r.birth,&parents.from.root,parents.from.folder(),to.folder(),
                &r.old,&r.target,owner).map(Self::Across),
        }
    }
    fn cold(r:&'a Rename,parents:&'a HeldParents<'_>)->io::Result<Self>{
        match &parents.to{
            None=>DirectoryRename::hold(&r.birth,parents.from.folder(),&r.old,&r.target,None).map(Self::Within),
            Some(to)=>DirectoryParentMove::hold_reserved(&r.birth,&parents.from.root,parents.from.folder(),to.folder(),
                &r.old,&r.target).map(Self::Across),
        }
    }
    fn reservation(&mut self)->io::Result<Reservation>{
        match self{
            Self::Within(m)=>m.reservation_sample().map(Reservation::Within),
            Self::Across(m)=>m.reservation_sample().map(|(a,b)|Reservation::Across(a,b)),
        }
    }
    fn release_unchanged(self)->io::Result<DirectoryHold>{
        match self{Self::Within(m)=>m.release_unchanged(),Self::Across(m)=>m.release_unchanged()}
    }
    fn started(&self)->bool{
        match self{Self::Within(m)=>m.side()!=DirectoryRenameSide::Original,Self::Across(m)=>m.phase()!=ParentMoveStep::Old}
    }
    fn settle<T>(self,want:DirectoryRenameSide,admit:impl Fn()->io::Result<()>,point:impl Fn(RelabelCut)->io::Result<()>,
        finish:impl FnOnce(&DirectoryHold)->io::Result<T>)->io::Result<(DirectoryHold,T)>{
        match self{
            Self::Within(m)=>m.settle(want,admit,||point(RelabelCut::NameChanged),finish),
            Self::Across(m)=>m.settle(want,|_|admit(),|at|point(if at==ParentMoveStep::LinkAtNew{RelabelCut::OuterMoved}
                else{RelabelCut::NameChanged}),finish),
        }
    }
}

fn exact_parents(db:&Connection,c:&Catalog,r:&Rename,from:&Route,to:&Route)->io::Result<()>{
    if Route::at(db,c,r.birth.volume,r.source_parent())?!=*from||
       (from!=to&&Route::at(db,c,r.birth.volume,r.target_parent())?!=*to){return Err(denied());}
    Ok(())
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
        original.relabel(target,op,old,None,owner,live,point).map(|(route,hold,_,_)|(route,hold))
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
impl<'a> RelabelAt<'a>{
    /// One operation, one existing directory marker. Both SMB families
    /// use these same physical and SQL transitions, but not the authority
    /// that certifies each of their exact active account/setting cuts.
    fn relabel(&self,target:&str,op:OperationId,old:&Route,into:Option<&Route>,owner:&mut Option<DirectoryHold>,
        live:&impl Fn()->io::Result<()>,point:impl Fn(RelabelCut)->io::Result<()>)
        ->io::Result<(Route,DirectoryHold,ConfirmedRelabel,Option<OrdinaryRoot<'a>>)>{
        let src=self.source;let g=self.setting;
        if old.volume()!=g.volume||old.parent()!=self.directory||owner.is_none(){return Err(denied());}
        let authorize=|db:&Connection,catalog:&Catalog|(self.require)(&DirectoryRelabelCut{source:src,db,catalog});
        let mut parent=old.clone();
        let previous=parent.steps.pop().ok_or_else(denied)?; // never the enrolled F itself
        let to=into.unwrap_or(&parent);
        if to.volume()!=g.volume||to.steps.iter().any(|s|s.birth().id==previous.birth().id){return Err(denied());}
        let r=if into.is_none(){Rename::prepare(op,&previous,target,g.share,g.op,self.auth,self.actor,self.epoch)?}
            else{Rename::prepare_at(op,&previous,to.parent(),target,g.share,g.op,self.auth,self.actor,self.epoch)?};
        if old.parent()!=DirectoryParent::Directory(r.birth.id)||parent.parent()!=r.source_parent(){return Err(denied());}
        let exact_old=|db:&Connection,c:&Catalog|{
            exact_parents(db,c,&r,&parent,to)?;
            if r.placed_at(db,c,false)?!=*old||!r.is_unused(db)?{return Err(denied());}
            super::directory_vacant(db,c,to,target)
        };
        let new=CString::new(r.target.as_bytes()).map_err(|_|denied())?;
        let catalog=src.at(|db,c|{
            authorize(db,&c)?;exact_old(db,&c)?;live()?;Ok(c)
        })?;
        let roots=HeldParents::open(src,&catalog,&parent,to)?;live()?;
        let mut moving=match MovingDirectory::live(&r,&roots,owner){
            Err(_) if owner.is_none()=>return Err(unknown()),
            Err(e)=>return Err(e),
            Ok(x)=>x,
        };
        let attempt=(||{
            let census=moving.reservation()?;
            roots.require()?;
            let out=Connection::open(&src.dir,true)?;
            src.transaction_before_commit(&out,c"BEGIN IMMEDIATE",|db,c|{
                authorize(db,&c)?;exact_old(db,&c)?;
                census.charge(db,&r,&new)?;
                live()?;r.issue(db)
            },live)
        })();
        if let Err(error)=attempt{
            match src.at(|db,c|{
                if r.is_unused(db)?{Ok(false)}else{r.require_pending(db,&c)?;Ok(true)}
            }){
                Ok(false)=>{
                    *owner=moving.release_unchanged().ok();
                    return Err(if owner.is_some(){error}else{unknown()});
                },
                Ok(true)=>{}, // the actual original request survived the uncertain COMMIT
                Err(_)=>return Err(unknown()),
            }
        }
        let changed=(||{
            point(RelabelCut::Reserved)?;
            let admit=||{
                roots.require()?;
                src.at(|db,c|{
                    authorize(db,&c)?;exact_parents(db,&c,&r,&parent,to)?;r.require_pending(db,&c)?;r.live(db)?;live()
                })?;
                // The transaction may have done additional S-name I/O.
                live()
            };
            let (held,selected)=moving.settle(DirectoryRenameSide::Destination,admit,&point,|_|{
                roots.require()?;
                let out=Connection::open(&src.dir,true)?;
                let decided=src.transaction_before_commit(&out,c"BEGIN IMMEDIATE",|db,c|{
                    authorize(db,&c)?;exact_parents(db,&c,&r,&parent,to)?;r.live(db)?;live()?;
                    r.confirm(db,&c)?;r.confirmed_position(db,&c)
                },live);
                drop(out);
                match decided{
                    Ok(new)=>Ok(new),
                    Err(_)=>src.at(|db,c|{authorize(db,&c)?;exact_parents(db,&c,&r,&parent,to)?;r.confirmed_position(db,&c)}),
                }
            })?;
            point(RelabelCut::Confirmed)?;
            live()?;Ok((selected,held))
        })();
        let (route,held)=changed.map_err(|_:io::Error|unknown())?;
        Ok((route,held,ConfirmedRelabel{original:r},roots.to))
    }
}

pub(in crate::system::reader::smb) fn relabel_rust(tree:&RustRoot,ask:&OrdinaryDirectoryRenameAdmission<'_>,label:&str,
    setting:Setting,old:&Route,owner:&mut Option<DirectoryHold>)
    ->io::Result<(Route,DirectoryHold,ConfirmedRelabel,Option<super::super::super::volume::OrdinaryMedia>)>{
    let DirectoryParent::Directory(directory)=old.parent() else{return Err(denied());};
    let (member,target,parents)=ask.held(tree.pool(),directory)?;
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
    let into=parents.map(|names|source.at(|db,c|Route::by_names(db,&c,setting.volume,names))).transpose()?;
    let op=OperationId::new()?;
    original.relabel(target,op,old,into.as_ref(),owner,&||tree.original_write_live(member),|_|Ok(()))
        .map(|(route,held,proof,parent)|(route,held,proof,parent.map(OrdinaryRoot::into_media)))
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
            WHERE a.share_id=?1 AND d.directory_id=?2 AND d.state=1 AND d.position_op=?3 AND d.generation=?4 AND d.display=?5
                AND d.parent_id=?6")?;
        q.bind_bytes(1,&self.share.0)?;q.bind_bytes(2,&self.birth.id.0)?;q.bind_bytes(3,&self.source.0)?;
        q.bind_integer(4,self.source_gen)?;q.bind_bytes(5,self.old.as_bytes())?;
        q.bind_bytes(6,&self.source_parent().bytes())?;
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
    let (catalog,original,from,to,authority)=src.at(|db,c|{
        let r=selected(db,&c,op)?.ok_or_else(denied)?;
        let authority=r.cold(db,&c,allowed)?;
        let from=Route::at(db,&c,r.birth.volume,r.source_parent())?;
        let to=if r.source_parent()==r.target_parent(){from.clone()}else{Route::at(db,&c,r.birth.volume,r.target_parent())?};
        alive()?;Ok((c,r,from,to,authority))
    })?;
    let parents=HeldParents::open(src,&catalog,&from,&to)?;
    let moving=MovingDirectory::cold(&original,&parents)?;
    let started=moving.started();
    let advance=started&&authority.forward;
    let terminal=if advance{DirectoryRenameSide::Destination}else{DirectoryRenameSide::Original};
    let require=|db:&Connection,c:&Catalog|{
        exact_parents(db,c,&original,&from,&to)?;
        if original.cold(db,c,allowed)?!=authority{return Err(denied());}
        alive()
    };
    let admit=||{parents.require()?;src.at(|db,c|require(db,&c))?;alive()};
    // Even a cold no-syscall terminal checks the original admission and
    // current mounted scope; the final write repeats the exact SQL cut.
    admit()?;
    moving.settle(terminal,admit,|_|Ok(()),|_|{
        parents.require()?;
        let w=Connection::open(&src.dir,true)?;
        src.transaction_before_commit(&w,c"BEGIN IMMEDIATE",|db,c|{
            require(db,&c)?;
            if advance{
                original.confirm(db,&c)?;original.confirmed_position(db,&c)?;
            }else{
                original.abort(db,&c,authority,if started{2}else{1})?;original.placed_at(db,&c,false)?;
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
#[path = "../../tests/unit/attached/system/directory_relabel/tests.rs"]
mod tests;
