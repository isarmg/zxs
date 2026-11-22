//! Independently held original Unix VFS between short SQLite transactions.
use super::{sql,Id};
use crate::sqlite_install::{DATABASE,MARKER,bad,catalog::Canonical,engine,markers::SystemMark,
    originals::LockedSystem,pair::observed,reservation::Existing};
use rusqlite::{Connection,Transaction,TransactionBehavior};
use std::{io,time::Instant};
pub(in crate::sqlite_install) struct WriteGate<'a,'s>{ source:&'a AccountCut<'s>, until:Instant }
pub(super) fn expired(until:Instant)->io::Result<()> {
    if Instant::now()>=until { Err(bad("the current offline credential operation expired")) } else { Ok(()) }
}

pub(super) struct AccountCut<'s>{
    locking:&'s LockedSystem, database:Existing<'s>,schema:Canonical,marker:SystemMark,want:[u8;101],
    anchor:Connection,pool:Id,init:Id,
}
impl<'s> AccountCut<'s>{
    pub(super) fn at(locked:&'s LockedSystem,pool:Id,init:Id)->io::Result<Self>{
        if !locked.allows_account_write(){return Err(bad("a runtime shared lease is not authorized for account maintenance"));}
        let schema=Canonical::new()?;
        let database=Existing::observed_system(locked)?;
        let marker=SystemMark::before_sql(locked.source())?;
        let mut anchor=engine::existing(&database,engine::Access::Read)?;
        let (want,())=observed::OriginalRead::new(locked,&database,&schema,&marker).snapshot(&mut anchor,&pool,&init,|_|Ok(()))?;
        Ok(Self { locking:locked, database, schema, marker, want, anchor, pool, init })
    }
    fn unchanged(&self) -> io::Result<()> {
        if !self.anchor.is_autocommit() { return Err(bad("original anchor has a transaction across maintenance actions")); }
        engine::original_name(&self.anchor)?;
        self.locking.require()?;
        self.database.require()?;
        self.marker.verify(self.locking.source(),&self.want)
    }
    fn clean(&self) -> io::Result<()> { self.unchanged()?; self.database.sidecars_absent() }
    pub(super) fn deployed_origin(&self,tx:&Transaction<'_>,actor:&Id)->io::Result<()>{
        let found:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM main.pool_birth b
            JOIN main.operation i ON i.op_id=b.init_op AND i.kind=1 AND i.state=1 AND i.actor_id=?1
            WHERE b.singleton=1 AND b.pool_id=?2 AND b.init_op=?3 AND b.storage_uid=?4)",
            rusqlite::params![&actor[..],&self.pool[..],&self.init[..],i64::from(self.locking.source().uid())],|r|r.get(0)).map_err(sql)?;
        if !found{return Err(bad("independently protected site and original INIT principal disagree"));}
        crate::sqlite_install::catalog::initial_metadata(tx,&self.schema,self.locking.source(),&self.pool,&self.init)?;
        Ok(())
    }
    pub(super) fn fresh_origin(&self,expected:[u64;3])->io::Result<()>{self.clean()?;self.database.matches_fresh_origin(expected)}
    pub(super) fn original_objects(&self)
        ->io::Result<(std::fs::File,[u8;64],crate::sqlite_install::markers::SystemObservation)>{
        use crate::sqlite_install::pair::fresh_issuer::InitialDatabase;
        self.clean()?;
        let pin=self.database.fresh_observation_pin()?;
        let source=self.locking.source();
        let stamp=InitialDatabase::observation_on(&pin,source.uid())?;
        let marker=self.marker.observe(source,&self.want)?;
        self.clean()?;
        if InitialDatabase::observation_on(&pin,source.uid())?!=stamp{return Err(bad("original account database metadata changed"));}
        Ok((pin,stamp,marker))
    }
    pub(super) fn read<T>(&self,f:impl FnOnce(&Transaction<'_>)->io::Result<T>)->io::Result<T>{
        self.clean()?;
        let (mark,value)=observed::at_original(&self.pool,&self.init,self.locking,&self.database,&self.schema,&self.marker,f)?;
        self.clean()?;
        if mark!=self.want { return Err(bad("the original SYSTEM claim changed during credential maintenance")); }
        Ok(value)
    }
    // Pre- and post-commit concerns are separate. An unknown commit result
    // stops at the original operation, while the RO anchor still pins SQLite.
    pub(super) fn write<T>(&self,until:Instant,confirm:impl Fn()->io::Result<()>,
        f:impl FnOnce(&Transaction<'_>)->io::Result<T>)->io::Result<T>{
        confirm()?;
        let gate=WriteGate { source:self, until };
        let mut c=engine::native_pair(&gate)?;
        let tx=c.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql)?;
        self.schema.require(&tx)?;
        let root=self.locking.source();
        if observed::first_marker(&self.pool,&self.init,&tx,root)?!=self.want {
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
    pub(super) fn sync(&self)->io::Result<()>{
        helper_linux_evidence::sqlite_vfs::sync_held_unix_main(&self.anchor)?;
        self.locking.source().dir().sync_all()
    }
}
impl<'s> WriteGate<'_, 's> {
    pub(in crate::sqlite_install) fn database(&self)->io::Result<&Existing<'s>> {
        expired(self.until)?;
        self.source.clean()?;
        use rustix::fs::{StatVfsMountFlags, fstatvfs};
        if fstatvfs(self.source.locking.source().dir())?.f_flag.contains(StatVfsMountFlags::RDONLY) {
            return Err(bad("cannot write the original SYSTEM mount"));
        }
        Ok(&self.source.database)
    }
}

fn uncertain()->io::Error{bad("original account outcome not confirmed; preserve its exact pending and original media")}
