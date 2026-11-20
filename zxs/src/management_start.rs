//! Trusted pool selection. No Web settings, certificate or socket unit are
//! necessary for native HTTP. Forwarding headers are never configuration.
use std::{env,io,path::PathBuf};
unsafe extern "C"{
    fn zxs_web_signals()->std::os::raw::c_int;
    fn zxs_web_stopping()->std::os::raw::c_int;
}
fn invalid()->io::Error{io::Error::new(io::ErrorKind::InvalidInput,"invalid protected management service configuration")}
pub(super) fn run()->io::Result<()>{
    let system=PathBuf::from(env::var_os("ZXS_SYSTEM_ROOT").ok_or_else(invalid)?);
    let pool=env::var("ZXS_POOL_ID").map_err(|_|invalid())?.parse::<zxs::PoolId>()?;
    let proxy=match (env::var_os("ZXS_WEB_TRUSTED_PROXY"),env::var_os("ZXS_WEB_PUBLIC_ORIGIN")){
        (None,None)=>None,
        (Some(peer),Some(origin))=>Some(zxs::management::ExternalHttps::new(
            peer.to_str().ok_or_else(invalid)?.parse().map_err(|_|invalid())?,
            origin.to_str().ok_or_else(invalid)?)?),
        _=>return Err(invalid())
    };
    if unsafe{zxs_web_signals()}<0{return Err(io::Error::last_os_error());}
    zxs::management::serve(&system,pool,proxy,||unsafe{zxs_web_stopping()!=0})
}
