//! One installed Unix SQLite VFS. Opening never creates a database; only the
//! future exclusive new-file reservation can supply the writable path.
use super::{bad,unsupported};
use rusqlite::{config::DbConfig,Connection,FileControl,OpenFlags,MAIN_DB};
use std::{io,time::Duration};

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
    db.busy_timeout(Duration::ZERO).map_err(fail)?;
    for (config,expected) in [
        (DbConfig::SQLITE_DBCONFIG_DEFENSIVE,true),(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA,false),
        (DbConfig::SQLITE_DBCONFIG_ENABLE_FKEY,true),(DbConfig::SQLITE_DBCONFIG_ENABLE_ATTACH_CREATE,false),
        (DbConfig::SQLITE_DBCONFIG_ENABLE_ATTACH_WRITE,false),
    ]{if db.set_db_config(config,expected).map_err(fail)?!=expected{return Err(bad("SQLite rejected a required connection option"));}}
    // Only a proven, freshly reserved inode may set journal_mode; existing or
    // unknown databases are never converted. Connection-scoped EXTRA is read back.
    let journal:String=db.query_row("PRAGMA main.journal_mode=DELETE",[],|r|r.get(0)).map_err(fail)?;
    if journal!="delete"{return Err(bad("new database rejected SQLite DELETE journaling"));}
    db.execute_batch("PRAGMA main.synchronous=EXTRA;").map_err(fail)?;
    verified(&db,false)?;Ok(db)
}
