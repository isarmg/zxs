use std::{env, ffi::OsStr, io, path::PathBuf, process::ExitCode};
const USAGE:&str="usage: zxs new-id | check-new SYSTEM RECYCLE FILE... | init OP SYSTEM RECYCLE FILE... | inspect SYSTEM | dir-mkdir OP SYSTEM VOLUME root|DIR NAME | dir-list SYSTEM VOLUME root|DIR | file-create OP SYSTEM VOLUME NAME | file-receipt SYSTEM OP | file-list SYSTEM VOLUME | file-lookup SYSTEM VOLUME NAME | file-save OP SYSTEM FILE LENGTH SHA256 | file-save-receipt SYSTEM OP | file-info SYSTEM FILE | file-read SYSTEM FILE | file-commit OP SYSTEM FILE WORKING-REV | file-commit-receipt SYSTEM OP | file-node-read SYSTEM FILE NODE | file-node-range SYSTEM FILE NODE OFFSET LENGTH | file-write-range OP SYSTEM FILE REV OFFSET LENGTH SHA256 keep|NEW_SIZE | file-range-receipt SYSTEM OP | file-read-range SYSTEM FILE OFFSET LENGTH | file-move OP SYSTEM FILE FROM TO PLACEMENT_EPOCH | file-move-receipt OP SYSTEM FILE FROM TO PLACEMENT_EPOCH | file-delete[-receipt] OP SYSTEM FILE FROM EPOCH | file-restore[-receipt] OP SYSTEM FILE DELETE_OP DELETE_SHA256 RETAINED_EPOCH original|VOLUME | recycle-list SYSTEM LIMIT SOURCE_OR_ALL SINCE_OR_DASH THROUGH_OR_DASH [CURSOR]";
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
fn parent(args:&mut impl Iterator<Item=std::ffi::OsString>)->io::Result<zxs::DirectoryParent>{
    let v=word(args)?.parse()?;let address=word(args)?;
    Ok(if address=="root"{zxs::DirectoryParent::Root(v)}else{zxs::DirectoryParent::Child(v,address.parse()?)})
}
fn show_directory(d:&zxs::DirectoryInfo){
    println!("directory {}  entry {}  name {}  original {}  published-sequence {}",d.directory,d.entry,d.name,d.created_by,d.sequence);
}
fn run() -> io::Result<()> {
    let mut args = env::args_os().skip(1);
    let action=args.next().ok_or_else(||bad(USAGE))?;
    match action.to_str(){
        Some("new-id") if args.next().is_none()=>{println!("{}",zxs::OperationId::new()?);Ok(())}
        Some("dir-mkdir")=>{
            let op=word(&mut args)?.parse()?;let system=path(&mut args)?;let address=parent(&mut args)?;
            let name=word(&mut args)?;end(&mut args)?;let d=zxs::make_directory(&system,op,address,&name)?;
            println!("Original directory creation published (not current availability):");show_directory(&d);Ok(())
        }
        Some("dir-list")=>{
            let system=path(&mut args)?;let address=parent(&mut args)?;end(&mut args)?;let list=zxs::list_directory(&system,address)?;
            for e in &list.entries{match e{zxs::DirectoryEntry::File(f)=>show_file(f),zxs::DirectoryEntry::Directory(d)=>show_directory(d)}}
            for m in &list.pending_moves{eprintln!("Original MOVE {}: {:?}; use file-move-receipt.",m.operation,m.status);}
            for m in &list.pending_lifecycle{eprintln!("Original {:?} {}: {:?}; query its original receipt.",m.kind,m.operation,m.status);}
            if list.reserved_private_entries!=0{eprintln!("{} issued private entries; not business directories or FILEs.",list.reserved_private_entries);}
            Ok(())
        }
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
        Some("file-lookup")=>{
            let system=path(&mut args)?;let volume=word(&mut args)?.parse()?;let name=word(&mut args)?;end(&mut args)?;
            let f=zxs::lookup_file(&system,volume,&name)?.ok_or_else(||io::Error::new(io::ErrorKind::NotFound,"no FILE at this exact display name"))?;
            show_file(&f);Ok(())
        }
        Some("file-list")=>{
            let system=path(&mut args)?;let volume=word(&mut args)?.parse()?;end(&mut args)?;
            let listed=zxs::list_initial_files(&system,volume)?;
            for f in listed.files{show_file(&f);}
            for m in listed.pending_moves{
                let detail=match m.status{
                    zxs::MoveStatus::TargetReopenedSourcePending if m.to==volume=>"target is current on this checked volume; original source cleanup pending",
                    zxs::MoveStatus::TargetReopenedSourcePending=>"target was reopened; original source cleanup pending (target is not inspected here)",
                    zxs::MoveStatus::DecidedTargetPending=>"decided; target is not admitted for reading yet",
                    _=>"not yet decided; source remains its owner",
                };
                eprintln!("Original MOVE {}: {detail}.",m.operation);
            }
            for m in &listed.pending_lifecycle{
                let detail=match (m.kind,m.status){
                    (zxs::RecycleKind::Delete,zxs::MoveStatus::DecidedTargetPending)=>"deletion decided, hidden from ordinary FILEs; retained installation pending",
                    (zxs::RecycleKind::Delete,zxs::MoveStatus::TargetReopenedSourcePending)=>"retained target was reopened; original source cleanup pending",
                    (zxs::RecycleKind::Restore,zxs::MoveStatus::DecidedTargetPending)=>"restoration decided, ordinary target not open for new reads",
                    (zxs::RecycleKind::Restore,zxs::MoveStatus::TargetReopenedSourcePending)=>"restored target is current on this checked volume; recycle source cleanup pending",
                    _=>"not yet decided; previous custody remains authoritative",
                };eprintln!("Original {:?} {} FILE {}: {detail}.",m.kind,m.operation,m.file);
            }
            if listed.reserved_private_entries!=0{
                eprintln!("{} issued private slot(s) occupied: not current FILEs; contents and ownership not confirmed, nothing removed.",listed.reserved_private_entries);
            }
            Ok(())
        }
        Some("file-move"|"file-move-receipt")=>{
            let op=word(&mut args)?.parse()?;let system=path(&mut args)?;let id=word(&mut args)?.parse()?;
            let from=word(&mut args)?.parse()?;let to=word(&mut args)?.parse()?;
            let epoch=word(&mut args)?.parse().map_err(|_|bad("invalid original FILE placement epoch"))?;end(&mut args)?;
            let r=if action==OsStr::new("file-move"){zxs::move_file(&system,op,id,from,to,epoch)?}
                else{zxs::move_receipt(&system,op,id,from,to,epoch)?.ok_or_else(||io::Error::new(io::ErrorKind::NotFound,"no such original MOVE"))?};
            println!("Original MOVE {:?}: {} FILE {} entry {} history {} {}@{} -> {}@{} name {} decision {:?}",
                r.status,r.operation,r.file,r.entry,r.history,r.from,r.source_epoch,r.to,r.destination_epoch,r.name,r.decision_sequence);Ok(())
        }
        Some("file-delete"|"file-delete-receipt"|"file-restore"|"file-restore-receipt")=>{
            let op=word(&mut args)?.parse()?;let system=path(&mut args)?;let file=word(&mut args)?.parse()?;
            let (what,epoch)=if matches!(action.to_str(),Some("file-delete"|"file-delete-receipt")){
                let from=word(&mut args)?.parse()?;let epoch=number(&word(&mut args)?)?;
                (zxs::RecycleAction::Delete{file,from},epoch)
            }else{
                let operation=word(&mut args)?.parse()?;let deleted=zxs::DeletionKey::from_hex(file,operation,&word(&mut args)?)?;
                let epoch=number(&word(&mut args)?)?;let dest=word(&mut args)?;
                let target=if dest=="original"{zxs::RestoreTarget::Original}else{zxs::RestoreTarget::Volume(dest.parse()?)};
                (zxs::RecycleAction::Restore{deleted,target},epoch)
            };
            end(&mut args)?;
            let r=if matches!(action.to_str(),Some("file-delete-receipt"|"file-restore-receipt")){
                zxs::recycle_receipt(&system,op,epoch,what)?.ok_or_else(||io::Error::new(io::ErrorKind::NotFound,"no such original dedicated FILE operation"))?
            }else{zxs::apply_recycle(&system,op,epoch,what)?};
            show_recycle(&r);Ok(())
        }
        Some("recycle-list")=>{
            let system=path(&mut args)?;let limit=word(&mut args)?.parse::<u16>().map_err(|_|bad("invalid page limit"))?;
            let source=word(&mut args)?;let ordinary_source=if source=="all"{None}else{Some(source.parse()?)};
            let since_unix_seconds=optional_number(&word(&mut args)?)?;let through_unix_seconds=optional_number(&word(&mut args)?)?;
            let cursor=args.next().map(|v|v.into_string().map_err(|_|bad("non-ASCII cursor"))?.parse::<zxs::RecycleCursor>()).transpose()?;
            end(&mut args)?;let filter=zxs::RecycleFilter{ordinary_source,since_unix_seconds,through_unix_seconds};
            let page=zxs::list_retained(&system,filter,limit,cursor.as_ref())?;
            println!("Retained at SYSTEM sequence {} {} (original metadata only; storage availability not inspected):",
                page.as_of_sequence,hex_digest(&page.as_of_digest));
            for item in page.entries{show_recycle(&item.original);println!("  retained-by-this-original-now {}",item.retained_by_this_deletion_now);
                if let Some(restore)=item.pending_restore{print!("  pending ");show_recycle(&restore);}}
            if let Some(c)=page.next{println!("next {c}");}Ok(())
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
            let f=zxs::commit_working(&system,op,file,rev)?;show_node(&f);Ok(())
        }
        Some("file-commit-receipt")=>{
            let system=path(&mut args)?;let op=word(&mut args)?.parse()?;end(&mut args)?;
            let r=zxs::formal_receipt(&system,op)?.ok_or_else(||io::Error::new(io::ErrorKind::NotFound,"no such original formal commit"))?;
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
fn number(s:&str)->io::Result<u64>{s.parse().map_err(|_|bad("invalid nonnegative integer"))}
fn optional_number(s:&str)->io::Result<Option<u64>>{if s=="-"{Ok(None)}else{number(s).map(Some)}}
fn show_recycle(r:&zxs::RecycleReceipt){
    println!("Original {:?} {:?}: {} FILE {} entry {} history {} {}@{} -> {}@{} name {:?} decision-sequence {:?} decision {}",
        r.kind,r.status,r.operation,r.file,r.entry,r.history,r.from,r.source_epoch,r.to,r.destination_epoch,r.name,r.decision_sequence,
        r.decision.as_ref().map(hex_digest).unwrap_or_else(||"unconfirmed".to_owned()));
    println!("  deletion {} issued-utc-unix {}.{:09} delete-decision {} (original outcome, not an online health check)",
        r.delete_operation,r.deleted_at.unix_seconds,r.deleted_at.nanos,
        r.deletion.map(|key|hex_digest(&key.decision)).unwrap_or_else(||"unconfirmed".to_owned()));
}
fn show_edit(e:&zxs::SpanReceipt){
    println!("Working interval: FILE {} revision {} base node {} offset {} changed bytes {} logical bytes {} original decision {}",
        e.file,e.revision,e.base_node,e.edit.offset,e.edit.input.bytes,e.logical_bytes,e.decision_sequence);
}
fn hex_digest(digest:&[u8;32])->String{digest.iter().map(|v|format!("{v:02x}")).collect()}
fn show_node(f:&zxs::FormalNode){
    let kind=match &f.kind{zxs::FormalStorage::Full=>"FULL".to_owned(),zxs::FormalStorage::Incremental{parent,parent_version,changed}=>
        format!("INCR parent {parent} parent-version {parent_version} packed-bytes {} packed-sha256 {}",changed.bytes,hex_digest(&changed.sha256))};
    println!("Published manual {kind}: FILE {} main {} node {} version {} storage {} created-name {} length {} body-sha256 {} from SAVE {} revision {} decision {}",
        f.file,f.branch,f.node,f.version,f.storage,f.creation_name,f.body.bytes,hex_digest(&f.body.sha256),f.created_from_save,f.working_revision,f.decision_sequence);
}
fn main() -> ExitCode {
    match run() {Ok(())=>ExitCode::SUCCESS,Err(error)=>{eprintln!("zxs: {error}");ExitCode::FAILURE}}
}
