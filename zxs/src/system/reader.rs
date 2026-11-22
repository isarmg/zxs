//! Initial-pool observer through the actual pinned SQLite engine. No SQL
//! statement, pathname from a DB, connection or FILE authority escapes.
//! The directory OFD remains held across idle time; no SQL transaction does.
use super::{BirthRow,Catalog,ClaimRow,InitState,SCHEMA,lease::SystemDir,role_at};
use crate::{OperationId,PoolId,VolumeId,fs::{self,ComponentLimit,ControlRead,invalid}};
use std::{ffi::{CStr,CString,OsStr,c_char,c_int},io,os::unix::ffi::OsStrExt,path::{Path,PathBuf},ptr,sync::Mutex};

#[cfg(test)]
use std::ffi::c_void;

#[path="statement.rs"]
mod statement;
use statement::Statement;

#[repr(C)] struct Db{_opaque:[u8;0]}
unsafe extern "C"{
    fn zxs_sql_path_limit()->c_int;
    fn zxs_sql_open_read(path:*const c_char,db:*mut *mut Db)->c_int;
    fn zxs_sql_open_writer(path:*const c_char,db:*mut *mut Db)->c_int;
    fn zxs_sql_named(db:*mut Db)->c_int;
    fn zxs_sql_template(ddl:*const c_char,db:*mut *mut Db)->c_int;
    fn zxs_sql_family(db:*mut Db)->c_int;
    fn sqlite3_close(db:*mut Db)->c_int;
    fn sqlite3_get_autocommit(db:*mut Db)->c_int;
}
fn check(rc:c_int)->io::Result<()>{
    if rc==0{return Ok(());}
    let(kind,why)=match rc{
        776=>(io::ErrorKind::PermissionDenied,"requires authorized SQLite hot-journal recovery"),
        _ if rc&255==5 || rc&255==6=>(io::ErrorKind::WouldBlock,"busy or locked; not a successful outcome"),
        _=>(io::ErrorKind::InvalidData,"cannot positively verify this database"),
    };
    Err(io::Error::new(kind,format!("SQLite {rc}: {why}")))
}
struct Connection(*mut Db);
fn cached_schema_template(cache:&Mutex<Option<[u8;32]>>,compile:impl FnOnce()->io::Result<[u8;32]>)->io::Result<[u8;32]>{
    let mut stored=cache.lock().map_err(|_|io::Error::other("canonical SYSTEM schema template lock poisoned"))?;
    if let Some(digest)=*stored{return Ok(digest);}
    let digest=compile()?;
    *stored=Some(digest);
    Ok(digest)
}
impl Connection{
    fn open(dir:&SystemDir,writer:bool)->io::Result<Self>{
        let mut p=ptr::null_mut();
        check(unsafe{if writer{zxs_sql_open_writer(dir.path().as_ptr(),&mut p)}
                     else{zxs_sql_open_read(dir.path().as_ptr(),&mut p)}})?;
        if p.is_null(){return Err(invalid("SQLite returned no live connection"));}Ok(Self(p))
    }
    fn expected_schema()->io::Result<[u8;32]>{
        // Only the fixed compiled-in DDL and vendored SQLite feed this cache.
        // Actual DB schema, identity and catalog are checked on every cut.
        static TEMPLATE:Mutex<Option<[u8;32]>>=Mutex::new(None);
        cached_schema_template(&TEMPLATE,||{
            let ddl=CString::new(SCHEMA).expect("compiled static SQL is NUL-free");
            let mut p=ptr::null_mut();check(unsafe{zxs_sql_template(ddl.as_ptr(),&mut p)})?;
            if p.is_null(){return Err(invalid("SQLite returned no fixed in-memory template"));}
            Connection(p).shape() // close the memory-only reference before any real read transaction
        })
    }
    fn fixed(&self,sql:&CStr)->io::Result<()>{
        let s=self.prepare(sql)?;let rc=s.step();
        if rc!=101{check(rc)?;return Err(invalid("unexpected result for transaction statement"));}
        Ok(())
    }
}
impl Drop for Connection{
    fn drop(&mut self){
        // Statements have strictly shorter Rust borrows. No backup handle,
        // blob handle or callback can outlive this connection; no close_v2 zombie.
        let _=unsafe{sqlite3_close(self.0)};
    }
}
impl Statement<'_>{
    fn birth(&self)->io::Result<BirthRow>{
        Ok(BirthRow{pool:PoolId(self.array(0)?),op:OperationId(self.array(1)?),actor:self.array(2)?,
            request:self.array(3)?,state:match self.integer(4)?{0=>InitState::PendingIo,1=>InitState::Committed,
                _=>return Err(invalid("unknown INIT state"))},
            approval:self.array(5)?,uid:self.integer(6)?.try_into().map_err(|_|invalid("wrong original storage uid"))?,
            count:self.byte(7)?})
    }
    fn claim(&mut self)->io::Result<ClaimRow>{
        let ordinal=self.byte(8)?;let role=role_at(usize::from(ordinal));
        if self.byte(9)?!=super::role_tag(role){return Err(invalid("wrong role for original ordinal"));}
        Ok(ClaimRow{ordinal,role,volume:VolumeId(self.array(10)?),path:PathBuf::from(OsStr::from_bytes(self.row()?.blob(11,4096)?)),
           limit:ComponentLimit::from_original(self.byte(12)?)?,token:self.array(13)?})
    }
}

pub(crate) struct InitialReader{
    // Drop order: SQLite closes FIRST, before either physical proof or the
    // last lifetime directory SH can be released. No borrowed DB is public.
    conn:Connection,marker:ControlRead,dir:SystemDir,pool:PoolId,expected_schema:[u8;32],
}
impl InitialReader{
    pub(crate) fn open(system:&Path,expected:PoolId)->io::Result<Self>{
        let limit=usize::try_from(unsafe{zxs_sql_path_limit()}).map_err(|_|invalid("unknown SQLite Unix VFS"))?;
        let dir=SystemDir::hold(system,limit)?;
        Self::open_held(dir,expected)
    }
    fn open_held(dir:SystemDir,expected:PoolId)->io::Result<Self>{
        // Root and marker identifiers were inspected with statx (no open)
        // and shown distinct, single-link and protected before borrowing this
        // non-DB control file. Only SQLite ever opens the DB or its journal.
        let marker=dir.hold_marker(&dir.root)?;dir.require()?;
        let conn=Connection::open(&dir,false)?;
        let expected_schema=Connection::expected_schema()?;
        let s=Self{conn,marker,dir,pool:expected,expected_schema};
        s.snapshot()?;Ok(s)
    }
    /// An independent read-only SQLite connection on another storage thread.
    /// Validate both still-held original names before invoking the real VFS;
    /// no SQLite connection or database FD is transferred across threads.
    fn alongside(current:&SystemDir,old_mark:&ControlRead,pool:PoolId,expected_schema:[u8;32])->io::Result<Self>{
        let limit=usize::try_from(unsafe{zxs_sql_path_limit()}).map_err(|_|invalid("unknown SQLite Unix VFS"))?;
        let dir=SystemDir::hold(&current.root.path,limit)?;
        current.matches_held(&dir)?;old_mark.require(&dir.root)?;
        let marker=dir.hold_marker(&dir.root)?;
        let conn=Connection::open(&dir,false)?;
        let s=Self{conn,marker,dir,pool,expected_schema};
        current.matches_held(&s.dir)?;old_mark.require(&s.dir.root)?;Ok(s) // first business cut checks actual schema/catalog
    }
    pub(crate) fn snapshot(&self)->io::Result<Catalog>{self.at(|_,catalog|Ok(catalog))}
    /// Only bounded metadata may be selected here. KDF, disk transfer and
    /// slow streams are outside the SQL transaction. A later action rechecks.
    fn at<T>(&self,read:impl FnOnce(&Connection,Catalog)->io::Result<T>)->io::Result<T>{
        self.transaction(&self.conn,c"BEGIN",read)
    }
    fn transaction<T>(&self,conn:&Connection,begin:&CStr,read:impl FnOnce(&Connection,Catalog)->io::Result<T>)->io::Result<T>{
        self.transaction_before_commit(conn,begin,read,||Ok(()))
    }
    /// Check call admission after the final potentially blocking S-name
    /// proof and immediately before COMMIT. This hook cannot acquire F/R.
    fn transaction_before_commit<T>(&self,conn:&Connection,begin:&CStr,read:impl FnOnce(&Connection,Catalog)->io::Result<T>,
        last:impl FnOnce()->io::Result<()>)->io::Result<T>{
        self.dir.require()?;check(unsafe{zxs_sql_named(conn.0)})?;
        if unsafe{sqlite3_get_autocommit(conn.0)}==0{return Err(invalid("previous SQLite transaction did not close"));}
        conn.fixed(begin)?;
        let result=(||{
            check(unsafe{zxs_sql_family(conn.0)})?;
            if conn.shape()?!=self.expected_schema{return Err(invalid("SQLite schema differs from the exact qualified DDL"));}
            let mut s=conn.initial_catalog(&self.pool)?;
            let mut birth=None;let mut rows=Vec::new();
            loop{match s.step(){
                100=>{
                    if rows.len()>=64{return Err(invalid("too many SQLite volume rows"));}
                    if birth.is_none(){birth=Some(s.birth()?);}
                    rows.push(s.claim()?);
                },101=>break,rc=>{check(rc)?;return Err(invalid("unexpected SQLite step result"));}
            }}
            drop(s);
            let b=birth.ok_or_else(||invalid("no initialized row for this original pool"))?;
            let catalog=Catalog::checked(self.pool,b,rows)?;
            if catalog.state()!=InitState::Committed{return Err(invalid("original INIT still pending I/O"));}
            if b.uid!=fs::local_uid() || catalog.rows()[0].path.as_os_str()!=self.dir.root.path.as_os_str() ||
                catalog.rows()[0].limit!=self.dir.component_limit()? ||
                !catalog.marker_matches(0,&self.marker.bytes(&self.dir.root)?) {
                return Err(invalid("actual SYSTEM does not match its committed SQLite enrollment"));
            }
            let value=read(conn,catalog)?;
            self.dir.require()?;
            self.marker.require(&self.dir.root)?; // same held S control bytes/stamp at the final boundary
            check(unsafe{zxs_sql_named(conn.0)})?;
            last()?;
            Ok(value)
        })();
        match result{
            Ok(c)=>{if let Err(e)=conn.fixed(c"COMMIT"){let _=conn.fixed(c"ROLLBACK");return Err(e);}Ok(c)}
            Err(e)=>{if unsafe{sqlite3_get_autocommit(conn.0)}==0{conn.fixed(c"ROLLBACK")?;}Err(e)}
        }
    }
}

#[cfg(test)]
#[path="../../tests/unit/sqlite_reader.rs"]
mod tests;

#[path="volume.rs"]
pub(crate) mod volume;

#[path="credential.rs"]
mod credential;

#[path="account.rs"]
pub(crate) mod account;

#[path="smb.rs"]
pub(crate) mod smb;

#[path="work_record.rs"]
mod work_record;

#[path="transfer_material.rs"]
mod transfer_material;

#[path="file_location.rs"]
mod file_location;

#[cfg(test)]
pub(crate) use tests::{web_pool,ordinary_pool,stage_managed_file};
