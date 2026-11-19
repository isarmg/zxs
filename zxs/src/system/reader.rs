//! Initial-pool observer through the actual pinned SQLite engine. No SQL
//! statement, pathname from a DB, connection or FILE authority escapes.
//! The directory OFD remains held across idle time; no SQL transaction does.
use super::{BirthRow,CATALOG,Catalog,ClaimRow,InitState,ROOT_MARK,SCHEMA,lease::SystemDir,role_at};
use crate::{OperationId,PoolId,VolumeId,fs::{self,ComponentLimit,ControlRead,invalid}};
use std::{ffi::{CStr,CString,OsStr,c_char,c_int,c_void},io,os::unix::ffi::OsStrExt,path::{Path,PathBuf},ptr};

#[repr(C)] struct Db{_opaque:[u8;0]}
#[repr(C)] struct Stmt{_opaque:[u8;0]}
unsafe extern "C"{
    fn zxs_sql_path_limit()->c_int;
    fn zxs_sql_open_read(path:*const c_char,db:*mut *mut Db)->c_int;
    fn zxs_sql_named(db:*mut Db)->c_int;
    fn zxs_sql_schema(db:*mut Db,ddl:*const c_char)->c_int;
    fn sqlite3_prepare_v2(db:*mut Db,sql:*const c_char,n:c_int,s:*mut *mut Stmt,tail:*mut *const c_char)->c_int;
    fn sqlite3_bind_blob(s:*mut Stmt,k:c_int,x:*const c_void,n:c_int,drop_value:Option<unsafe extern "C" fn(*mut c_void)>)->c_int;
    fn sqlite3_step(s:*mut Stmt)->c_int;
    fn sqlite3_finalize(s:*mut Stmt)->c_int;
    fn sqlite3_close(db:*mut Db)->c_int;
    fn sqlite3_column_type(s:*mut Stmt,k:c_int)->c_int;
    fn sqlite3_column_blob(s:*mut Stmt,k:c_int)->*const c_void;
    fn sqlite3_column_bytes(s:*mut Stmt,k:c_int)->c_int;
    fn sqlite3_column_int64(s:*mut Stmt,k:c_int)->i64;
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
impl Connection{
    fn read(path:&CStr)->io::Result<Self>{
        let mut p=ptr::null_mut();check(unsafe{zxs_sql_open_read(path.as_ptr(),&mut p)})?;
        if p.is_null(){return Err(invalid("SQLite returned no live read-only connection"));}Ok(Self(p))
    }
    fn prepare(&self,sql:&CStr)->io::Result<Statement<'_>>{
        let mut p=ptr::null_mut();
        check(unsafe{sqlite3_prepare_v2(self.0,sql.as_ptr(),-1,&mut p,ptr::null_mut())})?;
        if p.is_null(){return Err(invalid("SQLite returned no statement"));}Ok(Statement(p,self))
    }
    fn fixed(&self,sql:&CStr)->io::Result<()>{
        let s=self.prepare(sql)?;let rc=unsafe{sqlite3_step(s.0)};
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
struct Statement<'a>(*mut Stmt,#[allow(dead_code)] &'a Connection);
impl Drop for Statement<'_>{fn drop(&mut self){let _=unsafe{sqlite3_finalize(self.0)};}}
impl Statement<'_>{
    fn integer(&self,k:i32)->io::Result<i64>{
        if unsafe{sqlite3_column_type(self.0,k)}!=1{return Err(invalid("unexpected SQLite integer storage class"));}
        Ok(unsafe{sqlite3_column_int64(self.0,k)})
    }
    fn blob(&self,k:i32,max:usize)->io::Result<&[u8]>{
        if unsafe{sqlite3_column_type(self.0,k)}!=4{return Err(invalid("unexpected SQLite blob storage class"));}
        let p=unsafe{sqlite3_column_blob(self.0,k)};
        let n=usize::try_from(unsafe{sqlite3_column_bytes(self.0,k)}).map_err(|_|invalid("bad SQLite blob length"))?;
        if p.is_null()||n==0||n>max{return Err(invalid("missing or oversized SQLite blob"));}
        // SAFETY: SQLite owns this row while this statement is on SQLITE_ROW.
        // No exposed step/column conversion can invalidate the borrowed cell.
        Ok(unsafe{std::slice::from_raw_parts(p.cast::<u8>(),n)})
    }
    fn array<const N:usize>(&self,k:i32)->io::Result<[u8;N]>{
        self.blob(k,N)?.try_into().map_err(|_|invalid("wrong SQLite identity/digest width"))
    }
    fn byte(&self,k:i32)->io::Result<u8>{
        u8::try_from(self.integer(k)?).map_err(|_|invalid("bad SQLite byte field"))
    }
    fn birth(&self)->io::Result<BirthRow>{
        Ok(BirthRow{pool:PoolId(self.array(0)?),op:OperationId(self.array(1)?),actor:self.array(2)?,
            request:self.array(3)?,state:match self.integer(4)?{0=>InitState::PendingIo,1=>InitState::Committed,
                _=>return Err(invalid("unknown INIT state"))},
            approval:self.array(5)?,uid:self.integer(6)?.try_into().map_err(|_|invalid("wrong original storage uid"))?,
            count:self.byte(7)?})
    }
    fn claim(&self)->io::Result<ClaimRow>{
        let ordinal=self.byte(8)?;let role=role_at(usize::from(ordinal));
        if self.byte(9)?!=super::role_tag(role){return Err(invalid("wrong role for original ordinal"));}
        Ok(ClaimRow{ordinal,role,volume:VolumeId(self.array(10)?),path:PathBuf::from(OsStr::from_bytes(self.blob(11,4096)?)),
           limit:ComponentLimit::from_original(self.byte(12)?)?,token:self.array(13)?})
    }
}

pub(crate) struct InitialReader{
    // Drop order: SQLite closes FIRST, before either physical proof or the
    // last lifetime directory SH can be released. No borrowed DB is public.
    conn:Connection,marker:ControlRead,dir:SystemDir,pool:PoolId,
}
impl InitialReader{
    pub(crate) fn open(system:&Path,expected:PoolId)->io::Result<Self>{
        let limit=usize::try_from(unsafe{zxs_sql_path_limit()}).map_err(|_|invalid("unknown SQLite Unix VFS"))?;
        let dir=SystemDir::hold(system,limit)?;
        // Root and marker identifiers were inspected with statx (no open)
        // and shown distinct, single-link and protected before borrowing this
        // non-DB control file. Only SQLite ever opens the DB or its journal.
        let marker=ControlRead::open(&dir.root,ROOT_MARK,101)?;dir.require()?;
        let conn=Connection::read(dir.path())?;
        let s=Self{conn,marker,dir,pool:expected};
        s.snapshot()?;Ok(s)
    }
    pub(crate) fn snapshot(&self)->io::Result<Catalog>{
        self.dir.require()?;check(unsafe{zxs_sql_named(self.conn.0)})?;
        if unsafe{sqlite3_get_autocommit(self.conn.0)}==0{return Err(invalid("previous SQLite transaction did not close"));}
        self.conn.fixed(c"BEGIN")?;
        let result=(||{
            let ddl=CString::new(SCHEMA).expect("compiled static SQL is NUL-free");
            check(unsafe{zxs_sql_schema(self.conn.0,ddl.as_ptr())})?;
            let select=CString::new(CATALOG).expect("compiled static SELECT is NUL-free");
            let s=self.conn.prepare(&select)?;
            // NULL destructor = SQLITE_STATIC; these 16 bytes belong to self
            // and remain live beyond statement finalization.
            check(unsafe{sqlite3_bind_blob(s.0,1,self.pool.0.as_ptr().cast(),16,None)})?;
            let mut birth=None;let mut rows=Vec::new();
            loop{match unsafe{sqlite3_step(s.0)}{
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
            self.dir.require()?;check(unsafe{zxs_sql_named(self.conn.0)})?;
            Ok(catalog)
        })();
        match result{
            Ok(c)=>{if let Err(e)=self.conn.fixed(c"COMMIT"){let _=self.conn.fixed(c"ROLLBACK");return Err(e);}Ok(c)}
            Err(e)=>{if unsafe{sqlite3_get_autocommit(self.conn.0)}==0{self.conn.fixed(c"ROLLBACK")?;}Err(e)}
        }
    }
}

#[cfg(test)]
#[path="../../tests/unit/sqlite_reader.rs"]
mod tests;
