//! Separate non-root, inherited-descriptor service. No commands, listener,
//! passdb secret reads, Web actions or SMB client logins.
use std::{env,fs::File,io,os::fd::{FromRawFd,OwnedFd},path::PathBuf,process::ExitCode};
unsafe extern "C"{
    fn zxs_smb_ctl_claim(fd:i32)->i32;
    fn zxs_smb_bound_fd(fd:i32)->i32;
    fn zxs_web_signals()->i32;
    fn zxs_web_stopping()->i32;
}
fn run()->io::Result<()>{
    let bad=||io::Error::other("incomplete dedicated SMB service launch");
    if env::args_os().len()!=1{return Err(bad());}
    let var=|k:&str|env::var(k).map_err(|_|bad());
    let system=PathBuf::from(env::var_os("ZXS_SYSTEM_ROOT").ok_or_else(bad)?);
    let pool=var("ZXS_POOL_ID")?.parse()?;let instance=var("ZXS_SAMBA_INSTANCE_ID")?;
    let op=var("ZXS_SAMBA_PROVISION_OP")?.parse()?;let config=var("ZXS_SAMBA_LIVE_CONFIG_SHA256")?;
    if unsafe{zxs_smb_ctl_claim(3)}<0||unsafe{zxs_smb_bound_fd(4)}<0||unsafe{zxs_web_signals()}<0{
        return Err(bad());
    }
    // This fresh executable's launch contract owns precisely these fixed
    // descriptors; never from_raw_fd for a number taken from a client.
    let link=unsafe{OwnedFd::from_raw_fd(3)};let bound=unsafe{File::from_raw_fd(4)};
    zxs::serve_smb_storage(&system,pool,&instance,op,&config,link,bound,||unsafe{zxs_web_stopping()!=0})
}
fn main()->ExitCode{
    match run(){Ok(())=>ExitCode::SUCCESS,Err(_)=>{eprintln!("The dedicated SMB storage service did not qualify or drain.");ExitCode::FAILURE}}
}
