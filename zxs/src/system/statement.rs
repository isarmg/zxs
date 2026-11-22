//! SQLite statement and live row ownership. Business code can observe cells
//! or advance the cursor, but cannot do both to the same borrowed row.
use super::{Connection,Db,check};
use crate::{PoolId,fs::invalid,system::CATALOG};
use std::{ffi::{CStr,CString,c_char,c_int,c_void},io,ptr};

#[repr(C)] struct Stmt{_opaque:[u8;0]}
unsafe extern "C"{
    fn sqlite3_prepare_v2(db:*mut Db,sql:*const c_char,n:c_int,s:*mut *mut Stmt,tail:*mut *const c_char)->c_int;
    fn sqlite3_bind_int64(s:*mut Stmt,k:c_int,v:i64)->c_int;
    fn sqlite3_bind_blob(s:*mut Stmt,k:c_int,x:*const c_void,n:c_int,drop_value:Option<unsafe extern "C" fn(*mut c_void)>)->c_int;
    fn zxs_sql_bind_copy(s:*mut Stmt,index:c_int,bytes:*const c_void,count:c_int)->c_int;
    fn sqlite3_step(s:*mut Stmt)->c_int;
    fn sqlite3_finalize(s:*mut Stmt)->c_int;
    fn sqlite3_data_count(s:*mut Stmt)->c_int;
    fn sqlite3_column_type(s:*mut Stmt,k:c_int)->c_int;
    fn sqlite3_column_blob(s:*mut Stmt,k:c_int)->*const c_void;
    fn sqlite3_column_text(s:*mut Stmt,k:c_int)->*const u8;
    fn sqlite3_column_bytes(s:*mut Stmt,k:c_int)->c_int;
    fn sqlite3_column_int64(s:*mut Stmt,k:c_int)->i64;
}
const MAX_SCHEMA_OBJECTS:u16=177;

impl Connection{
    pub(super) fn prepare(&self,sql:&CStr)->io::Result<Statement<'_>>{
        let mut p=ptr::null_mut();
        check(unsafe{sqlite3_prepare_v2(self.0,sql.as_ptr(),-1,&mut p,ptr::null_mut())})?;
        if p.is_null(){return Err(invalid("SQLite returned no statement"));}
        Ok(Statement{raw:p,_connection:self})
    }
    pub(super) fn initial_catalog<'a>(&'a self,pool:&'a PoolId)->io::Result<Statement<'a>>{
        let select=CString::new(CATALOG).expect("compiled static SELECT is NUL-free");
        let s=self.prepare(&select)?;
        // SQLITE_STATIC for this fixed 16-byte parameter, tied by the
        // returned statement's lifetime to both its connection and the pool.
        check(unsafe{sqlite3_bind_blob(s.raw,1,pool.0.as_ptr().cast(),16,None)})?;
        Ok(s)
    }
    /// Call only for a fixed internal shape SELECT, never for FILE data or
    /// user rows. No schema text, SQLite pointer or borrowed row escapes.
    pub(super) fn shape(&self)->io::Result<[u8;32]>{
        use crate::hash::Sha256;
        let s=self.prepare(c"SELECT type,name,tbl_name,sql FROM main.sqlite_schema ORDER BY type COLLATE BINARY,name COLLATE BINARY LIMIT ?1")?;
        s.bind_integer(1,i64::from(MAX_SCHEMA_OBJECTS)+1)?;
        let mut h=Sha256::new();h.update(b"ZXS-SQLITE-1/SCHEMA-ROWS\0")?;
        let mut count=0u16;let mut remaining=256*1024usize;
        loop{match s.step(){
            100=>{
                count+=1;if count>MAX_SCHEMA_OBJECTS{return Err(invalid("unexpected schema object count"));}
                for k in 0..4{
                    match unsafe{sqlite3_column_type(s.raw,k)}{
                        5=>h.update(&[0])?, // NULL automatic index SQL is not empty text.
                        3=>{
                            let p=unsafe{sqlite3_column_text(s.raw,k)};
                            let n=usize::try_from(unsafe{sqlite3_column_bytes(s.raw,k)}).map_err(|_|invalid("invalid schema cell size"))?;
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
}

pub(super) struct Statement<'a>{raw:*mut Stmt,_connection:&'a Connection}
impl Drop for Statement<'_>{fn drop(&mut self){let _=unsafe{sqlite3_finalize(self.raw)};}}
impl<'a> Statement<'a>{
    pub(super) fn bind_integer(&self,k:i32,n:i64)->io::Result<()>{check(unsafe{sqlite3_bind_int64(self.raw,k,n)})}
    pub(super) fn bind_bytes(&self,k:i32,b:&[u8])->io::Result<()>{
        let n=i32::try_from(b.len()).map_err(|_|invalid("oversize SQLite parameter"))?;
        // SQLITE_TRANSIENT: SQLite takes its own bytes before this function returns.
        check(unsafe{zxs_sql_bind_copy(self.raw,k,b.as_ptr().cast(),n)})
    }
    pub(super) fn step(&self)->i32{unsafe{sqlite3_step(self.raw)}}
    pub(super) fn done(&self)->io::Result<()>{
        let rc=self.step();
        if rc==101{return Ok(());}check(rc)?;Err(invalid("statement did not finish exactly once"))
    }
    pub(super) fn is_null(&self,k:i32)->bool{(unsafe{sqlite3_column_type(self.raw,k)})==5}
    pub(super) fn integer(&self,k:i32)->io::Result<i64>{
        if unsafe{sqlite3_column_type(self.raw,k)}!=1{return Err(invalid("unexpected SQLite integer storage class"));}
        Ok(unsafe{sqlite3_column_int64(self.raw,k)})
    }
    fn blob_bytes(&self,k:i32,max:usize)->io::Result<&[u8]>{
        if unsafe{sqlite3_column_type(self.raw,k)}!=4{return Err(invalid("unexpected SQLite blob storage class"));}
        let p=unsafe{sqlite3_column_blob(self.raw,k)};
        let n=usize::try_from(unsafe{sqlite3_column_bytes(self.raw,k)}).map_err(|_|invalid("bad SQLite blob length"))?;
        if p.is_null()||n==0||n>max{return Err(invalid("missing or oversized SQLite blob"));}
        // The storage class forbids coercion. Callers below either consume
        // these bytes immediately or use an exclusive live Row borrow.
        Ok(unsafe{std::slice::from_raw_parts(p.cast::<u8>(),n)})
    }
    pub(super) fn blob_eq(&self,k:i32,max:usize,expected:&[u8])->io::Result<bool>{
        Ok(self.blob_bytes(k,max)?==expected)
    }
    pub(super) fn array<const N:usize>(&self,k:i32)->io::Result<[u8;N]>{
        self.blob_bytes(k,N)?.try_into().map_err(|_|invalid("wrong SQLite identity/digest width"))
    }
    pub(super) fn byte(&self,k:i32)->io::Result<u8>{
        u8::try_from(self.integer(k)?).map_err(|_|invalid("bad SQLite byte field"))
    }

    /// Prove that SQLite has an actual current row, using its existing
    /// cursor state. Fixed-width copies and comparisons need no row loan.
    pub(super) fn row(&mut self)->io::Result<Row<'_, 'a>>{
        if unsafe{sqlite3_data_count(self.raw)}<=0{return Err(invalid("no live SQLite result row"));}
        Ok(Row{statement:self})
    }
}

/// No stepping, binding, coercion or raw handle while any variable cell is
/// borrowed; typed access to the other current cells is still permitted.
pub(super) struct Row<'s,'a>{statement:&'s mut Statement<'a>}
impl Row<'_, '_>{
    pub(super) fn blob(&self,k:i32,max:usize)->io::Result<&[u8]>{self.statement.blob_bytes(k,max)}
    pub(super) fn array<const N:usize>(&self,k:i32)->io::Result<[u8;N]>{self.statement.array(k)}
    pub(super) fn byte(&self,k:i32)->io::Result<u8>{self.statement.byte(k)}
    pub(super) fn is_null(&self,k:i32)->bool{self.statement.is_null(k)}
}

#[cfg(test)]
mod tests{
    use super::*;

    fn memory()->Connection{
        let mut p=ptr::null_mut();
        check(unsafe{super::super::zxs_sql_template(c"CREATE TABLE fixture(data BLOB);".as_ptr(),&mut p)}).unwrap();
        assert!(!p.is_null());
        Connection(p)
    }

    #[test]
    fn sqlite_variable_row_keeps_exact_types_bytes_and_bounds(){
        let db=memory();
        let mut q=db.prepare(c"SELECT X'6162',X'000063',NULL,'ab',X'',17").unwrap();
        assert_eq!(q.row().err().unwrap().to_string(),"no live SQLite result row");
        assert_eq!(q.step(),100);
        let copied={
            let row=q.row().unwrap();
            let first=row.blob(0,2).unwrap();
            let second=row.blob(1,3).unwrap();
            assert!(row.is_null(2));
            assert!(!row.is_null(4));
            assert_eq!(row.byte(5).unwrap(),17);
            assert_eq!(first,b"ab");
            assert_eq!(second,b"\0\0c");
            for column in [2,3,5]{
                assert_eq!(row.blob(column,20).unwrap_err().to_string(),"unexpected SQLite blob storage class");
            }
            for (column,max) in [(4,20),(1,2)]{
                assert_eq!(row.blob(column,max).unwrap_err().to_string(),"missing or oversized SQLite blob");
            }
            row.array::<2>(0).unwrap()
        };
        assert!(q.blob_eq(0,2,b"ab").unwrap());
        assert!(!q.blob_eq(0,2,b"ba").unwrap());
        q.done().unwrap();
        assert_eq!(q.row().err().unwrap().kind(),io::ErrorKind::InvalidInput);
        assert_eq!(copied,*b"ab");
        let mut empty=db.prepare(c"SELECT X'6162' WHERE 0").unwrap();
        assert_eq!(empty.step(),101);
        assert!(empty.row().is_err());
    }

    #[test]
    fn sqlite_row_reborrow_tracks_each_live_result_without_copies(){
        let db=memory();
        let mut q=db.prepare(c"WITH RECURSIVE rows(n) AS (VALUES (1) UNION ALL SELECT n+1 FROM rows WHERE n<4096)
            SELECT CAST(printf('%04x',n) AS BLOB),n FROM rows").unwrap();
        for n in 1..=4096{
            assert_eq!(q.step(),100);
            assert_eq!(q.integer(1).unwrap(),n);
            let row=q.row().unwrap();
            assert_eq!(row.blob(0,4).unwrap(),format!("{n:04x}").as_bytes());
        }
        assert_eq!(q.step(),101);
        assert!(q.row().is_err());
    }
}
