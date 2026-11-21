//! One installed Unix SQLite VFS. Opening never creates a database; only the
//! future exclusive new-file reservation can supply the writable path.
use super::{bad,unsupported,reservation::Existing};
use rusqlite::{config::DbConfig,Connection,FileControl,OpenFlags,MAIN_DB};
use std::{io,time::Duration};

// SQLite bounds internal table/index/trigger hash entries, including the
// schema table and WITHOUT ROWID primary indexes that have no sqlite_schema
// row. This parser/engine budget is not the canonical catalogue row limit.
// This paired definition uses 177 visible + 23 hidden PKs + the schema table.
const MAX_ENGINE_SCHEMA_OBJECTS:i32=208;

pub(super) fn path_limit()->io::Result<usize>{helper_linux_evidence::sqlite_vfs::unix_sqlite_path_limit()}

pub(super) fn verified(conn:&Connection,readonly:bool)->io::Result<()>{
    let sql=|_|bad("cannot establish fixed SQLite connection policy");
    if conn.is_readonly(MAIN_DB).map_err(sql)?!=readonly{return Err(bad("SQLite did not grant the exact requested access"));}
    original_name(conn)?;
    let journal:String=conn.query_row("PRAGMA main.journal_mode",[],|r|r.get(0)).map_err(sql)?;
    let fk:i64=conn.query_row("PRAGMA foreign_keys",[],|r|r.get(0)).map_err(sql)?;
    let trust:i64=conn.query_row("PRAGMA trusted_schema",[],|r|r.get(0)).map_err(sql)?;
    let sync:i64=conn.query_row("PRAGMA main.synchronous",[],|r|r.get(0)).map_err(sql)?;
    if journal!="delete" || fk!=1 || trust!=0 || sync!=3{return Err(unsupported("requires native SQLite DELETE/EXTRA, foreign keys and untrusted schema"));}
    Ok(())
}

pub(super) fn original_name(conn:&Connection)->io::Result<()>{
    let mut moved=-1;
    conn.file_control(Some(MAIN_DB),FileControl::HasMoved(&mut moved)).map_err(|_|bad("the Unix VFS could not attest the open database name"))?;
    if moved!=0{return Err(bad("the named database is not the original database opened by SQLite"));}Ok(())
}

/// Caller retains the original SYSTEM directory, full lifetime lock and pathname
/// invariants; this private function is not a way to authorize a named database.
pub(super) fn open_reserved(reserved:&super::reservation::Reserved<'_>)->io::Result<Connection>{
    // Engine/capability checks precede OS reservation in the installer as well.
    // No URI, CREATE, custom VFS, extension, busy spin or connection fallback.
    path_limit()?;
    let fail=|_|bad("the original reserved database could not be opened with the fixed SQLite policy");
    let flags=OpenFlags::SQLITE_OPEN_READ_WRITE|OpenFlags::SQLITE_OPEN_NO_MUTEX|OpenFlags::SQLITE_OPEN_NOFOLLOW;
    reserved.pristine()?;
    let db=Connection::open_with_flags_and_vfs(reserved.path(),flags,c"unix").map_err(fail)?;
    reserved.pristine()?;original_name(&db)?;
    if db.is_readonly(MAIN_DB).map_err(fail)?{return Err(bad("SQLite silently opened the reserved file read-only"));}
    configure(&db,false)?;
    // Only a proven fresh reservation can even request a journal mode change.
    let journal:String=db.query_row("PRAGMA main.journal_mode=DELETE",[],|r|r.get(0)).map_err(fail)?;
    if journal!="delete"{return Err(bad("new database rejected SQLite DELETE journaling"));}
    verified(&db,false)?;Ok(db)
}
pub(super) enum Access { Read, Write }
pub(super) fn existing(origin:&Existing<'_>,mode:Access)->io::Result<Connection>{
    let ro=matches!(mode,Access::Read);
    if !ro&&!origin.allows_install_write(){return Err(bad("a readonly observation cannot open the database for writing"));}
    open_existing(origin,ro)
}
pub(super) fn native_pair(permit:&super::pair::NativeWriteGate<'_, '_>)->io::Result<Connection>{
    open_existing(permit.database()?,false)
}
fn open_existing(origin:&Existing<'_>,ro:bool)->io::Result<Connection>{
    path_limit()?;origin.sidecars_absent()?;
    let flags=OpenFlags::SQLITE_OPEN_NO_MUTEX|OpenFlags::SQLITE_OPEN_NOFOLLOW|
        if ro { OpenFlags::SQLITE_OPEN_READ_ONLY } else { OpenFlags::SQLITE_OPEN_READ_WRITE };
    let db=Connection::open_with_flags_and_vfs(origin.path(),flags,c"unix")
        .map_err(|_|bad("cannot open the existing original database in the required mode"))?;
    origin.sidecars_absent()?;original_name(&db)?;
    if db.is_readonly(MAIN_DB).map_err(|_|bad("unknown SQLite access mode"))?!=ro{
        return Err(bad("existing connection lacks the exact requested mode"));
    }
    configure(&db,ro)?;verified(&db,ro)?;origin.sidecars_absent()?;Ok(db)
}
fn configure(db:&Connection,ro:bool)->io::Result<()>{
    let fail=|_|bad("SQLite rejected a required local connection setting");
    limits(db)?;
    db.busy_timeout(Duration::ZERO).map_err(fail)?;
    for (config,expected) in [
        (DbConfig::SQLITE_DBCONFIG_DEFENSIVE,true),(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA,false),
        (DbConfig::SQLITE_DBCONFIG_ENABLE_FKEY,true),(DbConfig::SQLITE_DBCONFIG_ENABLE_ATTACH_CREATE,false),
        (DbConfig::SQLITE_DBCONFIG_ENABLE_ATTACH_WRITE,false),
    ]{if db.set_db_config(config,expected).map_err(fail)?!=expected{return Err(bad("SQLite rejected a required connection option"));}}
    // All are connection-local. Existing media is not converted or configured.
    db.execute_batch("PRAGMA temp_store=MEMORY; PRAGMA main.synchronous=EXTRA;").map_err(fail)?;
    if ro{
        db.execute_batch("PRAGMA query_only=ON;").map_err(fail)?;
        let q:i64=db.query_row("PRAGMA query_only",[],|r|r.get(0)).map_err(fail)?;
        if q!=1{return Err(bad("read-only observation may not change the database"));}
    }
    Ok(())
}

pub(super) fn limits(db:&Connection)->io::Result<()>{
    let fail=|_|bad("cannot establish the SQLite schema/data limits");
    use rusqlite::limits::Limit;
    for (key,n) in [(Limit::SQLITE_LIMIT_LENGTH,1024*1024),(Limit::SQLITE_LIMIT_SCHEMA,MAX_ENGINE_SCHEMA_OBJECTS)]{
        db.set_limit(key,n).map_err(fail)?;
        let actual=db.limit(key).map_err(fail)?;
        if actual<=0||actual>n{return Err(bad("could not bound schema/data values"));}
    }
    Ok(())
}
