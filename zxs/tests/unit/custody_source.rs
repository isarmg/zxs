use super::*;
use crate::{DraftClaim,FormalNode,LengthPolicy,OperationId,Role,SpanEdit,create_empty,initialize,save_first,save_interval,
    commit_working,read_node_to,test_support::PrivateTemp};
use std::{fs as osfs,io::Cursor,os::unix::fs::{FileExt,PermissionsExt},path::PathBuf};
fn claim(b:&[u8])->DraftClaim{DraftClaim::new(b.len() as u64,crate::hash::digest(b).unwrap()).unwrap()}
struct Original{_top:PrivateTemp,_r:PrivateTemp,system:PathBuf,dir:PathBuf,first:FormalNode}
impl Original{
    fn new(bytes:&[u8])->Self{
        let t=PrivateTemp::under(&std::env::temp_dir());let system=t.child("system");let root=t.child("data");
        let r=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
        let p=initialize(OperationId::new().unwrap(),&system,&r.child("recycle"),std::slice::from_ref(&root)).unwrap();
        let vol=p.volumes.iter().find(|x|x.volume.role==Role::File).unwrap().id;let f=create_empty(&system,OperationId::new().unwrap(),vol,"normal").unwrap();
        save_first(&system,OperationId::new().unwrap(),f.file,claim(bytes),Cursor::new(bytes)).unwrap();
        let first=commit_working(&system,OperationId::new().unwrap(),f.file,1).unwrap();Self{_top:t,_r:r,system,dir:root.join("normal"),first}
    }
    fn write(&self,at:u64,bytes:&[u8]){
        save_interval(&self.system,OperationId::new().unwrap(),self.first.file,1,
            SpanEdit::new(at,claim(bytes),LengthPolicy::KeepOrGrow).unwrap(),Cursor::new(bytes)).unwrap();
    }
    fn second(&self)->FormalNode{commit_working(&self.system,OperationId::new().unwrap(),self.first.file,2).unwrap()}
}
#[test]
#[ignore="private native FILE and dedicated recycle filesystems"]
fn source_lends_only_the_exact_physical_ancestors_and_index_package_of_its_clean_formal_history(){
    let t=Original::new(b"old baseline");let id=t.first.file;
    let clean=Clean::capture(&t.system,id).unwrap().verify().unwrap();assert_eq!(clean.objects().len(),3);
    assert_eq!(clean.head(),t.first.node);drop(clean);
    t.write(1,b"x");assert!(Clean::capture(&t.system,id).is_err());let second=t.second();
    let clean=Clean::capture(&t.system,id).unwrap().verify().unwrap();
    assert_eq!(clean.file(),id);assert_eq!(clean.history(),t.first.history);assert_eq!(clean.head(),second.node);
    let mut raw_total=0;
    for e in clean.objects(){
        let native=std::ffi::OsStr::from_bytes(e.name.to_bytes());
        let actual=osfs::read(t.dir.join(native)).unwrap();let mut gathered=Vec::new();
        clean.stream(e.kind,|at,b|{assert_eq!(gathered.len() as u64,at);gathered.extend_from_slice(b);Ok(())}).unwrap();
        assert_eq!(gathered,actual);assert_eq!(e.digest,crate::hash::digest(&actual).unwrap());raw_total+=e.len;
    }
    assert_eq!(raw_total,clean.physical_bytes());assert_eq!(clean.objects().len(),4);
}
use std::os::unix::ffi::OsStrExt;
#[test]
#[ignore="two private registered native filesystems"]
fn entirely_overwritten_raw_corruption_still_blocks_the_complete_history_handoff(){
    let t=Original::new(b"first");let id=t.first.file;t.write(0,b"FINAL");let d=t.second();
    let f=osfs::OpenOptions::new().write(true).open(t.dir.join(format!("{}-normal",t.first.storage))).unwrap();
    f.write_all_at(b"!",0).unwrap();f.sync_all().unwrap();
    let mut latest=Vec::new();read_node_to(&t.system,id,d.node,&mut latest).unwrap();assert_eq!(latest,b"FINAL");
    assert!(Clean::capture(&t.system,id).unwrap().verify().is_err());
}
#[test]
#[ignore="two private registered native filesystems"]
fn unknown_occupants_and_equal_byte_new_native_files_never_become_transfer_sources(){
    let t=Original::new(b"first");let id=t.first.file;osfs::write(t.dir.join("extra"),b"do not touch").unwrap();
    assert!(Clean::capture(&t.system,id).is_err());assert_eq!(osfs::read(t.dir.join("extra")).unwrap(),b"do not touch");
    osfs::remove_file(t.dir.join("extra")).unwrap();
    let verified=Clean::capture(&t.system,id).unwrap().verify().unwrap();let path=t.dir.join(format!("{}-normal",t.first.storage));
    let separate=t._top.0.join("old-original");osfs::rename(&path,&separate).unwrap();
    osfs::write(&path,osfs::read(&separate).unwrap()).unwrap();osfs::set_permissions(&path,osfs::Permissions::from_mode(0o600)).unwrap();
    let mut output=Vec::new();assert!(verified.stream(Leaf::FullRaw,|_,b|{output.extend_from_slice(b);Ok(())}).is_err());
    assert!(output.is_empty());assert!(separate.exists()&&path.exists());
}

#[test]
#[ignore="two private registered native filesystems"]
fn ordinary_user_current_and_fixed_history_pin_actual_leaf_leases_without_claiming_the_retired_working_record(){
    use std::io::Write;
    struct Probe{files:Vec<PathBuf>,flush:bool,bytes:Vec<u8>}
    impl Probe{
        fn verify_leases(&self)->io::Result<()>{
            for path in &self.files{
                let fd=osfs::File::open(path)?;
                match fd.try_lock(){
                    Err(std::fs::TryLockError::WouldBlock)=>{},
                    Err(e)=>return Err(io::Error::from(e)),
                    Ok(())=>return Err(io::Error::other("a reader lost an original retirement lease")),
                }
            }Ok(())
        }
    }
    impl Write for Probe{
        fn write(&mut self,b:&[u8])->io::Result<usize>{self.verify_leases()?;self.bytes.extend_from_slice(b);Ok(b.len())}
        fn flush(&mut self)->io::Result<()>{self.verify_leases()?;self.flush=true;Ok(())}
    }
    let t=Original::new(b"first");let id=t.first.file;
    let first=t.dir.join(format!("{}-normal",t.first.storage));let first_node=t.dir.join(format!("node-{}",t.first.node));
    let mut before=Probe{files:vec![first.clone(),first_node.clone()],flush:false,bytes:Vec::new()};
    crate::read_current_to(&t.system,id,&mut before).unwrap();assert!(before.flush);assert_eq!(before.bytes,b"first");
    t.write(0,b"NEW");let next=t.second();let incr=t.dir.join(format!("{}-normal",next.storage));
    for current in [true,false]{
        let mut proof=Probe{files:vec![first.clone(),first_node.clone(),incr.clone()],flush:false,bytes:Vec::new()};
        if current{crate::read_current_range_to(&t.system,id,0,1,&mut proof).unwrap();}else{crate::read_node_range_to(&t.system,id,next.node,0,1,&mut proof).unwrap();}
        assert!(proof.flush);assert_eq!(proof.bytes,b"N");
    }
    let mut old=Probe{files:vec![first,first_node],flush:false,bytes:Vec::new()};
    crate::read_node_to(&t.system,id,t.first.node,&mut old).unwrap();assert!(old.flush);assert_eq!(old.bytes,b"first");
    for p in old.files.iter().chain(std::iter::once(&incr)){let fd=osfs::File::open(p).unwrap();fd.try_lock().unwrap();}
}

#[test]
#[ignore="private registered FILE and dedicated recycle filesystems"]
fn verified_older_inventory_cannot_lend_an_extra_child_or_repeat_a_real_pack_at_a_different_ordinal(){
    let t=Original::new(b"baseline");let id=t.first.file;t.write(1,b"X");t.second();
    let live=Clean::capture(&t.system,id).unwrap().verify().unwrap();
    let(p,_,s)=LocalPool::with_snapshot(&t.system,Access::Read).unwrap();let(c,f,ds)=live.originals();
    let v=p.ordinary(live.volume()).unwrap();let native=Sites::birthplace(c,f,ds);
    live.require_at(&v,c"normal",live.source_root(),&native).unwrap();
    let one=*ds.first().unwrap();let object=native.children.first().unwrap().clone();
    let mut wrong=native.clone();wrong.children=ChildRoles::Two(object.clone(),object);
    assert!(live.require_at(&v,c"normal",live.source_root(),&wrong).is_err());
    let dir=v.root.child_directory(c"normal").unwrap().unwrap();
    assert!(Clean::at_original_sites(&v,dir,c,f,ChildRoles::Two(one,one),&wrong).is_err());
    assert_eq!(s.actual_graph(id).unwrap().hashes(),ds.map(|d|d.digest));
    assert!(live.stream(Leaf::IncrNext,|_,_|panic!("an unissued ordinal furnished bytes")).is_err());
}
