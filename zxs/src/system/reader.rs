//! Initial-pool observer through the actual pinned SQLite engine. No SQL
//! statement, pathname from a DB, connection or FILE authority escapes.
//! The directory OFD remains held across idle time; no SQL transaction does.
use super::{BirthRow,CATALOG,Catalog,ClaimRow,InitState,SCHEMA,lease::SystemDir,role_at};
use crate::{OperationId,PoolId,VolumeId,fs::{self,ComponentLimit,ControlRead,invalid}};
use std::{ffi::{CStr,CString,OsStr,c_char,c_int,c_void},io,os::unix::ffi::OsStrExt,path::{Path,PathBuf},ptr};

#[repr(C)] struct Db{_opaque:[u8;0]}
#[repr(C)] struct Stmt{_opaque:[u8;0]}
unsafe extern "C"{
    fn zxs_sql_path_limit()->c_int;
    fn zxs_sql_open_read(path:*const c_char,db:*mut *mut Db)->c_int;
    fn zxs_sql_open_writer(path:*const c_char,db:*mut *mut Db)->c_int;
    fn zxs_sql_bind_copy(s:*mut Stmt,index:c_int,bytes:*const c_void,count:c_int)->c_int;
    fn zxs_sql_named(db:*mut Db)->c_int;
    fn zxs_sql_template(ddl:*const c_char,db:*mut *mut Db)->c_int;
    fn zxs_sql_family(db:*mut Db)->c_int;
    fn sqlite3_prepare_v2(db:*mut Db,sql:*const c_char,n:c_int,s:*mut *mut Stmt,tail:*mut *const c_char)->c_int;
    fn sqlite3_bind_int64(s:*mut Stmt,k:c_int,v:i64)->c_int;
    fn sqlite3_bind_blob(s:*mut Stmt,k:c_int,x:*const c_void,n:c_int,drop_value:Option<unsafe extern "C" fn(*mut c_void)>)->c_int;
    fn sqlite3_step(s:*mut Stmt)->c_int;
    fn sqlite3_finalize(s:*mut Stmt)->c_int;
    fn sqlite3_close(db:*mut Db)->c_int;
    fn sqlite3_column_type(s:*mut Stmt,k:c_int)->c_int;
    fn sqlite3_column_blob(s:*mut Stmt,k:c_int)->*const c_void;
    fn sqlite3_column_text(s:*mut Stmt,k:c_int)->*const u8;
    fn sqlite3_column_bytes(s:*mut Stmt,k:c_int)->c_int;
    fn sqlite3_column_int64(s:*mut Stmt,k:c_int)->i64;
    fn sqlite3_get_autocommit(db:*mut Db)->c_int;
}
const MAX_SCHEMA_OBJECTS:u16=132;
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
    fn open(dir:&SystemDir,writer:bool)->io::Result<Self>{
        let mut p=ptr::null_mut();
        check(unsafe{if writer{zxs_sql_open_writer(dir.path().as_ptr(),&mut p)}
                     else{zxs_sql_open_read(dir.path().as_ptr(),&mut p)}})?;
        if p.is_null(){return Err(invalid("SQLite returned no live connection"));}Ok(Self(p))
    }
    fn prepare(&self,sql:&CStr)->io::Result<Statement<'_>>{
        let mut p=ptr::null_mut();
        check(unsafe{sqlite3_prepare_v2(self.0,sql.as_ptr(),-1,&mut p,ptr::null_mut())})?;
        if p.is_null(){return Err(invalid("SQLite returned no statement"));}Ok(Statement(p,self))
    }
    /// Call only for a fixed internal shape SELECT, never for FILE data or
    /// user rows. No schema text, SQLite pointer or borrowed row escapes.
    fn shape(&self)->io::Result<[u8;32]>{
        use crate::hash::Sha256;
        let s=self.prepare(c"SELECT type,name,tbl_name,sql FROM main.sqlite_schema ORDER BY type COLLATE BINARY,name COLLATE BINARY LIMIT ?1")?;
        s.bind_integer(1,i64::from(MAX_SCHEMA_OBJECTS)+1)?;
        let mut h=Sha256::new();h.update(b"ZXS-SQLITE-1/SCHEMA-ROWS\0")?;
        let mut count=0u16;let mut remaining=256*1024usize;
        loop{match unsafe{sqlite3_step(s.0)}{
            100=>{
                count+=1;if count>MAX_SCHEMA_OBJECTS{return Err(invalid("unexpected schema object count"));}
                for k in 0..4{
                    match unsafe{sqlite3_column_type(s.0,k)}{
                        5=>h.update(&[0])?, // NULL automatic index SQL is not empty text.
                        3=>{
                            let p=unsafe{sqlite3_column_text(s.0,k)};
                            let n=usize::try_from(unsafe{sqlite3_column_bytes(s.0,k)}).map_err(|_|invalid("invalid schema cell size"))?;
                            if p.is_null()||n>65536||n>remaining{return Err(invalid("schema metadata exceeds its qualified bound"));}
                            remaining-=n;h.update(&[1])?;h.update(&(n as u32).to_be_bytes())?;
                            // Same actual row bytes while the original statement is on SQLITE_ROW.
                            h.update(unsafe{std::slice::from_raw_parts(p,n)})?;
                        }
                        _=>return Err(invalid("unexpected schema storage class")),
                    }
                }
            }
            101=>break,rc=>{check(rc)?;return Err(invalid("unexpected SQLite schema step"));}
        }}
        h.update(&count.to_be_bytes())?;Ok(h.finish())
    }
    fn expected_schema()->io::Result<[u8;32]>{
        let ddl=CString::new(SCHEMA).expect("compiled static SQL is NUL-free");
        let mut p=ptr::null_mut();check(unsafe{zxs_sql_template(ddl.as_ptr(),&mut p)})?;
        if p.is_null(){return Err(invalid("SQLite returned no fixed in-memory template"));}
        Connection(p).shape() // reference connection closes here, before any real read transaction
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
    fn bind_integer(&self,k:i32,n:i64)->io::Result<()>{check(unsafe{sqlite3_bind_int64(self.0,k,n)})}
    fn bind_bytes(&self,k:i32,b:&[u8])->io::Result<()>{
        let n=i32::try_from(b.len()).map_err(|_|invalid("oversize SQLite parameter"))?;
        // SQLITE_TRANSIENT: SQLite takes its own bytes before this function returns.
        check(unsafe{zxs_sql_bind_copy(self.0,k,b.as_ptr().cast(),n)})
    }
    fn done(&self)->io::Result<()>{
        let rc=unsafe{sqlite3_step(self.0)};
        if rc==101{return Ok(());}check(rc)?;Err(invalid("statement did not finish exactly once"))
    }
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
    conn:Connection,marker:ControlRead,dir:SystemDir,pool:PoolId,expected_schema:[u8;32],
}
impl InitialReader{
    pub(crate) fn open(system:&Path,expected:PoolId)->io::Result<Self>{
        let limit=usize::try_from(unsafe{zxs_sql_path_limit()}).map_err(|_|invalid("unknown SQLite Unix VFS"))?;
        let dir=SystemDir::hold(system,limit)?;
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
        self.dir.require()?;check(unsafe{zxs_sql_named(conn.0)})?;
        if unsafe{sqlite3_get_autocommit(conn.0)}==0{return Err(invalid("previous SQLite transaction did not close"));}
        conn.fixed(begin)?;
        let result=(||{
            check(unsafe{zxs_sql_family(conn.0)})?;
            if conn.shape()?!=self.expected_schema{return Err(invalid("SQLite schema differs from the exact qualified DDL"));}
            let select=CString::new(CATALOG).expect("compiled static SELECT is NUL-free");
            let s=conn.prepare(&select)?;
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
            let value=read(conn,catalog)?;
            self.dir.require()?;
            self.marker.require(&self.dir.root)?; // same held S control bytes/stamp at the final boundary
            check(unsafe{zxs_sql_named(conn.0)})?;
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
