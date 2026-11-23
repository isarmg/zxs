//! One pending SQL birth transaction. No marker, online-ready response or account
//! arises here. Retry lookup/completion is a separate action, never a fresh INIT.
use super::{
    bad, engine,
    originals::Group,
    request::{Id, Request, role},
    reservation::Reserved,
};
pub(super) use crate::fresh_random::operation as random;
use rusqlite::{Connection, TransactionBehavior, params};
use std::{io, os::unix::ffi::OsStrExt};
pub(super) const SCHEMA: &str = include_str!("schema.sql");

struct Issue {
    pool: Id,
    rows: Vec<(Id, [u8; 32])>,
}
pub(super) struct Pending<'g> {
    pub(super) pool: Id,
    pub(super) op: Id,
    source: Reserved<'g>,
    result: Vec<(Id, [u8; 32])>,
}
impl Pending<'_> {
    pub(super) fn require(
        &self,
        db: &super::reservation::Existing<'_>,
        cat: &super::catalog::Catalog,
        r: &Request,
    ) -> io::Result<()> {
        if self.op != r.op()
            || self.pool != cat.pool
            || cat.committed
            || !self.source.same_existing(db)
            || self.result.len() != cat.markers.len()
            || self
                .result
                .iter()
                .zip(&cat.markers)
                .any(|((v, t), m)| &m[37..53] != v.as_slice() || &m[69..] != t.as_slice())
        {
            return Err(bad(
                "fresh original database or its issued results changed before material enrollment",
            ));
        }
        Ok(())
    }
}
fn pending_uncertain(_: rusqlite::Error) -> io::Error {
    bad(
        "INIT transaction did not produce a confirmed pending receipt; retain the original operation and media",
    )
}

fn root_count(issue: &Issue) -> io::Result<i64> {
    i64::try_from(issue.rows.len()).map_err(|_| bad("invalid original claim count"))
}

fn ordinal(index: usize) -> io::Result<i64> {
    i64::try_from(index).map_err(|_| bad("invalid original claim ordinal"))
}

fn first<'g>(request: &Request, group: &'g Group, issue: &Issue) -> io::Result<Pending<'g>> {
    if group.limits().len() != issue.rows.len() || issue.rows.len() != request.paths().len() {
        return Err(bad("incomplete proposed original result"));
    }
    let reserved = Reserved::create(group, request)?;
    // Both the original group and its S OFD outlive this connection and all its
    // statements. No APIs on this path open/close another raw FD for the DB inode.
    let mut db = engine::open_reserved(&reserved)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(pending_uncertain)?;
    let objects: i64 = tx
        .query_row("SELECT count(*) FROM main.sqlite_schema", [], |r| r.get(0))
        .map_err(pending_uncertain)?;
    if objects != 0 {
        return Err(bad("unexpected schema in the freshly reserved original"));
    }
    tx.execute_batch(SCHEMA).map_err(pending_uncertain)?;
    let op = request.op();
    let actor = request.actor();
    let approval = request.approval();
    let sha = request.sha();
    let once = |rows: usize| {
        if rows == 1 {
            Ok(())
        } else {
            Err(bad("expected one original inserted row"))
        }
    };
    once(
        tx.execute(
            "INSERT INTO operation(op_id,kind,actor_id,request_sha,state) VALUES(?1,1,?2,?3,0)",
            params![&op[..], &actor[..], &sha[..]],
        )
        .map_err(pending_uncertain)?,
    )?;
    once(tx.execute("INSERT INTO pool_birth(singleton,pool_id,init_op,approval_ref,storage_uid,root_count) VALUES(1,?1,?2,?3,?4,?5)",
         params![&issue.pool[..],&op[..],&approval[..],i64::from(request.uid()),root_count(issue)?]).map_err(pending_uncertain)?)?;
    for (i, ((id, token), path)) in issue.rows.iter().zip(request.paths()).enumerate() {
        once(tx.execute("INSERT INTO volume_claim(claim_op,ordinal,role,volume_id,root_path,component_limit,claim_token) VALUES(?1,?2,?3,?4,?5,?6,?7)",
          params![&op[..],ordinal(i)?,i64::from(role(i)),&id[..],path.as_os_str().as_bytes(),i64::from(group.limits()[i]),&token[..]]).map_err(pending_uncertain)?)?;
    }
    group.recheck()?;
    reserved.require_name(false)?;
    engine::original_name(&tx)?;
    tx.commit().map_err(pending_uncertain)?;
    read_pending(&mut db, request, group, issue)?;
    engine::verified(&db, false)?;
    reserved.require_name(false)?;
    db.close()
        .map_err(|_| bad("SQLite close uncertain; retain the original"))?;
    reserved.sync_parent()?;
    group.scan_names(0, &[super::DATABASE.as_bytes()])?;
    for i in 1..issue.rows.len() {
        group.scan_names(i, &[])?;
    }
    group.recheck()?;
    reserved.require_name(false)?;
    Ok(Pending {
        pool: issue.pool,
        op,
        source: reserved,
        result: issue.rows.clone(),
    })
}

fn read_pending(
    db: &mut Connection,
    request: &Request,
    group: &Group,
    issue: &Issue,
) -> io::Result<()> {
    // A committed SQL pending birth is NOT an active pool. Query every bound
    // result in one fresh short read; do not call a marker a commit witness.
    let op = request.op();
    let actor = request.actor();
    let approval = request.approval();
    let sha = request.sha();
    let slice = db.transaction().map_err(pending_uncertain)?;
    let app: i64 = slice
        .query_row("PRAGMA main.application_id", [], |r| r.get(0))
        .map_err(pending_uncertain)?;
    let version: i64 = slice
        .query_row("PRAGMA main.user_version", [], |r| r.get(0))
        .map_err(pending_uncertain)?;
    if (app, version) != (0x5a58_5331, super::catalog::SCHEMA_VERSION) {
        return Err(bad("fresh database does not identify the agreed schema"));
    }
    let mut stmt = slice
        .prepare(include_str!("initial-catalog.sql"))
        .map_err(pending_uncertain)?;
    let mut rows = stmt.query([&issue.pool[..]]).map_err(pending_uncertain)?;
    for (i, ((id, token), path)) in issue.rows.iter().zip(request.paths()).enumerate() {
        let r = rows
            .next()
            .map_err(pending_uncertain)?
            .ok_or_else(|| bad("missing pending claim on independent readback"))?;
        let bytes = |col| r.get::<_, Vec<u8>>(col).map_err(pending_uncertain);
        let number = |col| r.get::<_, i64>(col).map_err(pending_uncertain);
        if bytes(0)? != issue.pool
            || bytes(1)? != op
            || bytes(2)? != actor
            || bytes(3)? != sha
            || number(4)? != 0
            || bytes(5)? != approval
            || number(6)? != i64::from(request.uid())
            || number(7)? != root_count(issue)?
            || number(8)? != ordinal(i)?
            || number(9)? != i64::from(role(i))
            || bytes(10)? != *id
            || bytes(11)? != path.as_os_str().as_bytes()
            || number(12)? != i64::from(group.limits()[i])
            || bytes(13)? != *token
        {
            return Err(bad(
                "pending SQL result does not match the original request and issued values",
            ));
        }
    }
    if rows.next().map_err(pending_uncertain)?.is_some() {
        return Err(bad("unexpected extra initial claim"));
    }
    drop(rows);
    drop(stmt);
    slice.commit().map_err(pending_uncertain)?;
    Ok(())
}

#[cfg(test)]
pub(super) fn seed_retirement_fixture(
    request: &Request,
    group: &Group,
) -> io::Result<(Id, Vec<Id>)> {
    // Ignored preflight source example: separate fresh disposable paths only.
    let issue = Issue {
        pool: [4; 16],
        rows: (0..request.paths().len())
            .map(|i| {
                (
                    [0x11 + u8::try_from(i).unwrap(); 16],
                    [0x21 + u8::try_from(i).unwrap(); 32],
                )
            })
            .collect(),
    };
    let pending = first(request, group, &issue)?;
    let done = super::complete::original(
        &super::catalog::Canonical::new()?,
        request,
        group,
        Some(&pending),
    )?;
    Ok((
        done.pool,
        issue.rows.iter().map(|(volume, _)| *volume).collect(),
    ))
}
