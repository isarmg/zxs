use std::{env, io, path::PathBuf, process::ExitCode};
fn run() -> io::Result<()> {
    let mut args = env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("check-new")) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "usage: zxs check-new SYSTEM RECYCLE FILE [FILE ...]"));
    }
    let mut path = || args.next().map(PathBuf::from).ok_or_else(||
        io::Error::new(io::ErrorKind::InvalidInput,"both SYSTEM and RECYCLE directories are required"));
    let system = path()?;
    let recycle = path()?;
    let files:Vec<_> = args.take(zxs::MAX_FILE_ROOTS+1).map(PathBuf::from).collect();
    for root in zxs::inspect_fresh(&system,&recycle,&files)? {
        println!("{:?}\t{:?}\tdevice={} inode={}",root.role,root.path,root.entity.device,root.entity.inode);
    }
    println!("Fresh directory checks passed at inspection; no pool was initialized.");
    Ok(())
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => { eprintln!("zxs: {error}"); ExitCode::FAILURE }
    }
}
