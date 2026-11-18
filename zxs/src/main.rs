use std::{env, ffi::OsStr, io, path::PathBuf, process::ExitCode};
const USAGE:&str="usage: zxs new-id | check-new SYSTEM RECYCLE FILE... | init OP SYSTEM RECYCLE FILE... | inspect SYSTEM";
fn bad(message:&'static str)->io::Error {io::Error::new(io::ErrorKind::InvalidInput,message)}
fn show(scope:&str,info:zxs::PoolInfo) {
    println!("{scope}");
    println!("pool {} initialized-by {}",info.id,info.initialized_by);
    for reg in info.volumes {
        println!("{:?}\t{}\t{:?}\tdevice={} inode={}",reg.volume.role,reg.id,reg.volume.path,
            reg.volume.entity.device,reg.volume.entity.inode);
    }
}
fn run() -> io::Result<()> {
    let mut args = env::args_os().skip(1);
    let action=args.next().ok_or_else(||bad(USAGE))?;
    match action.to_str(){
        Some("new-id") if args.next().is_none()=>{println!("{}",zxs::OperationId::new()?);Ok(())}
        Some("inspect")=>{
            let root=args.next().map(PathBuf::from).ok_or_else(||bad(USAGE))?;
            if args.next().is_some(){return Err(bad(USAGE));} show("Current registrations inspected:",zxs::inspect(&root)?);Ok(())
        }
        Some("check-new"|"init")=>{
            let op=if action==OsStr::new("init"){
                Some(args.next().and_then(|a|a.into_string().ok()).ok_or_else(||bad(USAGE))?.parse()?)
            }else{None};
            let system=args.next().map(PathBuf::from).ok_or_else(||bad(USAGE))?;
            let recycle=args.next().map(PathBuf::from).ok_or_else(||bad(USAGE))?;
            let files:Vec<_>=args.take(zxs::MAX_FILE_ROOTS+1).map(PathBuf::from).collect();
            if let Some(op)=op {show("Original registration receipt (not current data-volume availability):",zxs::initialize(op,&system,&recycle,&files)?);}
            else {
                for root in zxs::inspect_fresh(&system,&recycle,&files)? {
                    println!("{:?}\t{:?}\tdevice={} inode={}",root.role,root.path,root.entity.device,root.entity.inode);
                }
                println!("Fresh directory checks passed at inspection; no pool was initialized.");
            }
            Ok(())
        }
        _=>Err(bad(USAGE))
    }
}
fn main() -> ExitCode {
    match run() {Ok(())=>ExitCode::SUCCESS,Err(error)=>{eprintln!("zxs: {error}");ExitCode::FAILURE}}
}
