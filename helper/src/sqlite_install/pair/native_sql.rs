//! Public request bytes and exact v7 transaction components. Parsing and
//! original SQL history cannot issue native account-maintenance authority.
use super::{Id, bad, login, distinct_roles, domain::LocalDomain};
use rusqlite::{params, Row, Transaction};
use sha2::{Digest, Sha256};
use std::io;

pub(super) mod transaction;
pub(super) mod web_reset;
pub(in crate::sqlite_install) use transaction::WriteGate;

const FIRST_LOOKUP: &str = include_str!("native_sql/original-first.sql");
const RESET_LOOKUP: &str = include_str!("native_sql/original-reset.sql");
const ACTIVATE: &str = include_str!("native_sql/activate-native.sql");

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
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) struct Binding { pool: Id, instance: Id, sha: [u8; 32], sid: [u8; 28] }

impl Binding {
    /// Parsing these selected public bytes establishes the request fields,
    /// not the origin of the file or the live Samba domain. The caller must
    /// already be using the exact separately protected installation record.
    pub(super) fn expected(bytes: &[u8; 128], pool: &Id, instance: &Id, original_provision: &Id,
        live_configuration: &[u8; 32]) -> io::Result<Self> {
        let domain = LocalDomain::expected_binding(bytes, pool, instance, original_provision, live_configuration)?;
        Ok(Self { pool: *pool, instance: *instance, sha: Sha256::digest(bytes).into(),
            sid: domain.expected_smb_sid() })
    }
}

pub(super) struct Birth { scope: Core, binding: Binding, web: Vec<u8>, smb: Vec<u8> }
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Ids { web: Id, smb: Id }
impl Ids {
    pub(super) fn web_value(self) -> Id { self.web }
    pub(super) fn smb_value(self) -> Id { self.smb }
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
fn sql(_: rusqlite::Error) -> io::Error { bad("cannot establish the exact original native account operation") }
fn blob<const N: usize>(r: &Row<'_>, index: usize) -> io::Result<[u8; N]> {
    r.get_ref(index).map_err(sql)?.as_blob().map_err(|_| bad("incorrect native account SQL BLOB type"))?
        .try_into().map_err(|_| bad("incorrect native account SQL BLOB length"))
}
fn history<T>(state: i64, value: T) -> io::Result<Original<T>> {
    match state {
        0 => Ok(Original::Pending(value)),
        1 => Ok(Original::Committed(value)),
        _ => Err(bad("unsupported account maintenance state")),
    }
}
fn require_init(tx: &Transaction<'_>, c: &Core) -> io::Result<()> {
    // Defence against interpreting an empty/different pool as an absent
    // account. The calling protected transaction still verifies exact v7
    // canonical schema, original S marker and normal upstream Unix VFS.
    let init: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.pool_birth b
           JOIN main.operation i ON i.op_id=b.init_op AND i.kind=1 AND i.state=1
           WHERE b.singleton=1 AND b.pool_id=?1 AND b.init_op=?2)",
        params![&c.pool[..], &c.init[..]], |r| r.get(0)).map_err(sql)?;
    if !init { return Err(bad("there is no such confirmed original pool")); }
    Ok(())
}
fn collision(tx: &Transaction<'_>, op: &Id) -> io::Result<bool> {
    tx.query_row("SELECT EXISTS(SELECT 1 FROM main.operation WHERE op_id=?1)",
        [&op[..]], |r| r.get(0)).map_err(sql)
}
impl Birth {
    pub(super) fn new(pool: Id, init: Id, op: Id, actor: Id, approval: Id, binding: Binding,
        web: &[u8], smb: &[u8]) -> io::Result<Self> {
        let scope = Core::new(pool, init, op, actor, approval)?;
        if binding.pool != pool || !login(web) || !login(smb) || !distinct_roles(web, smb) {
            return Err(bad("wrong dedicated binding or two invalid protocol names"));
        }
        Ok(Self { scope, binding, web: web.to_vec(), smb: smb.to_vec() })
    }
    pub(super) fn preview(&self) -> (&Id,&Id,&[u8],&[u8]) {
        (&self.scope.op,&self.scope.approval,&self.web,&self.smb)
    }
    pub(super) fn sha(&self) -> [u8; 32] {
        let c = self.scope;
        let mut h = Sha256::new();
        h.update(b"ZXS-SQLITE-1/NATIVE-PAIR\0");
        for id in [c.pool, c.init, c.op, c.actor, c.approval, self.binding.instance] { h.update(id); }
        h.update(self.binding.sha);
        for name in [&self.web, &self.smb] { h.update([name.len() as u8]); h.update(name); }
        h.update(self.binding.sid);
        h.finalize().into()
    }

    pub(super) fn original(&self, tx: &Transaction<'_>) -> io::Result<Original<Ids>> {
        let c = self.scope;
        require_init(tx, &c)?;
        let digest = self.sha();
        let mut stmt = tx.prepare(FIRST_LOOKUP).map_err(sql)?;
        let mut rows = stmt.query(params![&c.pool[..], &c.op[..], &c.actor[..], &digest[..], &c.init[..],
            &c.approval[..], &self.web, &self.smb, &self.binding.sid[..], &self.binding.instance[..],
            &self.binding.sha[..]]).map_err(sql)?;
        if let Some(row) = rows.next().map_err(sql)? {
            let found = history(row.get(0).map_err(sql)?, Ids::checked(blob(row, 1)?, blob(row, 2)?)?)?;
            if rows.next().map_err(sql)?.is_some() { return Err(bad("ambiguous original first account pair")); }
            return Ok(found);
        }
        let occupied: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.operation INDEXED BY singleton_bootstraps WHERE kind IN (1,2) AND kind=2)
               OR EXISTS(SELECT 1 FROM main.account_pair) OR EXISTS(SELECT 1 FROM main.web_password)",
            [], |r| r.get(0)).map_err(sql)?;
        if occupied || collision(tx, &c.op)? { return Err(bad("conflicting first account history; not an absent order")); }
        Ok(Original::Absent)
    }

    pub(super) fn markers(&self, ids: Ids) -> io::Result<super::native_epoch::ExpectedMarkers> {
        let c = self.scope;
        let e = super::native_epoch::Epoch::new(c.pool, self.binding.instance, ids.smb, c.op, 1)?;
        super::native_epoch::ExpectedMarkers::new(None, e)
    }

    /// Only a private transaction component, NOT a maintenance entry point.
    /// The eventual caller must independently authorize the exact choices,
    /// verify fresh native media before prompting, retain both original EX
    /// leases, conditionally reserve this order, COMMIT and reread it from the
    /// original durable SQL database before making any native change.
    fn reserve(&self, tx: &Transaction<'_>, ids: Ids, web: &super::web_secret::PreparedWeb) -> io::Result<()> {
        if self.original(tx)? != Original::Absent { return Err(bad("the original setup is not vacant")); }
        let c = self.scope;
        one(tx.execute("INSERT INTO main.account_pair(singleton,birth_op,approval_ref,instance_id,setup_bound_sha,
                web_id,web_login,smb_id,smb_login,smb_sid) VALUES(1,?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![&c.op[..], &c.approval[..], &self.binding.instance[..], &self.binding.sha[..],
                &ids.web[..], &self.web, &ids.smb[..], &self.smb, &self.binding.sid[..]]))?;
        one(web.insert(tx, &c.op))?;
        stage(tx, &c, 2, &self.sha())?;
        if self.original(tx)? != Original::Pending(ids) {
            return Err(bad("incomplete original setup inside its pending transaction"));
        }
        Ok(())
    }
}

pub(super) struct Reset {
    scope: Core, setup: Id, account: Id, previous: Id, previous_generation: i64,
    binding: Binding, login: Vec<u8>,
}
impl Reset {
    pub(super) fn smb(pool: Id, init: Id, setup: Id, op: Id, actor: Id, approval: Id, account: Id,
        previous: Id, previous_generation: i64, binding: Binding, login_name: &[u8]) -> io::Result<Self> {
        let scope = Core::new(pool, init, op, actor, approval)?;
        if [setup, account, previous].contains(&[0; 16]) || [setup, previous].contains(&op)
            || !(1..i64::MAX).contains(&previous_generation) || binding.pool != pool
            || (previous_generation == 1) != (previous == setup)
            || !login(login_name) || !distinct_roles(&[], login_name) {
            return Err(bad("invalid original SMB reset identity or predecessor"));
        }
        Ok(Self { scope, setup, account, previous, previous_generation, binding, login: login_name.to_vec() })
    }
    pub(super) fn sha(&self) -> [u8; 32] {
        let c = self.scope;
        let mut h = Sha256::new();
        h.update(b"ZXS-SQLITE-1/RESET-SMB\0");
        for id in [c.pool, c.init, self.setup, c.op, c.actor, c.approval, self.account, self.previous] {
            h.update(id);
        }
        h.update(self.previous_generation.to_be_bytes());
        h.update(self.binding.instance);
        h.update(self.binding.sha);
        h.finalize().into()
    }
    pub(super) fn generation(&self) -> i64 { self.previous_generation + 1 }
    fn account(&self, tx: &Transaction<'_>) -> io::Result<()> {
        let c = self.scope;
        // Validate the original public setup row, not merely a name+SID which
        // can be self-consistent in a corrupt catalogue. Bind the current
        // installation's pool, instance and immutable user. The historical
        // setup hash used its own BOUND; an approved configuration change can
        // have a different BOUND for this reset without changing that history.
        let mut stmt = tx.prepare(
            "SELECT b.actor_id,b.request_sha,a.approval_ref,a.setup_bound_sha,a.web_login,
                    a.web_id
               FROM main.account_pair a JOIN main.operation b ON b.op_id=a.birth_op
              WHERE a.singleton=1 AND b.kind=2 AND b.state=1 AND a.birth_op=?1
                AND a.smb_id=?2 AND a.smb_login=?3 AND a.smb_sid=?4 AND a.instance_id=?5").map_err(sql)?;
        let mut rows = stmt.query(params![&self.setup[..], &self.account[..], &self.login,
            &self.binding.sid[..], &self.binding.instance[..]]).map_err(sql)?;
        let row = rows.next().map_err(sql)?.ok_or_else(||bad("wrong confirmed original native SMB identity"))?;
        let recorded: [u8; 32] = blob(row, 1)?;
        let historical = Binding { sha: blob(row, 3)?, ..self.binding };
        let web = row.get_ref(4).map_err(sql)?.as_blob().map_err(|_|bad("wrong original Web login type"))?;
        Ids::checked(blob(row, 5)?, self.account)?;
        if Birth::new(c.pool, c.init, self.setup, blob(row, 0)?, blob(row, 2)?, historical, web, &self.login)?
            .sha() != recorded || rows.next().map_err(sql)?.is_some() {
            return Err(bad("inconsistent original setup request for the SMB identity"));
        }
        self.retained_previous(tx)
    }

    fn retained_previous(&self, tx: &Transaction<'_>) -> io::Result<()> {
        if self.previous_generation == 1 {
            // Construction tied this predecessor to the exact SETUP just checked.
            return Ok(());
        }
        let c = self.scope;
        let mut stmt = tx.prepare(
            "SELECT o.actor_id,o.request_sha,r.approval_ref,r.bound_sha,r.previous_op
               FROM main.credential_reset r JOIN main.operation o ON o.op_id=r.op_id
              WHERE r.op_id=?1 AND r.role=1 AND r.generation=?2 AND o.kind=7 AND o.state=1
                AND EXISTS(SELECT 1 FROM main.credential_epoch q JOIN main.operation p ON p.op_id=q.credential_op
                  WHERE q.role=1 AND q.credential_op=r.previous_op AND q.generation=r.generation-1
                    AND p.kind=CASE WHEN q.generation=1 THEN 2 ELSE 7 END AND p.state=1)").map_err(sql)?;
        let mut rows = stmt.query(params![&self.previous[..], self.previous_generation]).map_err(sql)?;
        let row = rows.next().map_err(sql)?.ok_or_else(||bad("no exact committed original SMB predecessor"))?;
        let stored = Binding { sha: blob(row, 3)?, ..self.binding };
        let prior = Self::smb(c.pool, c.init, self.setup, self.previous, blob(row, 0)?, blob(row, 2)?,
            self.account, blob(row, 4)?, self.previous_generation - 1, stored, &self.login)?;
        if prior.sha() != blob::<32>(row, 1)? || rows.next().map_err(sql)?.is_some() {
            return Err(bad("inconsistent previous original SMB maintenance request"));
        }
        Ok(())
    }
    pub(super) fn original(&self, tx: &Transaction<'_>) -> io::Result<Original<i64>> {
        let c = self.scope;
        require_init(tx, &c)?;
        self.account(tx)?;
        let digest = self.sha();
        let mut stmt = tx.prepare(RESET_LOOKUP).map_err(sql)?;
        let mut rows = stmt.query(params![&c.pool[..], &c.init[..], &self.setup[..], &c.op[..], &c.actor[..],
            &digest[..], &c.approval[..], 1i64, &self.account[..], &self.previous[..], self.generation(),
            &self.binding.instance[..], &self.binding.sha[..]]).map_err(sql)?;
        if let Some(row) = rows.next().map_err(sql)? {
            let generation = row.get::<_,i64>(1).map_err(sql)?;
            let found = history(row.get(0).map_err(sql)?, generation)?;
            if generation != self.generation() || rows.next().map_err(sql)?.is_some() {
                return Err(bad("ambiguous or wrong original SMB reset result"));
            }
            return Ok(found);
        }
        if collision(tx, &c.op)? { return Err(bad("operation ID already belongs to an unmatching order")); }
        Ok(Original::Absent)
    }

    pub(super) fn markers(&self) -> io::Result<super::native_epoch::ExpectedMarkers> {
        let c = self.scope;
        let epoch = |op, generation| super::native_epoch::Epoch::new(c.pool, self.binding.instance, self.account, op, generation);
        let previous = epoch(self.previous, self.previous_generation as u64)?;
        let target = epoch(c.op, self.generation() as u64)?;
        super::native_epoch::ExpectedMarkers::new(Some(previous), target)
    }

    /// No comparison, read, rewrite or use of another protocol's password.
    fn reserve(&self, tx: &Transaction<'_>) -> io::Result<()> {
        if self.original(tx)? != Original::Absent { return Err(bad("the exact reset request is not vacant")); }
        let c = self.scope;
        // The common trigger requires the exact still-active predecessor,
        // nonforking generation, and wrong-scope/duplicate Op exclusion.
        one(tx.execute("INSERT INTO main.credential_reset(op_id,role,previous_op,generation,approval_ref,bound_sha)
                VALUES(?1,1,?2,?3,?4,?5)",
            params![&c.op[..], &self.previous[..], self.generation(), &c.approval[..], &self.binding.sha[..]]))?;
        stage(tx, &c, 7, &self.sha())?;
        if self.original(tx)? != Original::Pending(self.generation()) {
            return Err(bad("incomplete original reset inside its pending transaction"));
        }
        Ok(())
    }
}

fn one(result: rusqlite::Result<usize>) -> io::Result<()> {
    if result.map_err(sql)? == 1 { Ok(()) } else { Err(bad("account operation affected an unexpected number of rows")) }
}
fn stage(tx: &Transaction<'_>, c: &Core, kind: i64, sha: &[u8; 32]) -> io::Result<()> {
    one(tx.execute("INSERT INTO main.operation(op_id,kind,actor_id,request_sha,state) VALUES(?1,?2,?3,?4,0)",
        params![&c.op[..], kind, &c.actor[..], &sha[..]]))
}

// Do NOT call this until the eventual native child/sync/independent native
// reread has supplied a real sealed TDB result. No production call exists in
// this candidate. A row is only a conditional update, not a COMMIT receipt.
fn activate(tx: &Transaction<'_>, c: &Core, setup: Id, binding: &Binding,
    request_sha: &[u8; 32], account: Id, login: &[u8], generation: i64, previous: Option<Id>) -> io::Result<()> {
    let mut statement = tx.prepare(ACTIVATE).map_err(sql)?;
    let mut rows = statement.query(params![&c.pool[..], &c.init[..], &setup[..], &c.op[..], &c.actor[..],
        &request_sha[..], &c.approval[..], &binding.instance[..], &binding.sha[..], &account[..],
        login, &binding.sid[..], generation, previous.as_ref().map(|v| &v[..])]).map_err(sql)?;
    let changed: Id = blob(rows.next().map_err(sql)?.ok_or_else(||bad("native original was not pending"))?, 0)?;
    if changed != c.op || rows.next().map_err(sql)?.is_some() { return Err(bad("ambiguous native confirmation")); }
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/sqlite/native_request.rs"]
mod tests;
