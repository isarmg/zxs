//! Exact selected/issued SAVE metadata. SQLite owns the order and current
//! authority, while only real FILE code can witness or append its bytes.
use super::{Connection,Statement,check};
use super::smb::first_work;
use crate::{
    FileId,OperationId,VolumeId,fs::invalid,hash::Sha256,identity::{AccountId,ShareId},
    ordinary::{portable_birth::Birth,working_round::{End,Key,Map,Record}},
};
use std::io;

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(super) struct Edit {
    pub(super) record:Record,
    pub(super) actor:AccountId,
    pub(super) credential:OperationId,
    pub(super) generation:i64,
    pub(super) share:ShareId,
    pub(super) setting:OperationId,
    pub(super) position:OperationId,
    pub(super) volume:VolumeId,
    pub(super) nonce:OperationId,
    pub(super) issue:i64,
    pub(super) predecessor:Option<OperationId>,
    pub(super) parent:Option<OperationId>,
    pub(super) previous_pending:Option<OperationId>,
    pub(super) retained:u8,
}
impl Edit {
    pub(super) fn request(self,b:&Birth)->io::Result<[u8;32]>{
        let k=self.record.key();let f=self.record.start();let e=self.record.end();
        let m=self.record.map().ok_or_else(||invalid("interval SAVE must specify the selected map"))?;
        if (k.pool,k.file)!=(b.pool,b.file)||self.generation<1||self.issue<1||
           [self.actor.0,self.credential.0,self.share.0,self.setting.0,self.position.0,self.volume.0,self.nonce.0].contains(&[0;16])||
           [self.predecessor,self.parent,self.previous_pending].into_iter().flatten().any(|id|id.0==[0;16]||id==k.save)||
           self.retained>15||self.previous_pending.is_none()&&self.retained!=0{
            return Err(invalid("wrong original authority or scope for a working SAVE"));
        }
        let mut h=Sha256::new();h.update(b"ZXS-SQLITE-1/WORK-EDIT\0")?;
        for id in [b.pool.0,self.volume.0,b.operation.0,b.entry.0,b.file.0,b.history.0,b.branch.0,
                   k.save.0,k.round.0,self.actor.0,self.credential.0,self.share.0,self.setting.0,self.position.0,self.nonce.0]{h.update(&id)?;}
        h.update(&self.generation.to_be_bytes())?;h.update(&self.issue.to_be_bytes())?;
        for op in [self.predecessor,self.parent,self.previous_pending]{
            h.update(&[u8::from(op.is_some())])?;if let Some(op)=op{h.update(&op.0)?;}
        }
        h.update(&[self.retained])?;
        for v in [k.revision,self.record.logical(),f.data,e.data,f.checks,m.at,m.bytes,e.checks]{h.update(&v.to_be_bytes())?;}
        h.update(&self.record.packed_sha())?;h.update(&self.record.directory_sha())?;h.update(&m.sha)?;
        Ok(h.finish())
    }
    /// Inserts only the prepared metadata and original pending state. A
    /// caller must already hold this FILE's EX and an immediate SQL cut.
    pub(super) fn insert(self,db:&Connection,b:&Birth)->io::Result<()>{
        let k=self.record.key();let f=self.record.start();let e=self.record.end();
        let sha=self.request(b)?;let m=self.record.map().ok_or_else(||invalid("missing working map"))?;
        let s=db.prepare(c"INSERT INTO main.work_edit(edit_op,file_id,share_id,setting_op,auth_op,writer_nonce,issue,round_op,predecessor,parent_commit,revision,logical_bytes,raw_at,raw_end,checks_at,map_at,map_bytes,checks_end,packed_sha,directory_sha,map_sha,previous_pending,retained_mask,position_op) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24)")?;
        for (i,v) in [k.save.0,k.file.0,self.share.0,self.setting.0,self.credential.0,self.nonce.0].iter().enumerate(){s.bind_bytes(i as i32+1,v)?;}
        s.bind_integer(7,self.issue)?;s.bind_bytes(8,&k.round.0)?;
        if let Some(p)=self.predecessor{s.bind_bytes(9,&p.0)?;}
        if let Some(p)=self.parent{s.bind_bytes(10,&p.0)?;}
        for (i,v) in [k.revision,self.record.logical(),f.data,e.data,f.checks,m.at,m.bytes,e.checks].into_iter().enumerate(){
            s.bind_integer(i as i32+11,v.try_into().map_err(|_|invalid("working coordinates exceed exact SQL integers"))?)?;
        }
        for (i,v) in [self.record.packed_sha(),self.record.directory_sha(),m.sha].iter().enumerate(){s.bind_bytes(i as i32+19,v)?;}
        if let Some(op)=self.previous_pending{s.bind_bytes(22,&op.0)?;}s.bind_integer(23,i64::from(self.retained))?;s.bind_bytes(24,&self.position.0)?;
        s.done()?;drop(s);
        let o=db.prepare(c"INSERT INTO main.operation(op_id,kind,actor_id,request_sha,state) VALUES(?1,10,?2,?3,0)")?;
        o.bind_bytes(1,&k.save.0)?;o.bind_bytes(2,&self.actor.0)?;o.bind_bytes(3,&sha)?;o.done()
    }
}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(super) struct Issued {
    pub(super) edit:Edit,
    pub(super) confirmed:bool,
}
pub(super) fn start_one(s:&Statement<'_>)->io::Result<bool>{
    match s.step(){100=>Ok(true),101=>Ok(false),e=>{check(e)?;Err(invalid("cannot inspect original work metadata"))}}
}
pub(super) fn natural(s:&Statement<'_>,i:i32)->io::Result<u64>{
    u64::try_from(s.integer(i)?).map_err(|_|invalid("negative original work coordinates"))
}
pub(super) fn optional(s:&Statement<'_>,i:i32)->io::Result<Option<OperationId>>{
    if s.is_null(i){Ok(None)}else{Ok(Some(OperationId(s.array(i)?)))}
}

pub(super) fn edit(db:&Connection,b:&Birth,op:OperationId)->io::Result<Option<Issued>>{
    let q=db.prepare(c"SELECT o.kind,o.state,o.actor_id,o.request_sha,w.file_id,w.share_id,w.setting_op,w.auth_op,w.writer_nonce,w.issue,w.round_op,w.predecessor,w.parent_commit,w.revision,w.logical_bytes,w.raw_at,w.raw_end,w.checks_at,w.map_at,w.map_bytes,w.checks_end,w.packed_sha,w.directory_sha,w.map_sha,r.volume_id,m.share_id,m.mode,a.smb_id,k.generation,w.previous_pending,w.retained_mask,w.position_op FROM main.operation o LEFT JOIN main.work_edit w ON w.edit_op=o.op_id LEFT JOIN main.root_share r ON r.share_id=w.share_id LEFT JOIN main.share_mode m ON m.change_op=w.setting_op LEFT JOIN main.credential_epoch k ON k.role=1 AND k.credential_op=w.auth_op LEFT JOIN main.account_pair a ON a.singleton=k.slot WHERE o.op_id=?1")?;
    q.bind_bytes(1,&op.0)?;
    if !start_one(&q)?{return Ok(None);}
    if q.integer(0)?!=10||FileId(q.array(4)?)!=b.file{return Err(invalid("operation belongs to a different FILE or kind"));}
    let key=Key{pool:b.pool,file:b.file,round:OperationId(q.array(10)?),save:op,revision:natural(&q,13)?};
    let record=Record::edit(key,End{data:natural(&q,15)?,checks:natural(&q,17)?},
        End{data:natural(&q,16)?,checks:natural(&q,20)?},natural(&q,14)?,q.array(21)?,q.array(22)?,
        Map{at:natural(&q,18)?,bytes:natural(&q,19)?,sha:q.array(23)?})?;
    let request=Edit{record,actor:AccountId(q.array(2)?),credential:OperationId(q.array(7)?),generation:q.integer(28)?,
        share:ShareId(q.array(5)?),setting:OperationId(q.array(6)?),position:OperationId(q.array(31)?),volume:VolumeId(q.array(24)?),nonce:OperationId(q.array(8)?),issue:q.integer(9)?,
        predecessor:optional(&q,11)?,parent:optional(&q,12)?,previous_pending:optional(&q,29)?,retained:q.byte(30)?};
    let confirmed=match q.byte(1)?{0=>false,1=>true,_=>return Err(invalid("invalid SAVE result phase"))};
    if request.share!=ShareId(q.array(25)?)||q.byte(26)?!=2||
        request.actor!=AccountId(q.array(27)?)||request.request(b)?!=q.array::<32>(3)?{
        return Err(invalid("recorded SAVE differs from its bounded origin"));
    }
    q.done()?;Ok(Some(Issued{edit:request,confirmed}))
}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(super) struct ReadRecord{
    pub(super) record:Record,
    pub(super) issue:i64,
}
pub(super) fn confirmed(db:&Connection,b:&Birth,op:OperationId)->io::Result<ReadRecord>{
    let q=db.prepare(c"SELECT kind,state FROM main.operation WHERE op_id=?1")?;
    q.bind_bytes(1,&op.0)?;if !start_one(&q)?{return Err(invalid("unknown material origin"));}
    if q.byte(1)?!=1{return Err(invalid("no confirmed working material at this cut"));}
    let kind=q.byte(0)?;q.done()?;
    match kind{
        6=>Ok(ReadRecord{record:first_work::confirmed_record(db,b,op)?,issue:0}),
        10=>{
            let w=edit(db,b,op)?.filter(|e|e.confirmed).ok_or_else(||invalid("no exact confirmed work revision"))?;
            Ok(ReadRecord{record:w.edit.record,issue:w.edit.issue})
        }
        _=>Err(invalid("operation is not an issued working material")),
    }
}
pub(super) fn selected(db:&Connection,b:&Birth)->io::Result<Option<ReadRecord>>{
    let q=db.prepare(c"SELECT save_op,round_op,issue,revision,logical_bytes,raw_end,checks_end FROM main.selected_work WHERE file_id=?1")?;
    q.bind_bytes(1,&b.file.0)?;if !start_one(&q)?{return Ok(None);}
    let row=confirmed(db,b,OperationId(q.array(0)?))?;
    let end=row.record.end();
    if row.record.key().round!=OperationId(q.array(1)?)||row.issue!=q.integer(2)?||row.record.key().revision!=natural(&q,3)?||
       row.record.logical()!=natural(&q,4)?||end.data!=natural(&q,5)?||end.checks!=natural(&q,6)?{
        return Err(invalid("current logical and physical work boundaries differ from the issued snapshot"));
    }
    q.done()?;Ok(Some(row))
}
pub(super) fn last_issue(db:&Connection,b:&Birth)->io::Result<Option<Issued>>{
    let q=db.prepare(c"SELECT edit_op FROM main.work_edit WHERE file_id=?1 ORDER BY issue DESC LIMIT 1")?;
    q.bind_bytes(1,&b.file.0)?;if !start_one(&q)?{return Ok(None);}
    let op=OperationId(q.array(0)?);q.done()?;
    Ok(Some(edit(db,b,op)?.ok_or_else(||invalid("missing original working edit"))?))
}
pub(super) fn pending_current(db:&Connection,b:&Birth)->io::Result<Option<Issued>>{
    let q=db.prepare(c"SELECT edit_op FROM main.pending_edit WHERE file_id=?1")?;
    q.bind_bytes(1,&b.file.0)?;if !start_one(&q)?{return Ok(None);}
    let op=OperationId(q.array(0)?);q.done()?;
    Ok(Some(edit(db,b,op)?.filter(|e|!e.confirmed).ok_or_else(||invalid("missing actual pending SAVE"))?))
}
/// Policy-only unsettled work; do not use in physical member/offset accounting.
pub(super) fn active_pending(db:&Connection,b:&Birth)->io::Result<Option<Issued>>{
    let q=db.prepare(c"SELECT edit_op FROM main.active_pending_edit WHERE file_id=?1")?;
    q.bind_bytes(1,&b.file.0)?;if !start_one(&q)?{return Ok(None);}
    let op=OperationId(q.array(0)?);q.done()?;
    Ok(Some(edit(db,b,op)?.filter(|e|!e.confirmed).ok_or_else(||invalid("missing active pending SAVE"))?))
}
pub(super) fn latest_commit(db:&Connection,b:&Birth)->io::Result<Option<(OperationId,OperationId,OperationId,i64)>>{
    let q=db.prepare(c"SELECT commit_op,save_op,round_op,ord FROM main.last_manual WHERE file_id=?1")?;
    q.bind_bytes(1,&b.file.0)?;if !start_one(&q)?{return Ok(None);}
    let row=(OperationId(q.array(0)?),OperationId(q.array(1)?),OperationId(q.array(2)?),q.integer(3)?);
    q.done()?;Ok(Some(row))
}

#[derive(Clone,Eq,PartialEq)]
pub(super) struct Members {
    must:std::collections::BTreeSet<Vec<u8>>,
    permitted:std::collections::BTreeSet<Vec<u8>>,
    charged_bytes:usize,
}
impl Members {
    fn register(&mut self,names:[std::ffi::CString;4],mask:u8,required:bool)->io::Result<()>{
        if mask>15{return Err(invalid("bad original set of preserved work members"));}
        for (i,name) in names.into_iter().enumerate(){
            if mask&(1<<i)==0{continue;}
            let bytes=name.into_bytes();
            self.charged_bytes+=1+bytes.len();
            if !self.permitted.insert(bytes.clone()){
                return Err(invalid("two issued rounds claim the same original name"));
            }
            if required{self.must.insert(bytes);}
        }
        if self.permitted.len()+1>crate::fs::DIRECTORY_NAMES||self.charged_bytes>crate::fs::DIRECTORY_BYTES{
            return Err(invalid("work materials exceed the existing FILE name budget"));
        }
        Ok(())
    }
    pub(super) fn fits_next_pair(&self,new:OperationId,old:Option<OperationId>,retained:u8)->io::Result<()>{
        let mut copy=self.clone();
        if let Some(round)=old{
            let names=crate::ordinary::working_round::names(round);
            for name in &names{
                let bytes=name.to_bytes();
                if !copy.permitted.remove(bytes)||copy.must.contains(bytes){return Err(invalid("old names do not belong to the pending work round"));}
                copy.charged_bytes-=1+bytes.len();
            }
            copy.register(names,retained,false)?;
        }
        copy.register(crate::ordinary::working_round::names(new),15,false)
    }
    pub(super) fn require(&self,scope:&crate::fs::WitnessRead<'_>)->io::Result<()>{
        scope.claimed_work(&self.must,&self.permitted)
    }
}
pub(super) fn members(db:&Connection,c:&super::Catalog,b:&Birth)->io::Result<Members>{
    let (legacy,old)=first_work::membership(db,c,b)?;
    let charged_bytes=5+old.iter().map(|n|1+n.len()).sum::<usize>();
    let mut m=Members{must:Default::default(),permitted:old,charged_bytes};
    if let Some((spec,done))=legacy{m.register(spec.names()?,15,done)?;}
    let q=db.prepare(c"SELECT w.edit_op FROM main.work_edit w JOIN main.operation o ON o.op_id=w.edit_op AND o.kind=10 AND o.state=1 WHERE w.file_id=?1 AND w.revision=1 ORDER BY w.issue")?;
    q.bind_bytes(1,&b.file.0)?;
    while start_one(&q)?{
        let op=OperationId(q.array(0)?);
        let first=edit(db,b,op)?.ok_or_else(||invalid("missing issuance of a work-round pair"))?;
        if first.edit.record.key().round!=op{
            return Err(invalid("work pair does not belong to its original round"));
        }
        m.register(crate::ordinary::working_round::names(op),15,true)?;
    }
    drop(q);
    if let Some(p)=pending_current(db,b)?.filter(|r|r.edit.record.key().revision==1){
        m.register(crate::ordinary::working_round::names(p.edit.record.key().round),15,false)?;
    }
    let q=db.prepare(c"SELECT w.previous_pending,w.retained_mask,p.round_op,p.revision,o.state,p.file_id
      FROM main.work_edit w JOIN main.work_edit p ON p.edit_op=w.previous_pending
      JOIN main.operation o ON o.op_id=p.edit_op AND o.kind=10
      JOIN main.file_location l ON l.file_id=w.file_id
      WHERE w.file_id=?1 AND w.retained_mask<>0 AND EXISTS(SELECT 1 FROM main.file_material_position h
        WHERE h.position_op=w.position_op AND h.file_id=l.file_id AND h.volume_id=l.volume_id
          AND h.backing_op IS l.backing_op)")?;
    q.bind_bytes(1,&b.file.0)?;
    while start_one(&q)? {
        let p=OperationId(q.array(0)?);
        if q.array::<16>(2)?!=p.0||q.integer(3)?!=1||q.byte(4)?!=0||q.array::<16>(5)?!=b.file.0{
            return Err(invalid("preserved work names do not belong to an actual superseded round"));
        }
        m.register(crate::ordinary::working_round::names(p),q.byte(1)?,false)?;
    }
    Ok(m)
}
/// The latest safe *physical* end of the selected round, distinct from
/// today's published revision. A same-round candidate can have a partial
/// regular-file tail but can never widen the chosen snapshot.
pub(super) fn basis(db:&Connection,b:&Birth,save:Option<OperationId>)->io::Result<Option<(ReadRecord,End)>>{
    save.map(|id|{let r=confirmed(db,b,id)?;Ok((r,ceiling(db,b,r)?))}).transpose()
}
pub(super) fn ceiling(db:&Connection,b:&Birth,selected:ReadRecord)->io::Result<End>{
    let cut=selected.record;
    let q=db.prepare(c"SELECT w.edit_op FROM main.work_edit w JOIN main.operation o ON o.op_id=w.edit_op AND o.kind=10
      JOIN main.file_location l ON l.file_id=w.file_id WHERE w.file_id=?1 AND w.round_op=?2
      AND (o.state=1 OR EXISTS(SELECT 1 FROM main.file_material_position h
        WHERE h.position_op=w.position_op AND h.file_id=l.file_id AND h.volume_id=l.volume_id
          AND h.backing_op IS l.backing_op))
      ORDER BY w.issue DESC LIMIT 1")?;
    q.bind_bytes(1,&b.file.0)?;q.bind_bytes(2,&cut.key().round.0)?;
    if !start_one(&q)?{return Ok(cut.end());}
    let id=OperationId(q.array(0)?);q.done()?;
    let last=edit(db,b,id)?.ok_or_else(||invalid("missing physical limits of this issued round"))?;
    let now=last.edit.record;let end=now.end();
    if last.edit.issue<selected.issue||now.key().round!=cut.key().round||now.key().revision<cut.key().revision||
       end.data<cut.end().data||end.checks<cut.end().checks{
        return Err(invalid("the original active work-round endpoints have regressed"));
    }
    Ok(end)
}
/// Origins authorized by this exact older or current snapshot. An old,
/// still-open handle is not silently advanced to the latest work revision.
pub(super) fn origin(db:&Connection,b:&Birth,cut:ReadRecord,need:crate::ordinary::working_round::Need)
    ->io::Result<crate::ordinary::working_round::Origin>{
    let target=confirmed(db,b,need.issuer)?;
    let owned=target.record;let chosen=cut.record;
    if target.issue>cut.issue||owned.end().data-owned.start().data!=need.bytes{
        return Err(invalid("selected work material is outside this confirmed snapshot"));
    }
    let physical=if owned.key().round==chosen.key().round{
        chosen.end()
    }else{
        let q=db.prepare(c"SELECT n.save_op,n.revision FROM main.manual_node n JOIN main.operation o ON o.op_id=n.commit_op AND o.kind=11 AND o.state=1 WHERE n.file_id=?1 AND n.round_op=?2")?;
        q.bind_bytes(1,&b.file.0)?;q.bind_bytes(2,&owned.key().round.0)?;
        if !start_one(&q)?{return Err(invalid("inherited work source is not a frozen earlier round"));}
        let archive=confirmed(db,b,OperationId(q.array(0)?))?;
        if archive.issue<target.issue||archive.issue>=cut.issue||archive.record.key().round!=owned.key().round||
           archive.record.key().revision!=natural(&q,1)?{
            return Err(invalid("inherited work source is outside its confirmed archive"));
        }
        q.done()?;archive.record.end()
    };
    Ok(crate::ordinary::working_round::Origin{record:owned,physical})
}

pub(super) fn version(db:&Connection,b:&Birth,node:crate::NodeId)->io::Result<ReadRecord>{
    let q=db.prepare(c"SELECT n.save_op,n.round_op,n.revision FROM main.manual_node n JOIN main.operation o ON o.op_id=n.commit_op AND o.kind=11 AND o.state=1 WHERE n.file_id=?1 AND n.node_id=?2")?;
    q.bind_bytes(1,&b.file.0)?;q.bind_bytes(2,&node.0)?;
    if !start_one(&q)?{return Err(io::ErrorKind::NotFound.into());}
    let r=confirmed(db,b,OperationId(q.array(0)?))?;
    if r.record.key().round!=OperationId(q.array(1)?)||r.record.key().revision!=natural(&q,2)?{
        return Err(invalid("manual node does not identify its original confirmed work revision"));
    }
    q.done()?;Ok(r)
}
