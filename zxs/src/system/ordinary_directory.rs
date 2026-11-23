//! The same SYSTEM authority selects a real ordinary parent and its original
//! directory births. Neither physical names nor an old operation grant SMB.
use super::{Catalog,Connection,InitialReader,Tree,check,denied};
use super::super::{account::share_mode::Setting,file_location::Place,volume::OrdinaryRoot,work_record};
use crate::{FileId,DirectoryId,EntryId,OperationId,PoolId,Role,VolumeId,display,
    fs::{DIRECTORY_BYTES,DIRECTORY_NAMES,HeldDir,MAX_PATH_DEPTH,invalid,path_budget,
        directory_birth::{DIRECTORY_MARK,DirectoryBirth,DirectoryHold,DirectoryParent}},
    hash::Sha256,identity::{AccountId,ShareId},ordinary::portable_birth::Birth,system::ROOT_MARK};
use std::{collections::BTreeSet,ffi::{CStr,CString},io,path::PathBuf};

pub(super) fn components(raw:&[u8])->io::Result<Vec<String>>{
    let text=std::str::from_utf8(raw).map_err(|_|denied())?;
    let names:Vec<_>=text.split('\\').map(|x|display::component(x.as_bytes(),255).map(str::to_owned))
        .take(MAX_PATH_DEPTH+1).collect::<io::Result<_>>()?;
    if raw.len()>4096||names.is_empty()||names.len()>MAX_PATH_DEPTH{return Err(denied());}
    Ok(names)
}
fn busy()->io::Error{io::Error::new(io::ErrorKind::WouldBlock,"original ordinary name is reserved but unconfirmed")}
fn exists()->io::Error{io::Error::from(io::ErrorKind::AlreadyExists)}

/// A directory's current parent/spelling and its immutable marker are
/// different facts. The initial position still is its original birth.
#[derive(Clone,Debug,Eq,PartialEq)]
pub(super) struct DirectoryPlace{birth:DirectoryBirth,parent:DirectoryParent,name:String,position:OperationId,generation:i64}
impl DirectoryPlace{
    fn born(birth:DirectoryBirth)->Self{
        let parent=birth.parent;let name=birth.name.clone();let position=birth.original;
        Self{birth,parent,name,position,generation:1}
    }
    pub(super) fn name(&self)->&str{&self.name}
    pub(super) fn birth(&self)->&DirectoryBirth{&self.birth}
    /// The enclosing directory of the currently selected position, not the
    /// immutable parent encoded in its on-media marker.
    pub(super) fn containing(&self)->DirectoryParent{self.parent}
    pub(super) fn position(&self)->OperationId{self.position}
    pub(super) fn generation(&self)->i64{self.generation}
    pub(super) fn is_birth(&self,birth:&DirectoryBirth)->bool{
        self.generation==1 && self.position==birth.original && &self.birth==birth && self.name==birth.name&&self.parent==birth.parent
    }
    pub(super) fn open_selected(&self,parent:&HeldDir,names:&BTreeSet<Vec<u8>>)->io::Result<DirectoryHold>{
        self.birth.open_position(parent,names,&self.name)
    }
    fn open(&self,parent:&HeldDir)->io::Result<DirectoryHold>{
        self.open_selected(parent,&parent.names()?.into_iter().collect())
    }
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::system::reader) struct Route{pool:PoolId,volume:VolumeId,steps:Vec<DirectoryPlace>}
impl Route{
    pub(in crate::system::reader) fn at(db:&Connection,c:&Catalog,volume:VolumeId,parent:DirectoryParent)->io::Result<Self>{
        Self::settled_at(db,c,volume,parent)?.ok_or_else(busy)
    }
    /// None is an actual selected ancestor with an unsettled rename. A
    /// missing/malformed directory or SQLite BUSY remains an error.
    pub(in crate::system::reader) fn settled_at(db:&Connection,c:&Catalog,volume:VolumeId,parent:DirectoryParent)
        ->io::Result<Option<Self>>{
        if !c.rows().iter().any(|r|r.role==Role::File&&r.volume==volume){return Err(denied());}
        let mut steps=Vec::new();let mut seen=BTreeSet::new();let mut at=parent;let mut pending=false;
        while let DirectoryParent::Directory(id)=at{
            if !seen.insert(id){return Err(invalid("ordinary parent ancestry is cyclic"));}
            if steps.len()>=MAX_PATH_DEPTH{return Err(invalid("ordinary parent ancestry is too deep"));}
            let (selected,moving)=by_id(db,c,volume,id)?;
            pending|=moving;
            at=selected.containing();steps.push(selected);
        }
        steps.reverse();
        let path=Self{pool:c.birth.pool,volume,steps};
        path.geometry(c)?;Ok((!pending).then_some(path))
    }
    /// Wire labels are only lookups. This returns the same verified route as
    /// the DirectoryId construction, never an unqualified Unix path.
    pub(super) fn by_names(db:&Connection,c:&Catalog,volume:VolumeId,names:&[impl AsRef<str>])->io::Result<Self>{
        if names.len()>MAX_PATH_DEPTH||!c.rows().iter().any(|r|r.role==Role::File&&r.volume==volume){return Err(denied());}
        let(mut steps,mut at)=(Vec::with_capacity(names.len()),DirectoryParent::Root);
        for name in names{
            let place=lookup(db,c,volume,at,name.as_ref())?.ok_or(io::ErrorKind::NotFound)?;
            at=DirectoryParent::Directory(place.birth.id);steps.push(place);
        }
        let path=Self{pool:c.birth.pool,volume,steps};
        path.geometry(c)?;Ok(path)
    }
    pub(in crate::system::reader) fn volume(&self)->VolumeId{self.volume}
    pub(in crate::system::reader) fn parent(&self)->DirectoryParent{
        self.steps.last().map_or(DirectoryParent::Root,|s|DirectoryParent::Directory(s.birth.id))
    }
    pub(in crate::system::reader) fn logical_positions(&self)
        ->impl Iterator<Item=(DirectoryId,OperationId,i64,&str)>{
        self.steps.iter().map(|s|(s.birth.id,s.position,s.generation,s.name()))
    }
    pub(in crate::system::reader) fn visible_child(&self,db:&Connection,c:&Catalog,id:DirectoryId)
        ->io::Result<Option<(String,OperationId,i64)>>{
        match by_id(db,c,self.volume,id){
            Ok((d,false)) if d.containing()==self.parent()=>Ok(Some((d.name,d.position,d.generation))),
            Ok(_)=>Ok(None),
            Err(e) if e.kind()==io::ErrorKind::NotFound=>Ok(None),
            Err(e)=>Err(e),
        }
    }
    pub(super) fn selected(&self)->Option<&DirectoryPlace>{self.steps.last()}
    /// The current selected spelling at each exact DirectoryPlace, not its birth label.
    pub(super) fn components(&self)->impl Iterator<Item=&str>{self.steps.iter().map(DirectoryPlace::name)}
    pub(super) fn parent_components(&self)->impl Iterator<Item=&str>{
        self.components().take(self.steps.len().saturating_sub(1))
    }
    /// SMB-relative human labels only, for the chosen VolumeId in this
    /// database cut. No host path or file capability is returned.
    pub(in crate::system::reader) fn show_child(&self,name:&str)->String{
        self.steps.iter().map(DirectoryPlace::name).chain(std::iter::once(name)).collect::<Vec<_>>().join("\\")
    }
    pub(in crate::system::reader) fn geometry(&self,c:&Catalog)->io::Result<PathBuf>{
        let row=c.rows().iter().find(|r|r.role==Role::File&&r.volume==self.volume).ok_or_else(denied)?;
        if c.birth.pool!=self.pool{return Err(denied());}
        let mut path=row.path.clone();
        for step in &self.steps{
            if step.name.len()>usize::from(row.limit.encoded()){return Err(invalid("selected ordinary directory name is longer than its volume allows"));}
            path.push(&step.name);
        }
        path_budget(&path)?;Ok(path)
    }
    pub(in crate::system::reader) fn hold(&self,root:&HeldDir)->io::Result<Vec<DirectoryHold>>{
        let mut pins=Vec::<DirectoryHold>::with_capacity(self.steps.len());
        for step in &self.steps{
            let parent=pins.last().map_or(root,|h|h.directory());
            pins.push(step.open(parent)?);
        }
        keep_chain(root,&pins)?;Ok(pins)
    }
}
pub(in crate::system::reader) fn keep_chain(root:&HeldDir,pins:&[DirectoryHold])->io::Result<()>{
    let mut parent=root;
    for pin in pins{pin.require(parent)?;parent=pin.directory();}
    Ok(())
}

fn digest(b:&DirectoryBirth,actor:AccountId,auth:OperationId,epoch:i64,share:ShareId,setting:OperationId)->io::Result<[u8;32]>{
    b.valid()?;
    if epoch<1||[actor.0,auth.0,share.0,setting.0].contains(&[0;16]){return Err(denied());}
    let mut h=Sha256::new();h.update(b"ZXS-SQLITE-1/CREATE-DIRECTORY\0")?;
    for id in [b.pool.0,b.original.0,actor.0,auth.0]{h.update(&id)?;}
    h.update(&epoch.to_be_bytes())?;
    for id in [share.0,setting.0,b.volume.0,b.parent.bytes(),b.id.0,b.entry.0]{h.update(&id)?;}
    h.update(&[b.name.len() as u8])?;h.update(b.name.as_bytes())?;Ok(h.finish())
}
#[derive(Clone,Debug,Eq,PartialEq)]
struct Original{birth:DirectoryBirth,done:bool,aborted:bool,actor:AccountId,share:ShareId,setting:OperationId,auth:OperationId,epoch:i64}
fn original(db:&Connection,c:&Catalog,op:OperationId)->io::Result<Option<Original>>{
    let mut q=db.prepare(c"SELECT o.state,o.actor_id,o.request_sha,d.directory_id,d.entry_id,d.volume_id,d.parent_id,d.display,
        d.share_id,d.setting_op,d.auth_op,e.generation,a.smb_id,s.volume_id,m.share_id,m.mode,
        EXISTS(SELECT 1 FROM main.directory_abort a WHERE a.create_op=d.create_op)
      FROM main.directory_entry d JOIN main.operation o ON o.op_id=d.create_op AND o.kind=13
      JOIN main.credential_epoch e ON e.role=1 AND e.credential_op=d.auth_op JOIN main.account_pair a ON a.singleton=e.slot
      JOIN main.root_share s ON s.share_id=d.share_id JOIN main.share_mode m ON m.change_op=d.setting_op WHERE d.create_op=?1")?;
    q.bind_bytes(1,&op.0)?;
    if !work_record::start_one(&q)?{return Ok(None);}
    let state=q.byte(0)?;let actor=AccountId(q.array(1)?);
    let birth=DirectoryBirth{pool:c.birth.pool,volume:VolumeId(q.array(5)?),parent:DirectoryParent::from_bytes(q.array(6)?),
        id:DirectoryId(q.array(3)?),entry:EntryId(q.array(4)?),original:op,name:display::component(q.row()?.blob(7,255)?,255)?.to_owned()};
    let share=ShareId(q.array(8)?);let setting=OperationId(q.array(9)?);let auth=OperationId(q.array(10)?);let epoch=q.integer(11)?;
    let aborted=q.byte(16)?==1;
    if state>1||(aborted&&state!=0)||q.array::<16>(12)?!=actor.0||q.array::<16>(13)?!=birth.volume.0||
       q.array::<16>(14)?!=share.0||q.byte(15)?!=2||q.array::<32>(2)?!=digest(&birth,actor,auth,epoch,share,setting)?{
        return Err(invalid("directory differs from its actual original SQLite birth"));}
    q.done()?;Ok(Some(Original{birth,done:state==1,aborted,actor,share,setting,auth,epoch}))
}
fn published(db:&Connection,c:&Catalog,op:OperationId,position:OperationId,generation:i64,parent:DirectoryParent,name:String)
    ->io::Result<DirectoryPlace>{
    let birth=original(db,c,op)?.filter(|b|b.done&&!b.aborted).ok_or_else(denied)?.birth;
    if generation==1&&position==op&&parent==birth.parent&&name==birth.name{return Ok(DirectoryPlace::born(birth));}
    let rename=directory_rename::selected(db,c,position)?
        .filter(|r|r.confirmed&&!r.aborted&&r.birth==birth&&r.target_parent()==parent&&r.source_gen.checked_add(1)==Some(generation)&&r.target==name)
        .ok_or_else(||invalid("directory position differs from its exact confirmed rename"))?;
    Ok(DirectoryPlace{birth:rename.birth,parent,name,position,generation})
}
fn by_id(db:&Connection,c:&Catalog,volume:VolumeId,id:DirectoryId)->io::Result<(DirectoryPlace,bool)>{
    let mut q=db.prepare(c"SELECT d.create_op,d.position_op,d.generation,d.display,d.parent_id,
        EXISTS(SELECT 1 FROM main.pending_directory_rename m WHERE m.directory_id=d.directory_id AND m.source_op=d.position_op)
      FROM main.directory_selected d WHERE d.directory_id=?1 AND d.volume_id=?2 AND d.state=1")?;
    q.bind_bytes(1,&id.0)?;q.bind_bytes(2,&volume.0)?;
    if !work_record::start_one(&q)?{return Err(io::ErrorKind::NotFound.into());}
    let op=OperationId(q.array(0)?);let position=OperationId(q.array(1)?);let generation=q.integer(2)?;
    let name=display::component(q.row()?.blob(3,255)?,255)?.to_owned();let parent=DirectoryParent::from_bytes(q.array(4)?);let moving=q.byte(5)?==1;q.done()?;
    drop(q);
    let at=published(db,c,op,position,generation,parent,name)?;
    if at.birth.id!=id||at.birth.volume!=volume{return Err(denied());}
    if moving{
        let q=db.prepare(c"SELECT rename_op FROM main.pending_directory_rename
             WHERE directory_id=?1 AND source_op=?2 LIMIT 2")?;
        q.bind_bytes(1,&id.0)?;q.bind_bytes(2,&position.0)?;
        if !work_record::start_one(&q)?{return Err(invalid("pending ancestor selection disappeared"));}
        let rename=OperationId(q.array(0)?);q.done()?;drop(q); // exact one, never pick a representative
        directory_rename::selected(db,c,rename)?
            .filter(|r|!r.confirmed&&!r.aborted&&r.birth==at.birth&&r.source==position&&
                    r.source_gen==generation&&r.source_parent()==parent&&r.old==at.name)
            .ok_or_else(||invalid("pending ancestor does not have its exact original rename"))?;
    }
    Ok((at,moving))
}
pub(super) fn lookup(db:&Connection,c:&Catalog,volume:VolumeId,parent:DirectoryParent,name:&str)->io::Result<Option<DirectoryPlace>>{
    display::component(name.as_bytes(),255)?;
    let q=db.prepare(c"SELECT create_op,position_op,generation,COALESCE(state,0) FROM main.directory_slot
      WHERE volume_id=?1 AND parent_id=?2 AND display=?3")?;
    q.bind_bytes(1,&volume.0)?;q.bind_bytes(2,&parent.bytes())?;q.bind_bytes(3,name.as_bytes())?;
    if !work_record::start_one(&q)?{return Ok(None);}
    let op=OperationId(q.array(0)?);let position=OperationId(q.array(1)?);let generation=q.integer(2)?;
    let done=q.byte(3)?==1;q.done()?;
    if !done{return Err(busy());}
    let at=published(db,c,op,position,generation,parent,name.into())?;
    if at.birth.volume!=volume||at.containing()!=parent{return Err(denied());}
    Ok(Some(at))
}

/// Same-slot scope for a pending or published directory, an ordinary FILE or
/// an incoming restore. The two media representations cannot bypass it.
fn directory_vacant(db:&Connection,c:&Catalog,parent:&Route,name:&str)->io::Result<()>{
    if lookup(db,c,parent.volume,parent.parent(),name)?.is_some(){return Err(exists());}
    if super::super::file_location::Place::renaming_name(db,parent.volume,parent.parent(),name)?{return Err(busy());}
    // An SMB directory may have a wider leaf than any ordinary FILE can have.
    if name.len()<=222&&super::super::file_location::Place::named(db,c,parent.volume,parent.parent(),name)?.is_some(){return Err(exists());}
    // A different business leaf may be the original physical FILE's
    // terminal. Protect that promised name even before either link exists.
    let q=db.prepare(c"SELECT 1 FROM main.file_location p
        WHERE p.volume_id=?1 AND p.parent_id=?2 AND p.flat_terminal=?3 AND p.role=2")?;
    q.bind_bytes(1,&parent.volume.0)?;q.bind_bytes(2,&parent.parent().bytes())?;q.bind_bytes(3,name.as_bytes())?;
    if work_record::start_one(&q)?{return Err(exists());}
    Ok(())
}
fn pending_at(db:&Connection,c:&Catalog,route:&Route,name:&str)->io::Result<Option<Original>>{
    let q=db.prepare(c"SELECT create_op,COALESCE(state,0),moving IS NOT NULL FROM main.directory_slot
             WHERE volume_id=?1 AND parent_id=?2 AND display=?3")?;
    q.bind_bytes(1,&route.volume.0)?;q.bind_bytes(2,&route.parent().bytes())?;q.bind_bytes(3,name.as_bytes())?;
    if !work_record::start_one(&q)?{return Ok(None);}
    let op=OperationId(q.array(0)?);let done=q.byte(1)?==1;let moving=q.byte(2)?==1;q.done()?;
    if moving{return Err(busy());}
    if done{return Err(exists());}
    let p=original(db,c,op)?.filter(|x|!x.aborted&&!x.done).ok_or_else(denied)?;
    if p.birth.volume!=route.volume||p.birth.parent!=route.parent()||p.birth.name!=name{return Err(denied());}
    Ok(Some(p))
}
fn exact_original(at:&DirectoryAt<'_>,db:&Connection,c:&Catalog,op:OperationId,parent:DirectoryParent,name:&str)
    ->io::Result<Option<Original>>{
    let g=at.setting;
    let x=original(db,c,op)?;
    if x.as_ref().is_some_and(|x|
        x.birth.parent!=parent||x.birth.name!=name||x.birth.volume!=g.volume||x.share!=g.share||
        x.setting!=g.op||x.auth!=at.auth||x.actor!=at.actor||x.epoch!=at.epoch){return Err(denied());}
    Ok(x)
}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) enum Cut{Reserved,Installed}
#[derive(Debug)]
pub(super) enum NamedKind{Directory(Route),File}

/// Select only the business type at the original catalog parent. The caller
/// must authorize this same read cut and later acquire the actual medium.
pub(super) fn kind_at(db:&Connection,c:&Catalog,volume:VolumeId,parents:&[impl AsRef<str>],name:&str)
    ->io::Result<NamedKind>{
    let mut route=Route::by_names(db,c,volume,parents)?;
    let dir=lookup(db,c,route.volume,route.parent(),name)?;
    let file=if name.len()<=222{Place::named(db,c,route.volume,route.parent(),name)?}else{None};
    match(dir,file){
        (Some(d),None)=>{route.steps.push(d);route.geometry(c)?;Ok(NamedKind::Directory(route))},
        (None,Some(f))=>{let _=f.bind(db,c)?;Ok(NamedKind::File)},
        (None,None)=>Err(io::ErrorKind::NotFound.into()),
        (Some(_),Some(_))=>Err(invalid("two original types claim one ordinary name")),
    }
}
impl<'s,'r> Tree<'s,'r>{
    /// Each name is resolved against one current, authenticated SQL cut.
    pub(super) fn ordinary_route(&self,names:&[String],write:bool)->io::Result<(Catalog,Route)>{
        self.session.source.at(|db,c|{
            self.require(db,&c,write)?;
            let route=Route::by_names(db,&c,self.setting.volume,names)?;Ok((c,route))
        })
    }
    /// An existing-only, untyped SMB open first selects a definite kind at
    /// the exact ordinary parent. A missing/bad disk never picks a FILE by
    /// exclusion, nor does a kind selection issue a native handle or grant.
    pub(super) fn ordinary_kind(&self,names:&[String])->io::Result<(Catalog,NamedKind)>{
        if names.len()>MAX_PATH_DEPTH{return Err(denied());}
        let (name,parents)=names.split_last().ok_or_else(denied)?;
        self.session.source.at(|db,c|{
            self.require(db,&c,false)?;
            let kind=kind_at(db,&c,self.setting.volume,parents,name)?;
            Ok((c,kind))
        })
    }
    /// A new SMB create can continue an earlier selection only by its actual
    /// original id and currently authorized source, not by a lookalike name.
    pub(super) fn resumable_directory(&self,parent:&Route,name:&str)->io::Result<Option<OperationId>>{
        let source=self.session.source;let g=self.setting;let a=&self.session.original;
        if parent.volume!=g.volume{return Err(denied());}
        let(c,pending)=source.at(|db,c|{
            self.require(db,&c,true)?;
            if Route::at(db,&c,g.volume,parent.parent())?!=*parent{return Err(denied());}
            let old=pending_at(db,&c,parent,name)?.filter(|p|p.actor==a.account&&p.auth==a.op&&p.epoch==a.generation&&
                p.setting==g.op&&p.share==g.share);
            Ok((c,old))
        })?;
        let Some(old)=pending else{return Ok(None);};
        let actual=OrdinaryRoot::at_parent(source,&c,parent,None)?;
        if !old.birth.resumable(actual.folder())?{return Ok(None);}
        actual.require_streaming()?;
        source.at(|db,c|{
            self.require(db,&c,true)?;
            if pending_at(db,&c,parent,name)?.as_ref()!=Some(&old){return Err(busy());}
            Ok(Some(old.birth.original))
        })
    }
    /// Offer the original id to the native share-mode gate before any disk
    /// mutation. This method returns no DirectoryHold on an unknown outcome.
    pub(super) fn make_ordinary_directory(&self,route:&Route,op:OperationId,name:&str,
        offer:impl FnOnce(&DirectoryBirth)->io::Result<()>,live:&impl Fn()->io::Result<()>,
        cut:impl FnMut(Cut)->io::Result<()>)->io::Result<(DirectoryBirth,DirectoryHold)>{
        let a=&self.session.original;
        let require=|cut:&DirectoryCreateCut<'_>|{
            debug_assert!(std::ptr::eq(cut.source,self.session.source));
            self.require(cut.db,cut.catalog,true)
        };
        let at=DirectoryAt{source:self.session.source,setting:self.setting,actor:a.account,auth:a.op,
            epoch:a.generation,require:&require,earlier:EarlierBirth::Native};
        at.make(route,op,name,offer,live,cut)
    }
}

/// Short exact source and account check, inside the same original SQLite
/// transactions. This does not issue a namespace or a live tree.
pub(super) struct DirectoryCreateCut<'a>{source:&'a InitialReader,db:&'a Connection,catalog:&'a Catalog}
impl<'a> DirectoryCreateCut<'a>{
    pub(super) fn parts(&self)->(&'a InitialReader,&'a Connection,&'a Catalog){(self.source,self.db,self.catalog)}
}
#[derive(Clone,Copy,Eq,PartialEq)]
enum EarlierBirth{Native,Fresh}
struct DirectoryAt<'a>{
    source:&'a InitialReader,setting:Setting,actor:AccountId,auth:OperationId,epoch:i64,
    require:&'a dyn Fn(&DirectoryCreateCut<'_>)->io::Result<()>,
    earlier:EarlierBirth,
}
impl DirectoryAt<'_>{
    fn authorized(&self,db:&Connection,catalog:&Catalog)->io::Result<()>{
        (self.require)(&DirectoryCreateCut{source:self.source,db,catalog})
    }
    fn make(&self,route:&Route,op:OperationId,name:&str,
        offer:impl FnOnce(&DirectoryBirth)->io::Result<()>,live:&impl Fn()->io::Result<()>,
        mut cut:impl FnMut(Cut)->io::Result<()>)->io::Result<(DirectoryBirth,DirectoryHold)>{
        let src=self.source;let g=self.setting;let parent=route.parent();
        if route.volume!=g.volume{return Err(denied());}
        display::component(name.as_bytes(),255)?;
        let (catalog,old,superseded)=src.at(|db,c|{
            self.authorized(db,&c)?;live()?;
            if Route::at(db,&c,g.volume,parent)?!=*route{return Err(denied());}
            let old=exact_original(self,db,&c,op,parent,name)?;
            let earlier=if old.is_none(){pending_at(db,&c,route,name)?}else{None};
            // An unrelated fresh Rust request never authorizes the
            // Native recovery/abandonment path for an earlier operation.
            if self.earlier==EarlierBirth::Fresh&&(old.is_some()||earlier.is_some()){return Err(busy());}
            if old.is_none()&&earlier.is_none(){directory_vacant(db,&c,route,name)?;}
            Ok((c,old,earlier))
        })?;
        let root=OrdinaryRoot::at_parent(src,&catalog,route,Some(true))?;
        let d=root.folder();let from_disk=||d.names().map(|v|v.into_iter().collect::<BTreeSet<_>>());
        if let Some(prior)=&superseded{
            let (terminal,paired)=prior.birth.pending_shape(d,&from_disk()?)?;
            if terminal{return Err(busy());}
            let lost=prior.auth!=self.auth||prior.setting!=g.op;
            if paired&&!lost{return Err(busy());}
            let reason=if lost{1}else{2};
            root.require_streaming()?;
            let sql=Connection::open(&src.dir,true)?;
            src.transaction(&sql,c"BEGIN IMMEDIATE",|db,c|{
                self.authorized(db,&c)?;
                if pending_at(db,&c,route,name)?.as_ref()!=Some(prior){return Err(denied());}
                live()?;
                let x=db.prepare(c"INSERT INTO main.directory_abort(create_op,setting_op,auth_op,reason) VALUES(?1,?2,?3,?4)")?;
                x.bind_bytes(1,&prior.birth.original.0)?;x.bind_bytes(2,&g.op.0)?;x.bind_bytes(3,&self.auth.0)?;x.bind_integer(4,reason)?;x.done()
            })?;
        }
        let birth=match &old{
            Some(prior) if !prior.aborted=>prior.birth.clone(),
            Some(_)=>return Err(denied()),
            None=>DirectoryBirth{pool:src.pool,volume:g.volume,parent,id:DirectoryId::new()?,entry:EntryId::new()?,original:op,name:name.into()},
        };
        birth.validate_geometry(d)?;
        if old.is_none(){birth.new_vacancy(d,&from_disk()?)?;}
        if let Some(prior)=&old{if prior.done{
            let held=birth.open(d)?;root.require_streaming()?;
            src.at(|db,c|{self.authorized(db,&c)?;live()?;
                if exact_original(self,db,&c,op,parent,name)?.as_ref()!=Some(prior){return Err(denied());}Ok(())})?;
            return Ok((birth,held)); // already published; never a native offer
        }}
        live()?;offer(&birth)?;live()?;
        let sha=digest(&birth,self.actor,self.auth,self.epoch,g.share,g.op)?;
        if old.is_none(){
            let census=from_disk()?;birth.new_vacancy(d,&census)?;root.require_streaming()?;
            let sql=Connection::open(&src.dir,true)?;
            let reserve=src.transaction(&sql,c"BEGIN IMMEDIATE",|db,c|{
                self.authorized(db,&c)?;
                if Route::at(db,&c,g.volume,parent)?!=*route||exact_original(self,db,&c,op,parent,name)?.is_some(){return Err(denied());}
                directory_vacant(db,&c,route,name)?;budget(db,g.volume,parent,&census,Some(Additional::Directory(&birth)))?;
                live()?;
                let q=db.prepare(c"INSERT INTO main.directory_entry(create_op,directory_id,entry_id,volume_id,parent_id,display,share_id,setting_op,auth_op)
                                              VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)")?;
                for(i,v)in [op.0.as_slice(),birth.id.0.as_slice(),birth.entry.0.as_slice(),g.volume.0.as_slice(),
                            parent.bytes().as_slice(),name.as_bytes(),g.share.0.as_slice(),g.op.0.as_slice(),self.auth.0.as_slice()
                           ].into_iter().enumerate(){q.bind_bytes(i as i32+1,v)?;}
                q.done()?;
                let q=db.prepare(c"INSERT INTO main.operation(op_id,kind,actor_id,request_sha,state) VALUES(?1,13,?2,?3,0)")?;
                q.bind_bytes(1,&op.0)?;q.bind_bytes(2,&self.actor.0)?;q.bind_bytes(3,&sha)?;q.done()
            });
            drop(sql);
            if let Err(e)=reserve{
                let selected=src.at(|db,c|{self.authorized(db,&c)?;exact_original(self,db,&c,op,parent,name)})?;
                if !selected.is_some_and(|x|!x.done&&!x.aborted&&x.birth==birth){return Err(e);}
            }
        }
        cut(Cut::Reserved)?;live()?;
        let held=birth.install(d)?;cut(Cut::Installed)?;
        held.require_newborn(d)?;root.require_streaming()?;
        let sql=Connection::open(&src.dir,true)?;
        let confirm=src.transaction(&sql,c"BEGIN IMMEDIATE",|db,c|{
            self.authorized(db,&c)?;
            if Route::at(db,&c,g.volume,parent)?!=*route||
               !exact_original(self,db,&c,op,parent,name)?.is_some_and(|x|!x.done&&!x.aborted&&x.birth==birth){return Err(denied());}
            live()?;
            let q=db.prepare(c"UPDATE main.operation SET state=1 WHERE op_id=?1 AND kind=13 AND state=0 RETURNING op_id")?;
            q.bind_bytes(1,&op.0)?;
            if !work_record::start_one(&q)?||q.array::<16>(0)?!=op.0{return Err(denied());}q.done()
        });
        drop(sql);
        if let Err(e)=confirm{
            let selected=src.at(|db,c|{self.authorized(db,&c)?;exact_original(self,db,&c,op,parent,name)})?;
            if !selected.is_some_and(|x|x.done&&!x.aborted&&x.birth==birth){return Err(e);}
        }
        Ok((birth,held))
    }
}

#[path="smb_rust_directory_birth.rs"]
mod rust_birth;
pub(crate) use rust_birth::RustNamedDirectory;

pub(super) enum Additional<'a>{
    File(&'a Birth),Directory(&'a DirectoryBirth),Pocket(&'a CStr),Rename(FileId,&'a CStr),IncomingFlat(&'a CStr,&'a CStr),
    RenameDirectory(DirectoryId,&'a CStr),IncomingDirectory(&'a DirectoryBirth,&'a CStr),
}
struct NameBudget{unaccounted:BTreeSet<Vec<u8>>,promised:BTreeSet<Vec<u8>>,count:usize,bytes:usize}
impl NameBudget{
    fn charge(&mut self,count:usize,bytes:usize)->io::Result<()>{
        self.count+=count;self.bytes+=bytes;
        if self.count>DIRECTORY_NAMES||self.bytes>DIRECTORY_BYTES{return Err(io::ErrorKind::StorageFull.into());}Ok(())
    }
    fn pair(&mut self,pin:&CStr,stage:&CStr,terminal:&[u8],pending:bool)->io::Result<()>{
        if terminal.is_empty()||terminal.len()>255{return Err(invalid("invalid reserved native terminal"));}
        self.claim(pin.to_bytes())?;self.claim(terminal)?;
        if pending{self.claim(stage.to_bytes())?;}
        self.unaccounted.remove(pin.to_bytes());let has_end=self.unaccounted.remove(terminal);
        if pending&&self.unaccounted.remove(stage.to_bytes())&&has_end{return Err(invalid("two native names claim the same birth slot"));}
        self.charge(2,2+pin.to_bytes().len()+if pending{terminal.len().max(stage.to_bytes().len())}else{terminal.len()})
    }
    fn incoming_pair(&mut self,pin:&CStr,end:&CStr)->io::Result<()>{
        self.claim(pin.to_bytes())?;self.claim(end.to_bytes())?;
        self.unaccounted.remove(pin.to_bytes());self.unaccounted.remove(end.to_bytes());
        self.charge(2,2+pin.to_bytes().len()+end.to_bytes().len())
    }
    fn rename_edge(&mut self,pin:&CStr,old:&[u8],new:&[u8])->io::Result<()>{
        if old.is_empty()||old.len()>255||new.is_empty()||new.len()>255{return Err(invalid("invalid original rename terminal"));}
        self.claim(pin.to_bytes())?;self.claim(old)?;self.claim(new)?;
        self.unaccounted.remove(pin.to_bytes());
        let a=self.unaccounted.remove(old);let b=self.unaccounted.remove(new);
        if a&&b{return Err(invalid("two native names claim one same-parent rename"));}
        self.charge(2,2+pin.to_bytes().len()+old.len().max(new.len()))
    }
    fn claim(&mut self,name:&[u8])->io::Result<()>{
        if !self.promised.insert(name.to_vec()){return Err(exists());}Ok(())
    }
    fn pocket(&mut self,pocket:&CStr)->io::Result<()>{
        self.claim(pocket.to_bytes())?;self.unaccounted.remove(pocket.to_bytes());self.charge(1,1+pocket.to_bytes().len())
    }
    fn finish(mut self)->io::Result<()>{
        let n=self.unaccounted.len();let b=self.unaccounted.iter().map(|n|n.len()+1).sum();self.charge(n,b)
    }
}
/// This one short metadata budget covers ordinary FILEs, native directory
/// pairs and incoming restore pockets under the same exact held parent.
pub(super) fn budget(db:&Connection,volume:VolumeId,parent:DirectoryParent,census:&BTreeSet<Vec<u8>>,more:Option<Additional<'_>>)->io::Result<()>{
    let marker=if parent==DirectoryParent::Root{ROOT_MARK}else{DIRECTORY_MARK};
    if !census.contains(marker.to_bytes()){return Err(invalid("selected ordinary parent has no original marker"));}
    let mut b=NameBudget{unaccounted:census.clone(),promised:BTreeSet::new(),count:0,bytes:0};
    let mut q=db.prepare(c"SELECT e.create_op,e.ticket,l.flat_terminal,o.state,l.file_id,r.target_terminal FROM main.file_location l
        JOIN main.root_entry e ON e.file_id=l.file_id JOIN main.operation o ON o.op_id=e.create_op
        LEFT JOIN main.pending_rename r ON r.file_id=l.file_id AND r.source_op=l.position_op AND r.target_parent=l.parent_id
        WHERE l.role=2 AND l.flat_terminal IS NOT NULL AND l.volume_id=?1 AND l.parent_id=?2")?;
    q.bind_bytes(1,&volume.0)?;q.bind_bytes(2,&parent.bytes())?;
    let mut charged_rename=false;
    loop{match q.step(){
        100=>{let row=q.row()?;let (pin,stage)=Birth::reservation_leaves(OperationId(row.array(0)?),OperationId(row.array(1)?))?;
             let old=row.blob(2,255)?;
             let requested=match &more{
                 Some(Additional::Rename(file,new)) if file.0==row.array::<16>(4)?=>Some(new.to_bytes()),
                 _=>None,
             };
             if let Some(new)=requested{
                 if !row.is_null(5){return Err(busy());}
                 charged_rename=true;b.rename_edge(&pin,old,new)?;
             }else if !row.is_null(5){
                 b.rename_edge(&pin,old,row.blob(5,255)?)?;
             }else{b.pair(&pin,&stage,old,row.byte(3)?==0)?;}
        }
        101=>break,n=>{check(n)?;return Err(denied());}
    }}
    let mut q=db.prepare(c"SELECT d.create_op,d.display,COALESCE(d.state,0),d.directory_id,r.target_name,r.target_parent
         FROM main.directory_selected d LEFT JOIN main.pending_directory_rename r
           ON r.directory_id=d.directory_id AND r.source_op=d.position_op AND r.source_parent=d.parent_id WHERE d.volume_id=?1 AND d.parent_id=?2")?;
    q.bind_bytes(1,&volume.0)?;q.bind_bytes(2,&parent.bytes())?;
    let mut charged_directory=false;
    loop{match q.step(){
        100=>{let row=q.row()?;
            let op=OperationId(row.array(0)?);let(pin,stage)=dir_reserved(op)?;let old=row.blob(1,255)?;
            let requested=match &more{
                Some(Additional::RenameDirectory(id,name)) if id.0==row.array::<16>(3)?=>Some(name.to_bytes()),
                _=>None,
            };
            let pending=!row.is_null(4);
            if let Some(new)=requested{
                if pending||row.byte(2)?!=1{return Err(busy());}
                charged_directory=true;b.rename_edge(&pin,old,new)?;
            }else if pending{
                if row.byte(2)?!=1{return Err(invalid("pending move did not follow an installed directory"));}
                if row.array::<16>(5)?==parent.bytes(){b.rename_edge(&pin,old,row.blob(4,255)?)?;}else{b.pair(&pin,&stage,old,false)?;}
            }else{b.pair(&pin,&stage,old,row.byte(2)?==0)?;}
        },
        101=>break,n=>{check(n)?;return Err(denied());}
    }}
    let mut q=db.prepare(c"SELECT d.create_op,r.target_name FROM main.operation o
        JOIN main.directory_rename r ON r.rename_op=o.op_id
        JOIN main.directory_selected d ON d.directory_id=r.directory_id AND d.position_op=r.source_op
             AND d.parent_id=r.source_parent AND d.state=1
        WHERE o.kind=16 AND o.state=0 AND d.volume_id=?1 AND r.target_parent=?2 AND r.source_parent<>r.target_parent
          AND NOT EXISTS(SELECT 1 FROM main.directory_rename_abort a WHERE a.rename_op=r.rename_op)")?;
    q.bind_bytes(1,&volume.0)?;q.bind_bytes(2,&parent.bytes())?;
    loop{match q.step(){
        100=>{
            let row=q.row()?;
            let (pin,_)=dir_reserved(OperationId(row.array(0)?))?;
            let target=display::component(row.blob(1,255)?,255)?;
            let end=CString::new(target).map_err(|_|denied())?;
            b.incoming_pair(&pin,&end)?;
        },
        101=>break,n=>{check(n)?;return Err(denied());}
    }}
    // A two-parent move bills both parents for all of that FILE's own
    // names until selection/abort, including the source rollback room.
    let mut q=db.prepare(c"SELECT b.create_op,b.ticket,r.file_id,r.target_name,r.target_terminal FROM main.pending_rename r
        JOIN main.root_entry b ON b.file_id=r.file_id
        WHERE r.volume_id=?1 AND r.target_parent=?2 AND r.parent_id<>r.target_parent AND r.backing_op IS NULL")?;
    q.bind_bytes(1,&volume.0)?;q.bind_bytes(2,&parent.bytes())?;
    loop{match q.step(){
        100=>{let row=q.row()?;
            let file=FileId(row.array(2)?);
            let name=crate::display::component(row.blob(3,222)?,222)?;
            let terminal=Birth::placed_container(name,file)?;
            if terminal.to_bytes()!=row.blob(4,255)?{return Err(invalid("wrong original incoming FILE terminal"));}
            let(pin,_)=Birth::reservation_leaves(OperationId(row.array(0)?),OperationId(row.array(1)?))?;
            b.incoming_pair(&pin,&terminal)?;
        },
        101=>break,n=>{check(n)?;return Err(denied());}
    }}
    let q=db.prepare(c"SELECT backing_op FROM main.file_location WHERE role=2 AND backing_op IS NOT NULL AND volume_id=?1 AND parent_id=?2
        UNION ALL SELECT transfer_op FROM main.pending_transfer WHERE direction IN (2,3) AND target_id=?1 AND target_parent=?2
        UNION ALL SELECT backing_op FROM main.pending_rename WHERE volume_id=?1 AND target_parent=?2
              AND parent_id<>target_parent AND backing_op IS NOT NULL")?;
    q.bind_bytes(1,&volume.0)?;q.bind_bytes(2,&parent.bytes())?;
    loop{match q.step(){
        100=>b.pocket(&Birth::placement_parent(OperationId(q.array(0)?))?)?,
        101=>break,n=>{check(n)?;return Err(denied());}
    }}
    match more{
        Some(Additional::File(f))=>b.pair(&f.witness()?,&f.lower(0)?,f.container()?.to_bytes(),true)?,
        Some(Additional::Directory(d))=>b.pair(&d.witness(),&d.stage(),d.leaf()?.to_bytes(),true)?,
        Some(Additional::Pocket(p))=>b.pocket(p)?,
        Some(Additional::IncomingFlat(pin,end))=>b.incoming_pair(pin,end)?,
        Some(Additional::IncomingDirectory(dir,end))=>b.incoming_pair(&dir.witness(),end)?,
        Some(Additional::Rename(..)) if !charged_rename=>return Err(invalid("no exact flat FILE for this rename budget")),
        Some(Additional::Rename(..))=>{},
        Some(Additional::RenameDirectory(..)) if !charged_directory=>return Err(invalid("no original directory for this rename budget")),
        Some(Additional::RenameDirectory(..))=>{},
        None=>{}
    }
    b.finish()
}
fn dir_reserved(op:OperationId)->io::Result<(CString,CString)>{
    if [ [0;16],[0xff;16] ].contains(&op.0){return Err(denied());}
    let pin=CString::new(format!(".zxs-new-{op}-7")).map_err(|_|denied())?;
    let stage=CString::new(format!(".zxs-new-{op}-4")).map_err(|_|denied())?;
    Ok((pin,stage))
}

#[path="directory_relabel.rs"]
mod directory_rename;
pub(super) use directory_rename::{ConfirmedRelabel,DirectoryRelabelCut,RelabelCut,relabel_rust,recovery_loop as recover_relabels};

#[cfg(test)]
#[path = "../../tests/unit/attached/system/ordinary_directory/tests.rs"]
mod tests;
