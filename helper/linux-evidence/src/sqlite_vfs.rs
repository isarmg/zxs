//! One linked SQLite engine and one scoped maintenance sync of its already
//! held Unix main-file object. No raw descriptor or exported handle access.
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

/// Strongly synchronize the exact, already-open main inode during an
/// authorized cold account-order confirmation. Caller has independently
/// established this private connection's original Unix VFS, its no-journal
/// cut and true exclusive directory lease. No new FD, lock or raw read.
#[allow(unsafe_code)]
pub fn sync_held_unix_main(connection:&rusqlite::Connection)->io::Result<()>{
    unix_sqlite_path_limit()?;
    let failed=||io::Error::other("the actual SQLite main-file synchronization could not be confirmed");
    if !connection.is_autocommit(){return Err(failed());}
    // SAFETY: FILE_POINTER returns a SQLite-owned sqlite3_file for this live
    // connection (not a VFS-private Rust-cast FD). This single-threaded
    // caller owns the connection and leaves no active statements/transaction.
    // This uses only the official synchronous xSync method; never FILECONTROL_SYNC
    // (an internal notification, not a flush), xClose or open/close raw fd.
    unsafe{
        let mut file:*mut native::sqlite3_file=std::ptr::null_mut();
        let rc=native::sqlite3_file_control(connection.handle(),c"main".as_ptr(),native::SQLITE_FCNTL_FILE_POINTER,
            (&raw mut file).cast());
        if rc!=native::SQLITE_OK||file.is_null(){return Err(failed());}
        let methods=(*file).pMethods;
        if methods.is_null()||(*methods).iVersion<1{return Err(failed());}
        let sync=(*methods).xSync.ok_or_else(failed)?;
        if sync(file,native::SQLITE_SYNC_FULL)!=native::SQLITE_OK{return Err(failed());}
    }
    Ok(())
}
