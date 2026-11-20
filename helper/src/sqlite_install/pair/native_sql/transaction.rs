//! Real v9 SQL phases for the dedicated offline storage-UID worker. No
//! native password, hash, SAM buffer, TDB file, or writable host descriptor
//! is an input. The original Unix VFS inode remains anchored across phases.
use super::{Birth, Core, Ids, Original, Reset, activate, first_ordinary::Selected, sql};
use crate::smb_profile::first::Plan;
use crate::sqlite_install::{DATABASE, MARKER, bad, catalog::Canonical, engine, markers::SystemMark,
    originals::LockedSystem, pair::{Id, observed, web_secret::PreparedWeb}, reservation::Existing};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use std::{io, time::Instant};

pub(in crate::sqlite_install::pair) enum Order { Setup(Birth), Reset(Reset), WebReset(super::web_reset::Reset) }

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub(in crate::sqlite_install::pair) enum ResultRow { Setup(Ids), Reset(i64) }

impl Order {
    fn core(&self) -> &Core { match self { Self::Setup(r) => &r.scope, Self::Reset(r) => &r.scope, Self::WebReset(r)=>r.core() } }
    pub(in crate::sqlite_install::pair) fn sha(&self) -> [u8; 32] { match self { Self::Setup(r) => r.sha(), Self::Reset(r) => r.sha(), Self::WebReset(r)=>r.sha() } }
    fn original(&self, tx: &Transaction<'_>) -> io::Result<Original<ResultRow>> {
        fn map<T, U>(value: Original<T>, f: impl FnOnce(T) -> U) -> Original<U> {
            match value { Original::Absent => Original::Absent, Original::Pending(v) => Original::Pending(f(v)),
                Original::Committed(v) => Original::Committed(f(v)) }
        }
        match self {
            Self::Setup(r) => Ok(map(r.original(tx)?, ResultRow::Setup)),
            Self::Reset(r) => Ok(map(r.original(tx)?, ResultRow::Reset)),
            Self::WebReset(r)=>Ok(map(r.original(tx)?,ResultRow::Reset)),
        }
    }
    fn free_to_start(&self, tx: &Transaction<'_>) -> io::Result<()> {
        if self.original(tx)? != Original::Absent {
            return Err(bad("this exact order already has history; do not start it again"));
        }
        if let Self::Reset(r) = self {
            // account()/retained_previous() above established the original
            // ID, request and generation. Check *current* eligibility before
            // questioning the operator. The canonical trigger repeats at INSERT.
            let active:bool=tx.query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM main.credential_reset
                   WHERE role=1 AND generation>?1)",
                [r.previous_generation], |row| row.get(0)).map_err(sql)?;
            if !active { return Err(bad("the original SMB predecessor is not the current active credential")); }
        }
        Ok(())
    }
    fn reserve(&self, tx: &Transaction<'_>, material: Enrollment<'_>) -> io::Result<ResultRow> {
        self.free_to_start(tx)?;
        match (self, material) {
            (Self::Setup(r), Enrollment::Setup { ids, web }) => { r.reserve(tx, ids, web)?; Ok(ResultRow::Setup(ids)) }
            (Self::Reset(r), Enrollment::Reset) => { r.reserve(tx)?; Ok(ResultRow::Reset(r.generation())) }
            _ => Err(bad("unrelated credential input cannot reserve this order")),
        }
    }
    fn activate(&self, tx: &Transaction<'_>, row: ResultRow) -> io::Result<()> {
        if self.original(tx)? != Original::Pending(row) {
            return Err(bad("no matching exact original pending credential to finish"));
        }
        match (self, row) {
            (Self::Reset(r), ResultRow::Reset(generation)) if generation==r.generation() =>
                activate(tx, &r.scope, r.setup, &r.binding, &r.sha(), r.account, &r.login, generation, Some(r.previous)),
            _ => Err(bad("wrong pending credential result for this operation")),
        }
    }
    pub(in crate::sqlite_install::pair) fn target(&self, row: ResultRow)
        -> io::Result<(crate::sqlite_install::pair::native_epoch::ExpectedMarkers, &[u8])> {
        match (self,row) {
            (Self::Setup(r),ResultRow::Setup(ids))=>Ok((r.markers(ids)?,&r.smb)),
            (Self::Reset(r),ResultRow::Reset(n)) if n==r.generation()=>Ok((r.markers()?,&r.login)),
            _=>Err(bad("wrong role's result for the native account")),
        }
    }
}

pub(in crate::sqlite_install::pair) enum Enrollment<'a> { Setup { ids: Ids, web: &'a PreparedWeb }, Reset }

/// Supervisor authorization is still a separate lifetime from this live SQL
/// anchor, and will arrive via the fixed worker channel. Dynamic reply
/// authorization is not derivable from Connection, uid, or a decoded PENDING.
/// Only the private native worker or role-specific Web reset invokes it.
pub(in crate::sqlite_install::pair) struct Session<'s> {
    order: Order, locked: &'s LockedSystem, database: Existing<'s>, schema: Canonical,
    marker: SystemMark, want: [u8;101], anchor: Connection, first: Option<(&'s Plan, &'s [u8;128])>,
}

pub(in crate::sqlite_install) struct WriteGate<'a,'s> {
    session: &'a Session<'s>, until: Instant,
}

fn expired(until:Instant)->io::Result<()> {
    if Instant::now()>=until { Err(bad("the current offline credential operation expired")) } else { Ok(()) }
}

impl<'s> Session<'s> {
    pub(in crate::sqlite_install::pair) fn open(locked: &'s LockedSystem, order: Order) -> io::Result<Self> {
        if !matches!(&order,Order::WebReset(_)){return Err(bad("native maintenance requires a bound first plan"));}
        Self::at(locked,order,None)
    }
    pub(in crate::sqlite_install::pair) fn open_native(locked:&'s LockedSystem,order:Order,plan:&'s Plan,bound:&'s [u8;128])->io::Result<Self>{
        let first=match &order {
            Order::Setup(birth)=>{Selected::bind(birth,bound,plan)?;Some((plan,bound))}
            // Scope and current BOUND are matched by the sealed native request;
            // resetting never invokes setup publication or restores initial modes.
            Order::Reset(_)=>None,
            _=>return Err(bad("wrong native maintenance role")),
        };
        Self::at(locked,order,first)
    }
    fn at(locked:&'s LockedSystem,order:Order,first:Option<(&'s Plan,&'s [u8;128])>)->io::Result<Self>{
        if !locked.allows_account_write(){return Err(bad("a runtime shared lease is not authorized for account maintenance"));}
        let schema=Canonical::new()?;
        let database=Existing::observed_system(locked)?;
        let marker=SystemMark::before_sql(locked.source())?;
        let mut anchor=engine::existing(&database,engine::Access::Read)?;
        let c=order.core();
        let (want,())=observed::snapshot(&mut anchor,&c.pool,&c.init,locked,&database,&schema,&marker,|_|Ok(()))?;
        Ok(Self { order, locked, database, schema, marker, want, anchor, first })
    }
    fn initial(&self)->io::Result<Option<Selected<'_>>>{
        match (&self.order,self.first) {
            (Order::Setup(birth),Some((plan,bound)))=>Ok(Some(Selected::bind(birth,bound,plan)?)),
            (Order::Setup(_),None)|(_,Some(_))=>Err(bad("missing or unrelated first plan authority")),
            (_,None)=>Ok(None),
        }
    }
    pub(in crate::sqlite_install::pair) fn order(&self) -> &Order { &self.order }
    fn unchanged(&self) -> io::Result<()> {
        if !self.anchor.is_autocommit() { return Err(bad("original anchor has a transaction across maintenance actions")); }
        engine::original_name(&self.anchor)?;
        self.locked.require()?;
        self.database.require()?;
        self.marker.verify(self.locked.source(),&self.want)
    }
    fn clean(&self) -> io::Result<()> { self.unchanged()?; self.database.sidecars_absent() }
    fn read<T>(&self,f:impl FnOnce(&Transaction<'_>)->io::Result<T>)->io::Result<T>{
        self.clean()?;
        let c=self.order.core();
        let (mark,value)=observed::at_original(&c.pool,&c.init,self.locked,&self.database,&self.schema,&self.marker,f)?;
        self.clean()?;
        if mark!=self.want { return Err(bad("the original SYSTEM claim changed during credential maintenance")); }
        Ok(value)
    }
    pub(in crate::sqlite_install::pair) fn original(&self) -> io::Result<Original<ResultRow>> {
        self.read(|tx|{
            let result=self.order.original(tx)?;
            if let (Some(first),Original::Committed(ResultRow::Setup(ids)))=(self.initial()?,result){first.history(tx,ids)?;}
            Ok(result)
        })
    }
    fn check_new_in(&self,tx:&Transaction<'_>)->io::Result<()>{
        self.order.free_to_start(tx)?;
        if let Some(first)=self.initial()?{first.vacant(tx)?;}
        Ok(())
    }
    pub(in crate::sqlite_install::pair) fn check_new(&self) -> io::Result<()> {
        self.read(|tx|self.check_new_in(tx))
    }

    pub(in crate::sqlite_install::pair) fn sync_pending(&self, row: ResultRow, until: Instant)
        -> io::Result<Pending<'_, 's>> {
        expired(until)?;
        self.clean()?;
        // A previously visible PENDING is not a previous successful fsync.
        // Only explicit current authority uses this exact VFS main handle,
        // then the original parent, then an independent SQLite open.
        helper_linux_evidence::sqlite_vfs::sync_held_unix_main(&self.anchor)?;
        self.locked.source().dir().sync_all()?;
        if self.original()?!=Original::Pending(row) { return Err(uncertain()); }
        expired(until)?;
        Ok(Pending { session:self, row, until })
    }
    pub(in crate::sqlite_install::pair) fn reserve(&self, material: Enrollment<'_>, until: Instant,
        live: impl Fn()->io::Result<()>) -> io::Result<Pending<'_, 's>> {
        let row=self.write(until,live,|tx|{self.check_new_in(tx)?;self.order.reserve(tx,material)})?;
        self.sync_pending(row,until).map_err(|_|uncertain())
    }
    pub(in crate::sqlite_install::pair) fn web_eligible(&self)->io::Result<Vec<u8>>{
        let Order::WebReset(r)=&self.order else{return Err(bad("wrong dedicated Web action"));};
        self.read(|tx|r.eligible(tx))
    }
    pub(in crate::sqlite_install::pair) fn reset_web(&self,material:&PreparedWeb,until:Instant,live:impl Fn()->io::Result<()>)
        ->io::Result<i64>{
        let Order::WebReset(r)=&self.order else{return Err(bad("wrong dedicated Web action"));};
        let generation=self.write(until,live,|tx|r.update(tx,material))?;
        (||->io::Result<i64>{
            helper_linux_evidence::sqlite_vfs::sync_held_unix_main(&self.anchor)?;
            self.locked.source().dir().sync_all()?;
            if self.original()?!=Original::Committed(ResultRow::Reset(generation)){return Err(uncertain());}
            Ok(generation)
        })().map_err(|_|uncertain())
    }
    // Pre- and post-commit concerns are separate. An unknown commit result
    // stops at the original operation, while the RO anchor still pins SQLite.
    fn write<T>(&self,until:Instant,confirm:impl Fn()->io::Result<()>,
        f:impl FnOnce(&Transaction<'_>)->io::Result<T>)->io::Result<T>{
        confirm()?;
        let gate=WriteGate { session:self, until };
        let mut c=engine::native_pair(&gate)?;
        let tx=c.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql)?;
        self.schema.require(&tx)?;
        let r=self.order.core();let root=self.locked.source();
        if observed::first_marker(&r.pool,&r.init,&tx,root)?!=self.want {
            return Err(bad("wrong original SYSTEM claim inside the credential transaction"));
        }
        gate.database()?;engine::original_name(&tx)?;confirm()?;
        let result=f(&tx)?;
        expired(until)?;
        self.unchanged()?;engine::original_name(&tx)?;
        let seen=root.controls(&[DATABASE.as_bytes(),MARKER,b"SYSTEM.sqlite3-journal"])?;
        if !seen[0]||!seen[1]{return Err(bad("original SQLite or SYSTEM marker missing before commit"));}
        confirm()?;
        tx.commit().map_err(|_|uncertain())?;
        c.close().map_err(|_|uncertain())?;
        Ok(result)
    }
}
impl<'s> WriteGate<'_, 's> {
    pub(in crate::sqlite_install) fn database(&self)->io::Result<&Existing<'s>> {
        expired(self.until)?;
        self.session.clean()?;
        use rustix::fs::{StatVfsMountFlags, fstatvfs};
        if fstatvfs(self.session.locked.source().dir())?.f_flag.contains(StatVfsMountFlags::RDONLY) {
            return Err(bad("cannot write the original SYSTEM mount"));
        }
        Ok(&self.session.database)
    }
}

pub(in crate::sqlite_install::pair) struct Pending<'a,'s> { session:&'a Session<'s>, row:ResultRow, until:Instant }
impl Pending<'_, '_> {
    pub(in crate::sqlite_install::pair) fn row(&self) -> ResultRow { self.row }
    pub(in crate::sqlite_install::pair) fn require(&self)->io::Result<()> {
        expired(self.until)?;
        if self.session.original()?!=Original::Pending(self.row) { return Err(uncertain()); }
        Ok(())
    }
    // The fixed transport calls this only after the verified native step:
    // actual native synchronizations and independent reread, proof scoped
    // to this one process, one request and one same held native original.
    pub(in crate::sqlite_install::pair) fn finish(self, confirmed:&super::super::native_run::Synced,
        live: impl Fn()->io::Result<()>) ->io::Result<ResultRow> {
        self.require()?;
        confirmed.require(&self.session.order.sha(),self.row)?;
        self.session.write(self.until,||{
            live()?;
            confirmed.require(&self.session.order.sha(),self.row)
        },|tx|match (self.session.initial()?,self.row) {
            (Some(first),ResultRow::Setup(ids))=>first.finish(tx,ids),
            (None,ResultRow::Reset(_))=>self.session.order.activate(tx,self.row),
            _=>Err(bad("no matching approved first plan or exact reset")),
        })?;
        (||->io::Result<ResultRow>{
            helper_linux_evidence::sqlite_vfs::sync_held_unix_main(&self.session.anchor)?;
            self.session.locked.source().dir().sync_all()?;
            if self.session.original()?!=Original::Committed(self.row){return Err(uncertain());}
            confirmed.require(&self.session.order.sha(),self.row)?;
            live()?;
            Ok(self.row)
        })().map_err(|_|uncertain())
    }
}

fn uncertain()->io::Error{
    bad("offline native credential not confirmed; preserve both originals and query only the exact original operation")
}
