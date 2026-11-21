//! Shared SQLite authority for the original Rust issuance.
use super::{Id,bad};
mod first_ordinary;
use rusqlite::{params,Row,Transaction};
use std::io;
pub(super) mod transaction;
pub(in crate::sqlite_install::pair) mod rust_vault;
pub(in crate::sqlite_install) use transaction::WriteGate;
#[derive(Clone, Copy)]
struct Core { pool: Id, init: Id, op: Id, actor: Id, approval: Id }
impl Core {
    fn new(pool: Id, init: Id, op: Id, actor: Id, approval: Id) -> io::Result<Self> {
        if op == init || [pool, init, op, actor, approval].contains(&[0; 16]) {
            return Err(bad("expected the original nonzero pool, INIT and maintenance request"));
        }
        Ok(Self { pool, init, op, actor, approval })
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Ids { web: Id, smb: Id }
impl Ids {
    pub(super) fn checked(web: Id, smb: Id) -> io::Result<Self> {
        if [web, smb].contains(&[0; 16]) || web == smb { return Err(bad("invalid fixed account IDs")); }
        Ok(Self { web, smb })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Original<T> {
    Absent,
    Pending(T), // explicitly NOT a history receipt or authorization
    Committed(T),
}
fn sql(_: rusqlite::Error) -> io::Error { bad("cannot establish the exact original storage account operation") }
fn blob<const N: usize>(r: &Row<'_>, index: usize) -> io::Result<[u8; N]> {
    r.get_ref(index).map_err(sql)?.as_blob().map_err(|_| bad("incorrect storage account SQL BLOB type"))?
        .try_into().map_err(|_| bad("incorrect storage account SQL BLOB length"))
}
fn history<T>(state: i64, value: T) -> io::Result<Original<T>> {
    match state {
        0 => Ok(Original::Pending(value)),
        1 => Ok(Original::Committed(value)),
        _ => Err(bad("unsupported account maintenance state")),
    }
}
fn collision(tx: &Transaction<'_>, op: &Id) -> io::Result<bool> {
    tx.query_row("SELECT EXISTS(SELECT 1 FROM main.operation WHERE op_id=?1)",
        [&op[..]], |r| r.get(0)).map_err(sql)
}
fn one(result: rusqlite::Result<usize>) -> io::Result<()> {
    if result.map_err(sql)? == 1 { Ok(()) } else { Err(bad("account operation affected an unexpected number of rows")) }
}
fn stage(tx: &Transaction<'_>, c: &Core, kind: i64, sha: &[u8; 32]) -> io::Result<()> {
    one(tx.execute("INSERT INTO main.operation(op_id,kind,actor_id,request_sha,state) VALUES(?1,?2,?3,?4,0)",
        params![&c.op[..], kind, &c.actor[..], &sha[..]]))
}
