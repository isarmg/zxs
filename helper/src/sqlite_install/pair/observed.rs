//! A readonly cut of one explicitly selected, offline SYSTEM. Physical
//! R/F paths may be compared as SQL bytes only, never resolved or opened here.
use super::Id;
use crate::sqlite_install::{bad,catalog::{self,Canonical,Initial},engine::{self,Access},markers::SystemMark,originals::{HeldRoot,LockedSystem},reservation::Existing};
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
/// The fixed Web unit may be associated with this exact enrolled SYSTEM.
/// No verifier, login, alternate path or non-Web credential leaves the cut.
pub(super) fn confirmed_web(pool:&Id,init:&Id,setup:&Id,locked:&LockedSystem)->io::Result<()> {
    with_origin(pool,init,locked,|tx,_|{
        let qualified:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM main.account_pair a
                JOIN main.operation o ON o.op_id=a.birth_op AND o.kind=2 AND o.state=1
                JOIN main.active_credential e ON e.role=0 AND e.slot=a.singleton
                WHERE a.singleton=1 AND a.birth_op=?1)",params![&setup[..]],|r|r.get(0)).map_err(sql)?;
        if !qualified{return Err(bad("the original Web administrator is not confirmed"));}
        Ok(())
    })
}
pub(super) fn init_claims(pool:&Id,init:&Id,locked:&LockedSystem)->io::Result<Vec<Initial>>{
    with_origin(pool,init,locked,|tx,template|catalog::initial_metadata(tx,template,locked.source(),pool,init))
}
pub(super) struct StartupClaims{pub(super) ordinary:Vec<Initial>,pub(super) recycle:Initial}
pub(super) fn startup_claims(plan:&crate::smb_profile::first::Plan,bound_sha:&[u8;32],locked:&LockedSystem)
    ->io::Result<StartupClaims>{
    let o=plan.origin;
    with_origin(&o.pool,&o.init,locked,|tx,template|{
        let live:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM main.account_pair a
               JOIN main.operation s ON s.op_id=a.birth_op AND s.kind=2 AND s.state=1
               JOIN main.active_credential e ON e.role=1 AND e.slot=a.singleton
               WHERE a.singleton=1 AND a.birth_op=?1 AND a.instance_id=?2 AND a.setup_bound_sha=?3)",
            params![&o.setup[..],&o.instance[..],&bound_sha[..]],|r|r.get(0)).map_err(sql)?;
        if !live{return Err(bad("no confirmed original native account on this installation"));}
        let claims=catalog::initial_metadata(tx,template,locked.source(),&o.pool,&o.init)?;
        let mut selected=Vec::with_capacity(plan.folders().len());
        for (f,key) in plan.folders(){
            let issued:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM main.root_share r
                 JOIN main.operation p ON p.op_id=r.publish_op AND p.kind=3 AND p.state=1
                 JOIN main.share_mode m ON m.share_id=r.share_id AND m.revision=1
                 JOIN main.operation q ON q.op_id=m.change_op AND q.kind=4 AND q.state=1
                 WHERE r.share_id=?1 AND r.publish_op=?2 AND r.volume_id=?3 AND r.display_label=?4
                   AND r.author_auth_op=?5 AND m.previous_op=?2 AND m.author_auth_op=?5
                   AND m.change_op=?6 AND m.mode=?7)",
                params![&key.share[..],&key.publish[..],&f.volume[..],f.name.as_bytes(),&o.setup[..],&key.setting[..],i64::from(f.mode.code())],
                |r|r.get(0)).map_err(sql)?;
            let item=claims.iter().find(|c|c.marker[20]==2&&c.marker[37..53]==f.volume)
                .ok_or_else(||bad("a selected ordinary volume has no exact original claim"))?;
            if !issued{return Err(bad("no confirmed original publication matching the selected folder"));}
            selected.push(Initial{path:item.path.clone(),marker:item.marker});
        }
        Ok(StartupClaims{ordinary:selected,recycle:catalog::initial_recycle(&claims)?.clone()})
    })
}
pub(super) fn approved_folders(plan:&crate::smb_profile::first::Plan,locked:&LockedSystem)->io::Result<()>{
    let o=plan.origin;
    let claims=init_claims(&o.pool,&o.init,locked)?;
    catalog::initial_recycle(&claims)?;
    if !plan.folders().iter().all(|(f,_)|claims.iter().any(|c|c.marker[20]==2&&c.marker[37..53]==f.volume)){
        return Err(bad("the first SMB plan is not limited to the original ordinary data volumes"));
    }
    Ok(())
}
