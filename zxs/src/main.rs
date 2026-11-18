use std::{env, ffi::OsStr, io, path::PathBuf, process::ExitCode};
const USAGE:&str="usage: zxs new-id | check-new SYSTEM RECYCLE FILE... | init OP SYSTEM RECYCLE FILE... | inspect SYSTEM | file-create OP SYSTEM VOLUME NAME | file-receipt SYSTEM OP | file-list SYSTEM VOLUME | file-info SYSTEM FILE | file-read SYSTEM FILE";
fn bad(message:&'static str)->io::Error {io::Error::new(io::ErrorKind::InvalidInput,message)}
fn show(scope:&str,info:zxs::PoolInfo) {
    println!("{scope}");
    println!("pool {} initialized-by {}",info.id,info.initialized_by);
    for reg in info.volumes {
        println!("{:?}\t{}\t{:?}\tdevice={} inode={}",reg.volume.role,reg.id,reg.volume.path,
            reg.volume.entity.device,reg.volume.entity.inode);
    }
}
fn word(args:&mut impl Iterator<Item=std::ffi::OsString>)->io::Result<String>{
    args.next().and_then(|a|a.into_string().ok()).ok_or_else(||bad(USAGE))
}
fn path(args:&mut impl Iterator<Item=std::ffi::OsString>)->io::Result<PathBuf>{
    args.next().map(PathBuf::from).ok_or_else(||bad(USAGE))
}
fn end(args:&mut impl Iterator<Item=std::ffi::OsString>)->io::Result<()>{
    if args.next().is_some(){Err(bad(USAGE))}else{Ok(())}
}
fn show_file(f:&zxs::FileInfo){
    println!("file {}  entry {}  history {}  volume {}  name {}  original {}  published-sequence {:?}",
        f.file,f.entry,f.history,f.volume,f.name,f.created_by,f.decision_sequence);
}
fn run() -> io::Result<()> {
    let mut args = env::args_os().skip(1);
    let action=args.next().ok_or_else(||bad(USAGE))?;
    match action.to_str(){
        Some("new-id") if args.next().is_none()=>{println!("{}",zxs::OperationId::new()?);Ok(())}
        Some("file-create")=>{
            let op=word(&mut args)?.parse()?;let system=path(&mut args)?;let volume=word(&mut args)?.parse()?;
            let name=word(&mut args)?;end(&mut args)?;
            let f=zxs::create_empty(&system,op,volume,&name)?;
            println!("Original FILE creation published (not current availability):");show_file(&f);Ok(())
        }
        Some("file-receipt")=>{
            let system=path(&mut args)?;let op=word(&mut args)?.parse()?;end(&mut args)?;
            let r=zxs::creation_receipt(&system,op)?.ok_or_else(||io::Error::new(io::ErrorKind::NotFound,"no original FILE creation"))?;
            println!("Original FILE creation: {:?}",r.status);show_file(&r.file);Ok(())
        }
        Some("file-list")=>{
            let system=path(&mut args)?;let volume=word(&mut args)?.parse()?;end(&mut args)?;
            let listed=zxs::list_initial_files(&system,volume)?;
            for f in listed.files{show_file(&f);}
            if listed.reserved_private_entries!=0{
                eprintln!("{} issued private slot(s) occupied: not current FILEs; contents and ownership not confirmed, nothing removed.",listed.reserved_private_entries);
            }
            Ok(())
        }
        Some("file-info"|"file-read")=>{
            let system=path(&mut args)?;let id=word(&mut args)?.parse()?;end(&mut args)?;
            let (file,bytes)=zxs::read_initial_file(&system,id)?;
            if action==OsStr::new("file-read"){
                use std::io::Write;
                let mut out=io::stdout().lock();out.write_all(&bytes)?;out.flush()
            }else{println!("Current initial FILE: manual, no formal node, 0 logical bytes");show_file(&file);Ok(())}
        }
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
