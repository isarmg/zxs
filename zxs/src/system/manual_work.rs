//! The one explicit manual publication action for an ordinary FILE. It
//! freezes the complete confirmed logical graph, not a physical whole copy.
//! No automatic caller, write grant or anonymous directory entry is created.
use super::{Catalog,Connection,InitialReader,WebAdmin,WebCurrent,denied};
use super::super::{file_location::{Place,Placed},smb::{directory,file_create,ordinary_directory::Route},volume::OrdinaryRoot,work_record::{self,Members,ReadRecord}};
use crate::{
    BranchId,FileId,HistoryId,NodeId,OperationId,PoolId,Role,VersionId,VolumeId,fs::{ControlRead,invalid},hash::Sha256,identity::AccountId,
    ordinary::{portable_birth::Birth,working_round::Selected},
};
use std::{collections::{BTreeMap,btree_map::Entry as Cached},io};
use super::super::SystemDir;

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(crate) struct Target {
    pub(crate) file:FileId,
    pub(crate) history:HistoryId,
    pub(crate) branch:BranchId,
    pub(crate) round:OperationId,
    pub(crate) save:OperationId,
    pub(crate) revision:u64,
    pub(crate) parent:Option<OperationId>,
}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(crate) struct Receipt {
    pub(crate) original:OperationId,
    pub(crate) node:NodeId,
    pub(crate) version:VersionId,
    pub(crate) ord:i64,
    pub(crate) target:Target,
}
/// Exact original S proof for a bounded worker, with no SQLite connection
/// or pathname selected by HTTP. The original reader keeps these OFDs live.
pub(crate) struct Site<'a>{original:&'a SystemDir,marker:&'a ControlRead,pool:PoolId,schema:[u8;32]}
#[derive(Clone,Copy)]
pub(crate) struct Who{account:AccountId,credential:OperationId,generation:i64}
impl InitialReader{
    pub(crate) fn manual_site(&self)->Site<'_>{Site{original:&self.dir,marker:&self.marker,pool:self.pool,schema:self.expected_schema}}
}
impl Site<'_>{
    pub(crate) fn freeze(&self,who:Who,op:OperationId,target:Target,authorized:impl Fn()->io::Result<()>)->io::Result<Receipt>{
        authorized()?;
        let src=InitialReader::alongside(self.original,self.marker,self.pool,self.schema)?;
        let admin=src.at(|db,c|{
            let current=WebCurrent::select(db,&c)?.ok_or_else(denied)?;
            if (current.account,current.op,current.generation)!=(who.account,who.credential,who.generation){return Err(denied());}
            Ok(WebAdmin{source:&src,account:who.account,origin:who.credential,generation:who.generation})
        })?;
        admin.freeze_authorized(op,target,authorized,||Ok(()))
    }
}
pub(crate) struct Item {pub(crate) file:FileId,pub(crate) name:String,pub(crate) volume:VolumeId}
fn busy(message:&str)->io::Error{io::Error::new(io::ErrorKind::WouldBlock,message.to_owned())}

struct Cut {
    birth:Birth,
    place:Place,
    selected:ReadRecord,
    target:Target,
    members:Members,
    alias:Option<file_create::Alias>,
    ord:i64,
}
fn target(db:&Connection,b:&Birth)->io::Result<(Target,ReadRecord,i64)>{
    if work_record::pending_current(db,b)?.is_some(){
        return Err(busy("original FILE still has an unsettled SAVE"));
    }
    let chosen=work_record::selected(db,b)?.ok_or_else(||busy("no confirmed working revision to freeze"))?;
    let latest=work_record::latest_commit(db,b)?;
    let k=chosen.record.key();
    if latest.is_some_and(|(_,_,round,_)|round==k.round){return Err(busy("this working round has already been frozen"));}
    let parent=latest.map(|(op,_,_,_)|op);
    let ord=latest.map_or(0,|(_,_,_,n)|n).checked_add(1).ok_or_else(||invalid("manual history order exhausted"))?;
    if ord<1{return Err(invalid("invalid current manual history"));}
    Ok((Target{file:b.file,history:b.history,branch:b.branch,round:k.round,save:k.save,revision:k.revision,parent},chosen,ord))
}
impl Cut{
    fn at(db:&Connection,c:&Catalog,birth:Birth,expected:Target)->io::Result<Self>{
        let place=ordinary_here(db,c,birth.file)?;
        let (target,selected,ord)=target(db,&birth)?;
        if target!=expected{return Err(busy("this FILE is no longer at the requested working revision"));}
        let members=work_record::members(db,c,&birth)?;
        let alias=file_create::display_owner_at(db,c,place.volume,place.parent,&place.name)?;
        Ok(Self{birth,place,selected,target,members,alias,ord})
    }
    fn require(&self,db:&Connection,c:&Catalog)->io::Result<()>{
        if file_create::committed_file(db,c,self.birth.file)?!=self.birth||ordinary_here(db,c,self.birth.file)?!=self.place||
           target(db,&self.birth)?!=(self.target,self.selected,self.ord)||
           work_record::members(db,c,&self.birth)?!=self.members||
           file_create::display_owner_at(db,c,self.place.volume,self.place.parent,&self.place.name)?!=self.alias{
            return Err(busy("the original manual cut changed before publication"));
        }Ok(())
    }
}

fn ordinary_here(db:&Connection,c:&Catalog,file:FileId)->io::Result<Place>{
    let p=Place::current(db,c,file)?;
    if p.role!=Role::File{return Err(io::ErrorKind::NotFound.into());}
    Ok(p)
}

impl WebAdmin<'_>{
    fn freeze_request(&self,op:OperationId,b:&Birth,t:Target)->io::Result<[u8;32]>{
        if [op.0,t.file.0,t.history.0,t.branch.0,t.round.0,t.save.0].contains(&[0;16])||
           t.parent.is_some_and(|p|p==op||p.0==[0;16])||op==t.save||t.revision==0||t.revision>i64::MAX as u64||
           (t.file,t.history,t.branch)!=(b.file,b.history,b.branch){
            return Err(invalid("wrong original manual publication target"));
        }
        let mut h=Sha256::new();h.update(b"ZXS-SQLITE-1/MANUAL-NODE\0")?;
        for id in [b.pool.0,op.0,self.account.0,self.origin.0,b.operation.0,b.volume.0,b.entry.0,
            t.file.0,t.history.0,t.branch.0,t.round.0,t.save.0]{h.update(&id)?;}
        h.update(&self.generation.to_be_bytes())?;h.update(&t.revision.to_be_bytes())?;
        h.update(&[u8::from(t.parent.is_some())])?;
        if let Some(p)=t.parent{h.update(&p.0)?;}
        Ok(h.finish())
    }
    fn freeze_result(&self,db:&Connection,op:OperationId,b:&Birth,t:Target)->io::Result<Option<Receipt>>{
        let expected=self.freeze_request(op,b,t)?;
        let q=db.prepare(c"SELECT o.kind,o.state,o.actor_id,o.request_sha,n.file_id,n.round_op,n.save_op,n.revision,n.auth_op,n.parent_op,n.ord,n.node_id,n.version_id FROM main.operation o LEFT JOIN main.manual_node n ON n.commit_op=o.op_id WHERE o.op_id=?1")?;
        q.bind_bytes(1,&op.0)?;
        if !work_record::start_one(&q)?{return Ok(None);}
        if q.byte(0)?!=11||q.byte(1)?!=1||q.array::<16>(2)?!=self.account.0||q.array::<32>(3)?!=expected||
           q.array::<16>(4)?!=t.file.0||q.array::<16>(5)?!=t.round.0||q.array::<16>(6)?!=t.save.0||
           work_record::natural(&q,7)?!=t.revision||q.array::<16>(8)?!=self.origin.0||work_record::optional(&q,9)?!=t.parent{
            return Err(invalid("operation belongs to a different original manual request or Web authority"));
        }
        let result=Receipt{original:op,node:NodeId(q.array(11)?),version:VersionId(q.array(12)?),ord:q.integer(10)?,target:t};
        if result.ord<1||[result.node.0,result.version.0].contains(&[0;16]){return Err(invalid("wrong original manual receipt"));}
        q.done()?;Ok(Some(result))
    }

    pub(crate) fn manual_author(&self)->Who{Who{account:self.account,credential:self.origin,generation:self.generation}}
    pub(crate) fn manual_target(&self,file:FileId)->io::Result<Target>{
        self.manual_preview(file).map(|(t,_)|t)
    }
    pub(crate) fn manual_preview(&self,file:FileId)->io::Result<(Target,Item)>{
        self.source.at(|db,c|{
            self.current(db,&c)?;
            let current=ordinary_here(db,&c,file)?;
            let b=file_create::committed_file(db,&c,file)?;
            let (choice,_,_)=target(db,&b)?;
            let parent=Route::at(db,&c,current.volume,current.parent)?;
            Ok((choice,Item{file:b.file,name:parent.show_child(&current.name),volume:current.volume}))
        })
    }
    /// Same confirmed original birth selector as ordinary directory
    /// metadata. Returns no share grant, host path or file content.
    pub(crate) fn manual_files(&self,after:Option<FileId>)->io::Result<(Vec<Item>,Option<FileId>)>{
        self.source.at(|db,c|{
            self.current(db,&c)?;
            const PAGE:usize=32;
            let q=db.prepare(c"SELECT l.file_id FROM main.file_location l JOIN main.operation o ON o.op_id=l.position_op AND o.state=1 WHERE l.role=2 AND l.file_id>?1 ORDER BY l.file_id LIMIT 33")?;
            q.bind_bytes(1,&after.unwrap_or(FileId([0;16])).0)?;
            let mut items=Vec::new();let mut parents=BTreeMap::new();
            while work_record::start_one(&q)? {
                let id=FileId(q.array(0)?);
                let now=ordinary_here(db,&c,id)?;
                let b=file_create::committed_file(db,&c,id)?;
                let key=(now.volume,now.parent.bytes());
                let parent=match parents.entry(key){
                    Cached::Occupied(r)=>r.into_mut(),
                    Cached::Vacant(r)=>r.insert(Route::at(db,&c,now.volume,now.parent)?),
                };
                items.push(Item{file:b.file,name:parent.show_child(&now.name),volume:now.volume});
            }
            let next=if items.len()>PAGE{items.truncate(PAGE);items.last().map(|i|i.file)}else{None};
            Ok((items,next))
        })
    }
    pub(crate) fn freeze(&self,op:OperationId,expected:Target)->io::Result<Receipt>{
        self.freeze_after(op,expected,||Ok(()))
    }
    pub(in crate::system::reader) fn freeze_after(&self,op:OperationId,expected:Target,after:impl FnOnce()->io::Result<()>)->io::Result<Receipt>{
        self.freeze_authorized(op,expected,||Ok(()),after)
    }
    fn freeze_authorized(&self,op:OperationId,expected:Target,authorized:impl Fn()->io::Result<()>,after:impl FnOnce()->io::Result<()>)->io::Result<Receipt>{
        authorized()?;let src=self.source;
        let (catalog,cut,done,route)=src.at(|db,c|{
            self.current(db,&c)?;
            let b=file_create::committed_file(db,&c,expected.file)?;
            if let Some(result)=self.freeze_result(db,op,&b,expected)?{return Ok((c,None,Some(result),None));}
            let cut=Cut::at(db,&c,b,expected)?;
            let route=super::super::smb::ordinary_directory::Route::at(db,&c,cut.place.volume,cut.place.parent)?;
            Ok((c,Some(cut),None,Some(route)))
        })?;
        if let Some(old)=done{authorized()?;return Ok(old);} // historical receipt, not a disk check
        let cut=cut.ok_or_else(||invalid("missing exact new manual target"))?;
        let root=OrdinaryRoot::at_parent(src,&catalog,&route.ok_or_else(||invalid("missing selected manual parent"))?,None)?;
        let names=root.folder().names()?.into_iter().collect();
        let placed=Placed{birth:cut.birth.clone(),here:cut.place.clone()};
        let installed=placed.open(root.folder(),&names,true,|s|cut.members.require(s))?;
        let actual=installed.directory();
        let proof=Selected::open_settled(actual,cut.selected.record)?;
        let used=proof.verify_map(actual,|needs|{
            let mut result=Vec::new();
            result.try_reserve_exact(needs.len()).map_err(|_|invalid("cannot hold selected work origins"))?;
            for batch in needs.chunks(64){
                src.at(|db,c|{
                    authorized()?;self.current(db,&c)?;cut.require(db,&c)?;
                    for n in batch{result.push(work_record::origin(db,&cut.birth,cut.selected,*n)?);}Ok(())
                })?;
            }
            Ok(result)
        })?;
        let result=Receipt{original:op,node:NodeId::new()?,version:VersionId::new()?,ord:cut.ord,target:expected};
        after()?;authorized()?;
        root.require_streaming()?;installed.require(root.folder())?;
        cut.members.require(&crate::fs::WitnessRead::at(actual)?)?;
        let alias=directory::check_alias_name(&root,&cut.place.name,cut.alias.as_ref(),None)?;
        used.require(actual)?;proof.require(actual)?;
        let sha=self.freeze_request(op,&cut.birth,expected)?;
        let w=Connection::open(&src.dir,true)?;
        let confirmed=src.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
            self.current(db,&c)?;cut.require(db,&c)?;authorized()?;
            let s=db.prepare(c"INSERT INTO main.manual_node(commit_op,file_id,round_op,save_op,revision,auth_op,parent_op,ord,node_id,version_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)")?;
            for (i,id) in [op.0,expected.file.0,expected.round.0,expected.save.0].iter().enumerate(){s.bind_bytes(i as i32+1,id)?;}
            s.bind_integer(5,expected.revision as i64)?;s.bind_bytes(6,&self.origin.0)?;
            if let Some(p)=expected.parent{s.bind_bytes(7,&p.0)?;}
            s.bind_integer(8,result.ord)?;s.bind_bytes(9,&result.node.0)?;s.bind_bytes(10,&result.version.0)?;
            s.done()?;drop(s);
            let o=db.prepare(c"INSERT INTO main.operation(op_id,kind,actor_id,request_sha,state) VALUES(?1,11,?2,?3,1)")?;
            o.bind_bytes(1,&op.0)?;o.bind_bytes(2,&self.account.0)?;o.bind_bytes(3,&sha)?;o.done()?;Ok(result)
        });
        drop(w);drop(alias);
        match confirmed{Ok(r)=>Ok(r),Err(cause)=>match src.at(|db,c|{
            authorized()?;self.current(db,&c)?;
            self.freeze_result(db,op,&file_create::committed_file(db,&c,expected.file)?,expected)
        }){
            Ok(Some(r))=>Ok(r),Ok(None)=>Err(cause),Err(_)=>Err(io::Error::other("manual publication outcome unconfirmed; query this exact operation")),
        }}
    }
}
