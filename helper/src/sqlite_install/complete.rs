//! Advance only the already authorized original INIT, with no fallback to new
//! IDs and no inferred migration. On uncertainty, retain everything.
use super::{bad,catalog::{self,Canonical,Catalog},engine::{self,Access},markers::Synced,originals::Group,
    request::{Id,Request},reservation::Existing};
use rusqlite::{TransactionBehavior,params};
use std::io;
pub(super) struct Receipt{pub(super) pool:Id,pub(super) op:Id,pub(super) previous:bool}
fn uncertain(_:rusqlite::Error)->io::Error{bad("SQLite completion uncertain; retain original media and operation, do not start again")}
pub(super) fn snapshot(db:&Existing<'_>,template:&Canonical,g:&Group,r:&Request)->io::Result<Catalog>{
    let mut c=engine::existing(db,Access::Read)?;
    let tx=c.transaction().map_err(uncertain)?;let state=catalog::read(&tx,template,g,r)?;
    engine::original_name(&tx)?;db.require()?;tx.commit().map_err(uncertain)?;
    c.close().map_err(|_|bad("original read connection did not close"))?;db.sidecars_absent()?;
    Ok(state)
}
pub(super) fn original(t:&Canonical,r:&Request,g:&Group,warm:Option<&super::stage::Pending<'_>>)->io::Result<Receipt>{
    g.recheck()?;
    let db=Existing::observe(g,r)?;let claim=snapshot(&db,t,g,r)?;
    if let Some(fresh)=warm{fresh.require(&db,&claim,r)?;}
    if claim.committed{
        // A historical receipt does not audit mounted data or repopulate a
        // missing marker. Do not emit a new completion or perform a sync/write.
        return Ok(Receipt{pool:claim.pool,op:r.op(),previous:true});
    }
    // The read connection is closed during external, potentially slow I/O.
    let marks=Synced::install(g,&db,&claim.markers)?;
    publish(&db,t,g,r,&claim,&marks)
}
pub(super) fn publish(db:&Existing<'_>,t:&Canonical,g:&Group,r:&Request,claim:&Catalog,marks:&Synced)->io::Result<Receipt>{
    db.sidecars_absent()?;marks.verify_clean(g,&claim.markers)?;
    let mut c=engine::existing(db,Access::Write)?;let tx=c.transaction_with_behavior(TransactionBehavior::Immediate).map_err(uncertain)?;
    if claim.committed||&catalog::read(&tx,t,g,r)?!=claim{return Err(bad("original INIT changed after marker synchronization"));}
    db.require()?;engine::original_name(&tx)?;marks.verify_clean(g,&claim.markers)?;
    {
        let mut s=tx.prepare(include_str!("complete-initial.sql")).map_err(uncertain)?;
        let mut rows=s.query(params![&r.op()[..],&r.actor()[..],&r.sha()[..],&claim.pool[..],&r.approval()[..]]).map_err(uncertain)?;
        let returned=rows.next().map_err(uncertain)?.ok_or_else(||bad("no original pending claim to finish"))?;
        if returned.get::<_,Vec<u8>>(0).map_err(uncertain)?!=r.op()||rows.next().map_err(uncertain)?.is_some(){
            return Err(bad("completion changed anything other than the exact original INIT"));
        }
    }
    db.require()?;engine::original_name(&tx)?;marks.verify_committing(g,&tx,&claim.markers)?;tx.commit().map_err(uncertain)?;
    c.close().map_err(|_|bad("final connection close uncertain; retain original operation"))?;
    g.system().sync_all()?;db.sidecars_absent()?;
    let after=snapshot(db,t,g,r)?;
    if !after.committed||after.pool!=claim.pool||after.markers!=claim.markers{return Err(bad("original committed receipt is not confirmed"));}
    marks.verify_clean(g,&claim.markers)?;Ok(Receipt{pool:after.pool,op:r.op(),previous:false})
}
