//! A readonly cut of one explicitly selected, offline SYSTEM. No remote volume
//! paths, credentials, network domains or proposed catalogue choices are inputs.
use super::{PairRequest,History};
use crate::sqlite_install::{bad,catalog::Canonical,engine::{self,Access},markers::SystemMark,originals::{HeldRoot,LockedSystem},reservation::Existing};
use rusqlite::{params,Transaction};
use std::{io,os::unix::ffi::OsStrExt};

fn sql(_:rusqlite::Error)->io::Error {bad("original account setup observation could not be completed")}

impl PairRequest {
    fn first_marker(&self,tx:&Transaction<'_>,root:HeldRoot<'_>)->io::Result<[u8;101]>{
        // Only row zero, in this exact successful original INIT. Neither a claim
        // copied to a volume nor the present name of a mounted data disk is
        // evidence for this private account lookup. No R/F pathname is selected.
        let mut s=tx.prepare("SELECT b.storage_uid,b.root_count,v.volume_id,v.root_path,v.component_limit,v.claim_token
            FROM main.pool_birth AS b JOIN main.operation AS i ON i.op_id=b.init_op AND i.kind=1 AND i.state=1
            JOIN main.volume_claim AS v ON v.claim_op=b.init_op AND v.ordinal=0 AND v.role=0
            WHERE b.singleton=1 AND b.pool_id=?1 AND b.init_op=?2 LIMIT 2").map_err(sql)?;
        let mut rows=s.query(params![&self.pool[..],&self.init[..]]).map_err(sql)?;
        let row=rows.next().map_err(sql)?.ok_or_else(||bad("no committed first SYSTEM claim for this original pool and operation"))?;
        let bytes=|i|row.get_ref(i).map_err(sql)?.as_blob().map_err(|_|bad("invalid SYSTEM claim bytes"));
        let n=|i|row.get::<_,i64>(i).map_err(sql);
        let volume:[u8;16]=bytes(2)?.try_into().map_err(|_|bad("invalid original SYSTEM ID"))?;
        let token:[u8;32]=bytes(5)?.try_into().map_err(|_|bad("invalid original SYSTEM material token"))?;
        if n(0)?!=i64::from(root.uid())||!(3..=64).contains(&n(1)?)||bytes(3)?!=root.path().as_os_str().as_bytes()
            ||n(4)?!=i64::from(root.byte_limit())||volume==self.pool||volume==[0;16]||token==[0;32]
            ||rows.next().map_err(sql)?.is_some(){return Err(bad("original SYSTEM claim does not match this selected directory"));}
        Ok(crate::sqlite_install::catalog::claimed_marker(0,&self.pool,&volume,&self.init,&token))
    }

    /// An exact historical result or genuine absence on this one original S,
    /// not authorization to ask for passwords, create either account or query
    /// a live server. The caller's original directory EX outlives SQLite close.
    pub(in crate::sqlite_install) fn observe_offline(&self,locked:&LockedSystem)->io::Result<History>{
        let template=Canonical::new()?;let source=locked.source();let db=Existing::observed_system(locked)?;
        // Hold the actual admitted private marker *before* SQLite opens. Inside
        // the transaction only statx its name and read its already-held FD;
        // never open/close another raw FD for the underlying database inode.
        let mark=SystemMark::before_sql(source)?;db.sidecars_absent()?;
        let mut c=engine::existing(&db,Access::Read)?;
        let tx=c.transaction().map_err(sql)?;template.require(&tx)?;
        let actual=self.first_marker(&tx,source)?;mark.verify(source,&actual)?;
        let result=self.original_after_init(&tx)?;
        mark.verify(source,&actual)?;engine::original_name(&tx)?;db.sidecars_absent()?;
        tx.commit().map_err(sql)?;c.close().map_err(|_|bad("original readonly account observation did not close cleanly"))?;
        locked.require()?;db.sidecars_absent()?;mark.verify(source,&actual)?;
        Ok(result)
    }
}
