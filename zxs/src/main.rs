use std::{env, ffi::OsStr, io, path::PathBuf, process::ExitCode};
const USAGE:&str="usage: zxs new-id | check-new SYSTEM RECYCLE FILE... | init OP SYSTEM RECYCLE FILE... | inspect SYSTEM | file-create OP SYSTEM VOLUME NAME | file-receipt SYSTEM OP | file-list SYSTEM VOLUME | file-save OP SYSTEM FILE LENGTH SHA256 | file-save-receipt SYSTEM OP | file-info SYSTEM FILE | file-read SYSTEM FILE | file-commit OP SYSTEM FILE WORKING-REV | file-commit-receipt SYSTEM OP | file-node-read SYSTEM FILE NODE | file-node-range SYSTEM FILE NODE OFFSET LENGTH | file-write-range OP SYSTEM FILE REV OFFSET LENGTH SHA256 keep|NEW_SIZE | file-range-receipt SYSTEM OP | file-read-range SYSTEM FILE OFFSET LENGTH";
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
        Some("file-save")=>{
            let op=word(&mut args)?.parse()?;let system=path(&mut args)?;let file=word(&mut args)?.parse()?;
            let len=word(&mut args)?.parse::<u64>().map_err(|_|bad("invalid first SAVE byte length"))?;
            let claim=zxs::DraftClaim::from_hex(len,&word(&mut args)?)?;end(&mut args)?;
            let r=zxs::save_first(&system,op,file,claim,io::stdin().lock())?;
            println!("Original working SAVE: FILE {} revision {} length {} decision {} (no formal node)",r.file,r.revision,r.body.bytes,r.decision_sequence);Ok(())
        }
        Some("file-save-receipt")=>{
            let system=path(&mut args)?;let op=word(&mut args)?.parse()?;end(&mut args)?;
            let r=zxs::first_save_receipt(&system,op)?.ok_or_else(||io::Error::new(io::ErrorKind::NotFound,"no original first SAVE"))?;
            println!("Original first SAVE {:?}: FILE {} length {} published-sequence {:?}",r.status,r.file,r.body.bytes,r.decision_sequence);Ok(())
        }
        Some("file-write-range")=>{
            let op=word(&mut args)?.parse()?;let system=path(&mut args)?;let file=word(&mut args)?.parse()?;
            let rev=word(&mut args)?.parse::<u64>().map_err(|_|bad("invalid source working revision"))?;
            let offset=word(&mut args)?.parse::<u64>().map_err(|_|bad("invalid working write offset"))?;
            let len=word(&mut args)?.parse::<u64>().map_err(|_|bad("invalid input extent byte count"))?;
            let input=zxs::DraftClaim::from_hex(len,&word(&mut args)?)?;
            let p=word(&mut args)?;let policy=if p=="keep"{zxs::LengthPolicy::KeepOrGrow}
                else{zxs::LengthPolicy::Set(p.parse().map_err(|_|bad("expected keep or explicit final size"))?)};
            end(&mut args)?;let r=zxs::save_interval(&system,op,file,rev,zxs::SpanEdit::new(offset,input,policy)?,io::stdin().lock())?;
            show_edit(&r);Ok(())
        }
        Some("file-range-receipt")=>{
            let system=path(&mut args)?;let op=word(&mut args)?.parse()?;end(&mut args)?;
            let r=zxs::interval_receipt(&system,op)?.ok_or_else(||io::Error::new(io::ErrorKind::NotFound,"no such original interval save"))?;
            println!("Original interval {:?}: operation {} FILE {}",r.status,r.operation,r.file);if let Some(e)=r.published{show_edit(&e);}Ok(())
        }
        Some("file-read-range")=>{
            let system=path(&mut args)?;let file=word(&mut args)?.parse()?;
            let off=word(&mut args)?.parse::<u64>().map_err(|_|bad("invalid current range offset"))?;
            let len=word(&mut args)?.parse::<u64>().map_err(|_|bad("invalid current range length"))?;end(&mut args)?;
            zxs::read_current_range_to(&system,file,off,len,io::stdout().lock())?;Ok(())
        }
        Some("file-commit")=>{
            let op=word(&mut args)?.parse()?;let system=path(&mut args)?;let file=word(&mut args)?.parse()?;
            let rev=word(&mut args)?.parse::<u64>().map_err(|_|bad("invalid expected working revision"))?;end(&mut args)?;
            let f=zxs::commit_first(&system,op,file,rev)?;show_node(&f);Ok(())
        }
        Some("file-commit-receipt")=>{
            let system=path(&mut args)?;let op=word(&mut args)?.parse()?;end(&mut args)?;
            let r=zxs::first_commit_receipt(&system,op)?.ok_or_else(||io::Error::new(io::ErrorKind::NotFound,"no such original formal commit"))?;
            println!("Original formal commit {:?}: operation {} FILE {}",r.status,r.operation,r.file);
            if let Some(f)=r.published{show_node(&f);}Ok(())
        }
        Some("file-node-read"|"file-node-range")=>{
            let system=path(&mut args)?;let file=word(&mut args)?.parse()?;let node=word(&mut args)?.parse()?;
            if action==OsStr::new("file-node-read"){
                end(&mut args)?;zxs::read_node_to(&system,file,node,io::stdout().lock())?;
            }else{
                let offset=word(&mut args)?.parse::<u64>().map_err(|_|bad("invalid range offset"))?;
                let length=word(&mut args)?.parse::<u64>().map_err(|_|bad("invalid range length"))?;end(&mut args)?;
                zxs::read_node_range_to(&system,file,node,offset,length,io::stdout().lock())?;
            }Ok(())
        }
        Some("file-info"|"file-read")=>{
            let system=path(&mut args)?;let id=word(&mut args)?.parse()?;end(&mut args)?;
            if action==OsStr::new("file-read"){zxs::read_current_to(&system,id,io::stdout().lock())?;}
            else{
                let current=zxs::current_info(&system,id)?;
                if let Some(e)=&current.edit{show_edit(e);}
                match (current.head,current.working){
                    (Some(f),None)=>show_node(&f),
                    (None,Some(r))=>println!("Current manual FILE: working revision {} length {} (header inspected, no formal node)",r.revision,r.body.bytes),
                    (None,None)=>println!("Current manual FILE: no confirmed working revision, no formal node, 0 logical bytes"),
                    _=>return Err(bad("inconsistent initial formal FILE state")),
                }
                show_file(&current.file);
            }Ok(())
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
fn show_edit(e:&zxs::SpanReceipt){
    println!("Working interval: FILE {} revision {} base node {} offset {} changed bytes {} logical bytes {} original decision {}",
        e.file,e.revision,e.base_node,e.edit.offset,e.edit.input.bytes,e.logical_bytes,e.decision_sequence);
}
fn show_node(f:&zxs::FormalNode){
    println!("Published manual FULL: FILE {} main {} node {} version {} storage {} created-name {} length {} from SAVE {} revision {} decision {}",
        f.file,f.branch,f.node,f.version,f.storage,f.creation_name,f.body.bytes,f.created_from_save,f.working_revision,f.decision_sequence);
}
fn main() -> ExitCode {
    match run() {Ok(())=>ExitCode::SUCCESS,Err(error)=>{eprintln!("zxs: {error}");ExitCode::FAILURE}}
}
