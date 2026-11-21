//! A readonly cut of one explicitly selected, offline SYSTEM. Physical
//! R/F paths may be compared as SQL bytes only, never resolved or opened here.
use super::Id;
use crate::sqlite_install::{bad,catalog::{self,Canonical,Initial},engine::{self,Access},markers::{SystemMark,SystemObservation},originals::{HeldRoot,LockedSystem},reservation::Existing};
use rusqlite::{params,Transaction};
use std::{io,os::unix::ffi::OsStrExt};

fn sql(_:rusqlite::Error)->io::Error {bad("original account setup observation could not be completed")}

pub(super) fn first_marker(pool:&Id,init:&Id,tx:&Transaction<'_>,root:HeldRoot<'_>)->io::Result<[u8;101]>{
    // Only row zero, in this exact successful original INIT. Neither a claim
    // copied to a volume nor the present name of a mounted data disk is
    // evidence for this private account lookup. No R/F pathname is selected.
    let mut s=tx.prepare("SELECT b.storage_uid,b.root_count,v.volume_id,v.root_path,v.component_limit,v.claim_token
        FROM main.pool_birth AS b JOIN main.operation AS i ON i.op_id=b.init_op AND i.kind=1 AND i.state=1
        JOIN main.volume_claim AS v ON v.claim_op=b.init_op AND v.ordinal=0 AND v.role=0
        WHERE b.singleton=1 AND b.pool_id=?1 AND b.init_op=?2 LIMIT 2").map_err(sql)?;
    let mut rows=s.query(params![&pool[..],&init[..]]).map_err(sql)?;
    let row=rows.next().map_err(sql)?.ok_or_else(||bad("no committed first SYSTEM claim for this original pool and operation"))?;
    let bytes=|i|row.get_ref(i).map_err(sql)?.as_blob().map_err(|_|bad("invalid SYSTEM claim bytes"));
    let n=|i|row.get::<_,i64>(i).map_err(sql);
    let volume:[u8;16]=bytes(2)?.try_into().map_err(|_|bad("invalid original SYSTEM ID"))?;
    let token:[u8;32]=bytes(5)?.try_into().map_err(|_|bad("invalid original SYSTEM material token"))?;
    if n(0)?!=i64::from(root.uid())||!(3..=64).contains(&n(1)?)||bytes(3)?!=root.path().as_os_str().as_bytes()
        ||n(4)?!=i64::from(root.byte_limit())||volume==*pool||volume==[0;16]||token==[0;32]
        ||rows.next().map_err(sql)?.is_some(){return Err(bad("original SYSTEM claim does not match this selected directory"));}
    Ok(crate::sqlite_install::catalog::claimed_marker(0,pool,&volume,init,&token))
}

// All callers use exactly this read-only original S cut. The first installer
// needs only proof of the committed INIT, not accounts or any R/F paths.
fn with_origin<T>(pool:&Id,init:&Id,locked:&LockedSystem,read:impl FnOnce(&Transaction<'_>,&Canonical)->io::Result<T>)->io::Result<T>{
    let template=Canonical::new()?;let source=locked.source();let db=Existing::observed_system(locked)?;
    // Hold the actual admitted private marker *before* SQLite opens. Inside
    // the transaction only statx its name and read its already-held FD;
    // never open/close another raw FD for the underlying database inode.
    let mark=SystemMark::before_sql(source)?;
    Ok(at_original(pool,init,locked,&db,&template,&mark,|tx|read(tx,&template))?.1)
}

// Reuse the *same* original DB identity, held SYSTEM marker and exact
// schema when an action requires more than one short, independent connection.
pub(super) fn at_original<T>(pool:&Id,init:&Id,locked:&LockedSystem,db:&Existing<'_>,template:&Canonical,mark:&SystemMark,
    read:impl FnOnce(&Transaction<'_>)->io::Result<T>)->io::Result<([u8;101],T)> {
    let mut c=engine::existing(db,Access::Read)?;
    let result=snapshot(&mut c,pool,init,locked,db,template,mark,read)?;
    c.close().map_err(|_|bad("original readonly account observation did not close cleanly"))?;
    db.sidecars_absent()?;mark.verify(locked.source(),&result.0)?;Ok(result)
}

// A caller may retain this actual standard-VFS connection, with *no active
// transaction*, to keep the original database inode alive across human input.
pub(super) fn snapshot<T>(c:&mut rusqlite::Connection,pool:&Id,init:&Id,locked:&LockedSystem,db:&Existing<'_>,template:&Canonical,
    mark:&SystemMark,read:impl FnOnce(&Transaction<'_>)->io::Result<T>)->io::Result<([u8;101],T)>{
    let source=locked.source();db.sidecars_absent()?;engine::original_name(c)?;
    if !c.is_autocommit(){return Err(bad("unexpected open SQLite transaction before the account observation"));}
    let tx=c.transaction().map_err(sql)?;template.require(&tx)?;
    let actual=first_marker(pool,init,&tx,source)?;mark.verify(source,&actual)?;
    let result=read(&tx)?;
    mark.verify(source,&actual)?;engine::original_name(&tx)?;db.sidecars_absent()?;
    tx.commit().map_err(sql)?;
    if !c.is_autocommit(){return Err(bad("original account snapshot did not end"));}
    engine::original_name(c)?;locked.require()?;db.sidecars_absent()?;mark.verify(source,&actual)?;
    Ok((actual,result))
}
pub(super) fn init_claims(pool:&Id,init:&Id,locked:&LockedSystem)->io::Result<Vec<Initial>>{
    with_origin(pool,init,locked,|tx,template|catalog::initial_metadata(tx,template,locked.source(),pool,init))
}
/// Only for one new ZXS issuance. Existing or partially applied account
/// authority, non-INIT history or an unknown first cut is never adopted.
pub(super) fn isolated_first_init(tx:&Transaction<'_>,pool:&Id,init:&Id,actor:&Id,init_sha:&[u8;32])->io::Result<()>{
    let found:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM main.pool_birth b
        JOIN main.operation i ON i.op_id=b.init_op AND i.kind=1 AND i.state=1
        WHERE b.singleton=1 AND b.pool_id=?1 AND b.init_op=?2 AND i.actor_id=?3 AND i.request_sha=?4)
        AND NOT EXISTS(SELECT 1 FROM main.operation WHERE op_id<>?2)
        AND NOT EXISTS(SELECT 1 FROM main.account_pair)
        AND NOT EXISTS(SELECT 1 FROM main.web_password)
        AND NOT EXISTS(SELECT 1 FROM main.smb_rust_revision)
        AND NOT EXISTS(SELECT 1 FROM main.credential_epoch)
        AND NOT EXISTS(SELECT 1 FROM main.credential_reset)
        AND NOT EXISTS(SELECT 1 FROM main.root_share)
        AND NOT EXISTS(SELECT 1 FROM main.share_mode)",
        params![&pool[..],&init[..],&actor[..],&init_sha[..]],|r|r.get(0)).map_err(sql)?;
    if !found{return Err(bad("not the exact committed account-free first v17 INIT"));}
    Ok(())
}
pub(super) fn isolated_first_claims(pool:&Id,init:&Id,actor:&Id,init_sha:&[u8;32],locked:&LockedSystem)
    ->io::Result<(Vec<Initial>,std::fs::File)>{
    super::fresh_issuer::require_v17()?;
    let source=locked.source();let schema=Canonical::new()?;let db=Existing::observed_system(locked)?;
    let marker=SystemMark::before_sql(source)?;
    let pin=db.fresh_observation_pin()?;
    let (_,claims)=at_original(pool,init,locked,&db,&schema,&marker,|tx|{
        isolated_first_init(tx,pool,init,actor,init_sha)?;
        catalog::initial_metadata(tx,&schema,source,pool,init)
    })?;
    db.sidecars_absent()?;let _same=db.fresh_observation_pin()?;
    Ok((claims,pin))
}
/// The actual offline read observer, never a credential writer. Recover the
/// exact original INIT claim and current ACTIVE metadata, with a non-readable
/// pin to the actual database checked before and after the short SQLite cut.
pub(super) fn deployed_active(locked:&LockedSystem,site:&super::fresh_issuer::SitePayload,plan:&crate::smb_profile::rust_first::RustPlan)
    ->io::Result<(std::fs::File,[u8;64],SystemObservation,[u8;224])>{
    use super::{fresh_issuer::InitialDatabase,native_sql::rust_vault::active_at};
    super::fresh_issuer::compiled_v17()?;
    let (actor,uid,_)=site.subject.tuple();
    let o=site.origin;
    let source=locked.source();
    if site.plan_sha!=plan.sha()||o!=plan.origin||site.system!=source.path()||uid!=source.uid(){
        return Err(bad("different held SYSTEM or installed v17 plan"));
    }
    let schema=Canonical::new()?;
    let db=Existing::observed_system(locked)?;
    let marker=SystemMark::before_sql(source)?;
    let pin=db.fresh_observation_pin()?;
    let stamp=InitialDatabase::observation_on(&pin,uid)?;
    let (body,public)=at_original(&o.pool,&o.init,locked,&db,&schema,&marker,|tx|{
        let is_original:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM main.operation
            WHERE op_id=?1 AND actor_id=?2 AND kind=1 AND state=1)",params![&o.init[..],&actor[..]],|r|r.get(0)).map_err(sql)?;
        if !is_original{return Err(bad("the protected site does not name the actual committed INIT actor"));}
        // Reconstruct and check the original bounded catalogue digest. The
        // R/F byte paths are not opened by the deployment observer.
        catalog::initial_metadata(tx,&schema,source,&o.pool,&o.init)?;
        active_at(tx,plan)?.record_prefix()
    })?;
    let named=db.fresh_observation_pin()?;
    if InitialDatabase::observation_on(&pin,uid)?!=stamp||InitialDatabase::observation_on(&named,uid)?!=stamp{
        return Err(bad("original SYSTEM database changed during the read-only deployment check"));
    }
    let original_mark=marker.observe(source,&body)?;
    Ok((pin,stamp,original_mark,public))
}
#[cfg(test)]
mod fresh_v17_tests{
    use super::*;
    #[test]
    fn fresh_v17_sql_cut_rejects_pending_init_other_actor_receipt_and_prior_accounts(){
        // Use only the frozen schema in memory; deliberately no host authority
        // or mounted root. The real route separately proves the S VFS and marks.
        let mut c=rusqlite::Connection::open_in_memory().unwrap();
        c.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        c.execute_batch(crate::sqlite_install::stage::SCHEMA).unwrap();
        let (pool,init,actor,reference,setup,instance)=([1u8;16],[2u8;16],[3u8;16],[4u8;16],[5u8;16],[6u8;16]);
        let requested=[7u8;32];
        let tx=c.transaction().unwrap();
        tx.execute("INSERT INTO operation VALUES(?1,1,?2,?3,0)",params![&init[..],&actor[..],&requested[..]]).unwrap();
        tx.execute("INSERT INTO pool_birth VALUES(1,?1,?2,?3,1001,3)",params![&pool[..],&init[..],&reference[..]]).unwrap();
        assert!(isolated_first_init(&tx,&pool,&init,&actor,&requested).is_err());
        for (i,path) in [b"/fixture/S".as_slice(),b"/fixture/R",b"/fixture/F"].into_iter().enumerate(){
            tx.execute("INSERT INTO volume_claim VALUES(?1,?2,?2,?3,?4,255,?5)",
                params![&init[..],i as i64,&[11u8+i as u8;16][..],path,&[21u8+i as u8;32][..]]).unwrap();
        }
        tx.execute("UPDATE operation SET state=1 WHERE op_id=?1",[&init[..]]).unwrap();
        assert!(isolated_first_init(&tx,&pool,&init,&actor,&requested).is_ok());
        assert!(isolated_first_init(&tx,&pool,&init,&[31;16],&requested).is_err());
        assert!(isolated_first_init(&tx,&pool,&init,&actor,&[32;32]).is_err());
        tx.commit().unwrap();

        let tx=c.transaction().unwrap();
        let mut sid=[42u8;28];sid[..12].copy_from_slice(&[1,5,0,0,0,0,0,5,21,0,0,0]);
        sid[24..].copy_from_slice(&1000u32.to_le_bytes());
        tx.execute("INSERT INTO account_pair VALUES(1,?1,?2,?3,?4,?5,X'776562',?6,X'736d62',?7,0,NULL)",
            params![&setup[..],&reference[..],&instance[..],&[19u8;32][..],&[20u8;16][..],&[21u8;16][..],&sid[..]]).unwrap();
        tx.execute("INSERT INTO web_password VALUES(1,?1,1,?2,?3)",
            params![&setup[..],&[22u8;16][..],&[23u8;32][..]]).unwrap();
        tx.execute("INSERT INTO operation VALUES(?1,2,?2,?3,0)",params![&setup[..],&actor[..],&[24u8;32][..]]).unwrap();
        assert!(isolated_first_init(&tx,&pool,&init,&actor,&requested).is_err());
        tx.rollback().unwrap();
        let tx=c.transaction().unwrap();
        assert!(isolated_first_init(&tx,&pool,&init,&actor,&requested).is_ok());
    }
}
