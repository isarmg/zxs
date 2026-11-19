//! Startup from a trusted host service unit, never from client-supplied paths.
use std::{env,io,os::fd::FromRawFd,os::unix::net::UnixListener,path::PathBuf};
unsafe extern "C"{
    fn zxs_web_listener(fd:std::os::raw::c_int)->std::os::raw::c_int;
    fn zxs_web_signals()->std::os::raw::c_int;
    fn zxs_web_stopping()->std::os::raw::c_int;
}
fn invalid()->io::Error{io::Error::new(io::ErrorKind::InvalidInput,"missing or invalid protected Web service configuration")}
pub(super) fn run()->io::Result<()>{
    let var=|k:&str|env::var(k).map_err(|_|invalid());
    if var("LISTEN_PID")?!=std::process::id().to_string()||var("LISTEN_FDS")?!="1"||var("LISTEN_FDNAMES")?!="zxs-management"{return Err(invalid());}
    let system=PathBuf::from(env::var_os("ZXS_SYSTEM_ROOT").ok_or_else(invalid)?);
    let pool=var("ZXS_POOL_ID")?.parse::<zxs::PoolId>()?;let origin=var("ZXS_WEB_ORIGIN")?;
    let proxy=var("ZXS_WEB_PROXY_UID")?.parse::<u32>().map_err(|_|invalid())?;
    if unsafe{zxs_web_listener(3)}<0||unsafe{zxs_web_signals()}<0{return Err(io::Error::last_os_error());}
    // SAFETY: this process inherited one validated listening descriptor for its
    // own PID, filesystem AF_UNIX/SOCK_STREAM/SO_ACCEPTCONN. Only main takes it.
    let listener=unsafe{UnixListener::from_raw_fd(3)};
    zxs::management::serve(&system,pool,&origin,proxy,listener,||unsafe{zxs_web_stopping()!=0})
}
