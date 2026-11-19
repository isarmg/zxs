//! Prerequisites of the one upstream engine linked into this process. No SQL,
//! file, path, database handle or arbitrary SQLite/VFS operation is exposed.
use std::{ffi::CStr,io};
use libsqlite3_sys as native;

const SOURCE:&CStr=c"2026-10-16 14:45:20 555e07ef33f69a0957d24d8e555b702464a131d6f1607f4c3c1e4551e8e6ca76";

/// Must succeed before *any* storage path is opened by SQLite (including read-only checks).
#[allow(unsafe_code)]
pub fn unix_sqlite_path_limit()->io::Result<usize>{
    let no=||io::Error::new(io::ErrorKind::Unsupported,"unsupported SQLite source, build options or Unix VFS");
    // SAFETY: fixed static C strings, no caller pointer. SQLite returns a library-static
    // source string and a registered VFS. This closed application registers/removes
    // no VFS or extensions; the descriptor stays valid for this scalar read.
    unsafe {
        let src=native::sqlite3_sourceid();
        if src.is_null() || CStr::from_ptr(src)!=SOURCE || native::sqlite3_libversion_number()!=3_054_001
            || native::sqlite3_compileoption_used(c"OMIT_WAL".as_ptr())!=1
            || native::sqlite3_compileoption_used(c"OMIT_LOAD_EXTENSION".as_ptr())!=1 {
            return Err(no());
        }
        let unix=native::sqlite3_vfs_find(c"unix".as_ptr());
        if unix.is_null() || (*unix).zName.is_null() || CStr::from_ptr((*unix).zName)!=c"unix" {
            return Err(no());
        }
        let n=usize::try_from((*unix).mxPathname).map_err(|_|no())?;
        if n==0 {return Err(no());}
        Ok(n)
    }
}
