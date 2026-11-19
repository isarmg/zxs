//! One pending SQL birth transaction. No marker, online-ready response or account
//! arises here. Retry lookup/completion is a separate action, never a fresh INIT.
use super::{approval::Approved,bad,engine,originals::Group,request::{role,Id,Request},reservation::Reserved};
use rusqlite::{params,TransactionBehavior};
use rustix::rand::{GetRandomFlags,getrandom};
use std::{io,os::unix::ffi::OsStrExt};
pub(super) const SCHEMA:&str=include_str!("schema.sql");

fn random<const N:usize>()->io::Result<[u8;N]>{
    let mut v=[0;N];
    loop{
        match getrandom(&mut v,GetRandomFlags::NONBLOCK){
            Err(rustix::io::Errno::INTR)=>continue,Err(e)=>return Err(e.into()),
            Ok(n) if n==N && v!=[0;N]=>return Ok(v),
            _=>return Err(bad("incomplete or zero identity from the OS; nothing issued")),
        }
    }
}
struct Issue{pool:Id,rows:Vec<(Id,[u8;32])>}
impl Issue{
    fn new(n:usize)->io::Result<Self>{
        let pool=random()?;let mut rows=Vec::with_capacity(n);
        for _ in 0..n{
            let id=random()?;let mark=random()?;
            if id==pool||rows.iter().any(|(old,token)|*old==id||*token==mark){return Err(bad("random enrollment identity collision"));}
            rows.push((id,mark));
        }
        Ok(Self{pool,rows})
    }
}

pub(super) struct Pending<'g>{pub(super) pool:Id,pub(super) op:Id,
    source:Reserved<'g>,result:Vec<(Id,[u8;32])>}
impl Pending<'_>{
    pub(super) fn require(&self,db:&super::reservation::Existing<'_>,cat:&super::catalog::Catalog,r:&Request)->io::Result<()>{
        if self.op!=r.op()||self.pool!=cat.pool||cat.committed||!self.source.same_existing(db)
          ||self.result.len()!=cat.markers.len()||self.result.iter().zip(&cat.markers).any(|((v,t),m)| &m[37..53]!=v.as_slice()||&m[69..]!=t.as_slice()){
            return Err(bad("fresh original database or its issued results changed before material enrollment"));
        }
        Ok(())
    }
}
pub(super) fn install(approved:Approved)->io::Result<super::complete::Receipt>{
    let (purpose,request,originals)=approved.into_parts();
    // Ensure the exact bounded release schema is accepted before reserving a real DB.
    let template=super::catalog::Canonical::new()?;
    let group=originals.lock(request.uid())?;
    if matches!(purpose,super::Purpose::Fresh){
        let p=first(&request,&group,&Issue::new(group.dirs().len())?)?;
        let done=super::complete::original(&template,&request,&group,Some(&p))?;
        if done.pool!=p.pool||done.op!=p.op||done.previous{return Err(bad("fresh result did not agree with its own pending original"));}
        return Ok(done);
    }
    super::complete::original(&template,&request,&group,None)
}
fn first<'g>(request:&Request,group:&'g Group,issue:&Issue)->io::Result<Pending<'g>>{
    if group.limits().len()!=issue.rows.len()||issue.rows.len()!=request.paths().len(){return Err(bad("incomplete proposed original result"));}
    let reserved=Reserved::create(group,request)?;
    // Both the original group and its S OFD outlive this connection and all its
    // statements. No APIs on this path open/close another raw FD for the DB inode.
    let mut db=engine::open_reserved(&reserved)?;
    let fail=|_|bad("INIT transaction did not produce a confirmed pending receipt; retain the original operation and media");
    let tx=db.transaction_with_behavior(TransactionBehavior::Immediate).map_err(fail)?;
    let objects:i64=tx.query_row("SELECT count(*) FROM main.sqlite_schema",[],|r|r.get(0)).map_err(fail)?;
    if objects!=0{return Err(bad("unexpected schema in the freshly reserved original"));}
    tx.execute_batch(SCHEMA).map_err(fail)?;
    let op=request.op();let actor=request.actor();let approval=request.approval();let sha=request.sha();
    let once=|rows:usize|if rows==1 {Ok(())}else{Err(bad("expected one original inserted row"))};
    once(tx.execute("INSERT INTO operation(op_id,kind,actor_id,request_sha,state) VALUES(?1,1,?2,?3,0)",
         params![&op[..],&actor[..],&sha[..]]).map_err(fail)?)?;
    once(tx.execute("INSERT INTO pool_birth(singleton,pool_id,init_op,approval_ref,storage_uid,root_count) VALUES(1,?1,?2,?3,?4,?5)",
         params![&issue.pool[..],&op[..],&approval[..],i64::from(request.uid()),issue.rows.len() as i64]).map_err(fail)?)?;
    for(i,((id,token),path))in issue.rows.iter().zip(request.paths()).enumerate(){
        once(tx.execute("INSERT INTO volume_claim(claim_op,ordinal,role,volume_id,root_path,component_limit,claim_token) VALUES(?1,?2,?3,?4,?5,?6,?7)",
          params![&op[..],i as i64,i64::from(role(i)),&id[..],path.as_os_str().as_bytes(),i64::from(group.limits()[i]),&token[..]]).map_err(fail)?)?;
    }
    group.recheck()?;reserved.require_name(false)?;engine::original_name(&tx)?;tx.commit().map_err(fail)?;
    // A committed SQL pending birth is NOT an active pool. Query every bound
    // result in one fresh short read; do not call a marker a commit witness.
    {let slice=db.transaction().map_err(fail)?;
     let app:i64=slice.query_row("PRAGMA main.application_id",[],|r|r.get(0)).map_err(fail)?;
     let version:i64=slice.query_row("PRAGMA main.user_version",[],|r|r.get(0)).map_err(fail)?;
     if (app,version)!=(0x5a585331,super::catalog::SCHEMA_VERSION){return Err(bad("fresh database does not identify the agreed schema"));}
     let mut stmt=slice.prepare(include_str!("initial-catalog.sql")).map_err(fail)?;
     let mut rows=stmt.query([&issue.pool[..]]).map_err(fail)?;
     for(i,((id,token),path))in issue.rows.iter().zip(request.paths()).enumerate(){
        let r=rows.next().map_err(fail)?.ok_or_else(||bad("missing pending claim on independent readback"))?;
        let bytes=|col|r.get::<_,Vec<u8>>(col).map_err(fail);let number=|col|r.get::<_,i64>(col).map_err(fail);
        if bytes(0)?!=issue.pool||bytes(1)?!=op||bytes(2)?!=actor||bytes(3)?!=sha||number(4)?!=0||bytes(5)?!=approval
          || number(6)?!=i64::from(request.uid())||number(7)?!=issue.rows.len()as i64||number(8)?!=i as i64
          ||number(9)?!=i64::from(role(i))||bytes(10)?!=*id||bytes(11)?!=path.as_os_str().as_bytes()
          ||number(12)?!=i64::from(group.limits()[i])||bytes(13)?!=*token{return Err(bad("pending SQL result does not match the original request and issued values"));}
     }
     if rows.next().map_err(fail)?.is_some(){return Err(bad("unexpected extra initial claim"));}
     drop(rows);drop(stmt);slice.commit().map_err(fail)?;
    }
    engine::verified(&db,false)?;reserved.require_name(false)?;db.close().map_err(|_|bad("SQLite close uncertain; retain the original"))?;
    reserved.sync_parent()?;group.scan_names(0,&[super::DATABASE.as_bytes()])?;
    for i in 1..issue.rows.len(){group.scan_names(i,&[])?;}
    group.recheck()?;reserved.require_name(false)?;
    Ok(Pending{pool:issue.pool,op,source:reserved,result:issue.rows.clone()})
}

#[cfg(test)]
#[path="../../tests/sqlite/stage.rs"] mod cases;
