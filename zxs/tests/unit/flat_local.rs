use super::*;
use crate::{FileId,NodeId,OperationId,PoolId,VersionId,ordinary::flat::{Build,Index,Leaf,stored,write_candidate_checksums},test_support::PrivateTemp};
use std::{ffi::{CStr,OsStr},fs::{self,OpenOptions},io::Write,os::unix::{ffi::OsStrExt,fs::{OpenOptionsExt,PermissionsExt}},path::{Path,PathBuf}};

const P:PoolId=PoolId([1;16]);const F:FileId=FileId([2;16]);const OP:OperationId=OperationId([5;16]);
const BUDGET:ReadBudget=ReadBudget{file_descriptors:6,buffer_bytes:512};
fn key()->VersionKey{VersionKey{pool:P,file:F,node:NodeId([3;16]),version:VersionId([4;16]),representation:7}}
fn path(dir:&Path,name:&CStr)->PathBuf{dir.join(OsStr::from_bytes(name.to_bytes()))}
fn create(path:&Path,bytes:&[u8])->File{
    let mut f=OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(path).unwrap();
    f.write_all(bytes).unwrap();f.sync_all().unwrap();f
}
fn witness(parent:&Path,main:&CStr,n:&CStr){
    fs::hard_link(path(parent,main),path(parent,n)).unwrap();File::open(parent).unwrap().sync_all().unwrap();
}
fn located(original:MaterialRef,full:bool)->Located{
    if full{Located::full(original,VersionStorageId::checked(*b"abc123def456789").unwrap(),"方案.xlsx").unwrap()}
    else{Located::change(original)}
}
fn material(parent:&Path,slot:u32,data:&[u8],full:bool)->MaterialRef{
    let m=Material::declared(F,OP,slot,data.len() as u64).unwrap();
    // Only an unissued coordinate for the fixture's exclusive creations. Its
    // blank hash is never the expected original used for reading the bytes.
    let names=located(MaterialRef::declared(P,m,[0;32]).unwrap(),full);
    let(a,ar,b,br)=names.names();let raw=create(&path(parent,&a),data);let checks=create(&path(parent,&b),&[]);
    let r=write_candidate_checksums(P,m,&raw,&checks).unwrap();
    witness(parent,&a,&ar);witness(parent,&b,&br);r
}
fn index(parent:&Path,i:&Index)->Descriptor{
    index_for(parent,key(),i)
}
fn index_for(parent:&Path,k:VersionKey,i:&Index)->Descriptor{
    let(d,b)=stored::encode(k,i).unwrap();let(a,w)=index_names(d.key);
    drop(create(&path(parent,&a),&b));witness(parent,&a,&w);d
}
fn private(parent:&Path){fs::create_dir(parent).unwrap();fs::set_permissions(parent,fs::Permissions::from_mode(0o700)).unwrap();}

#[test]
fn exact_held_parent_case_preserved_raw_and_changed_material_reopen_without_ancestor_or_native_handle(){
    let t=PrivateTemp::under(&std::env::temp_dir());let root=t.0.join("container");private(&root);
    let a=material(&root,3,b"abcdef",true);let b=material(&root,307,b"XYZ",false);
    let mut plan=Build::new(F).unwrap();
    for(leaf,at,n) in [(Leaf::Data(a.material),0,2),(Leaf::Data(b.material),1,2),(Leaf::Zero,0,2),(Leaf::Data(a.material),4,2)]{
        plan.add(leaf,at,n).unwrap();
    }
    let d=index(&root,&plan.finish(8).unwrap());let parent=HeldDir::open(&root).unwrap();
    let orig=|m:Material|->io::Result<Located>{if m==a.material{Ok(located(a,true))}
        else if m==b.material{Ok(located(b,false))}else{Err(invalid("not this original material"))}};
    let v=LocalVersion::open(&parent,d,orig).unwrap();let mut bytes=Vec::new();
    v.range_to(0,8,BUDGET,&mut bytes).unwrap();assert_eq!(bytes,b"abYZ\0\0ef");
    assert!(root.join("abc123def456789-方案.xlsx").is_file());bytes.clear();
    let raw=located(a,true).names().0;let guard=File::open(path(&root,&raw)).unwrap();guard.try_lock().unwrap();
    assert!(v.range_to(0,1,BUDGET,&mut bytes).is_err());assert!(bytes.is_empty());
    v.range_to(2,2,BUDGET,&mut bytes).unwrap();assert_eq!(bytes,b"YZ");guard.unlock().unwrap();
    // The original index itself remains locked across idle time; a sibling
    // lookup below was not allowed to rebind it by name.
    let(idx,_)=index_names(d.key);let ip=File::open(path(&root,&idx)).unwrap();assert!(ip.try_lock().is_err());
    drop(v);ip.try_lock().unwrap();
}

#[test]
fn two_link_same_byte_replacements_wrong_witness_and_wrong_parent_do_not_enable_a_read_or_repair(){
    let t=PrivateTemp::under(&std::env::temp_dir());let root=t.0.join("container");private(&root);
    let m=material(&root,11,b"data",false);let d=index(&root,&Index::full(m.material).unwrap());
    let parent=HeldDir::open(&root).unwrap();
    let original=|_:Material|Ok(located(m,false));
    let v=LocalVersion::open(&parent,d,original).unwrap();
    let(a,w,b,wc)=located(m,false).names();let(idx,iw)=index_names(d.key);let mut bytes=Vec::new();
    for (primary,wit) in [(&a,&w),(&b,&wc)]{
        let here=path(&root,primary);let right=path(&root,wit);
        let old=t.0.join("old");let shadow=t.0.join("shadow");
        fs::rename(&here,&old).unwrap();let saved=fs::read(&old).unwrap();drop(create(&here,&saved));
        fs::hard_link(&here,&shadow).unwrap(); // replacement has the correct nlink, mode, extent and bytes
        assert!(v.range_to(0,4,BUDGET,&mut bytes).is_err());assert!(bytes.is_empty());
        assert_eq!(fs::read(&right).unwrap(),saved);assert_eq!(fs::read(&shadow).unwrap(),saved);
        fs::remove_file(&here).unwrap();fs::remove_file(&shadow).unwrap();fs::rename(&old,&here).unwrap();
        fs::rename(&right,&old).unwrap();assert!(v.range_to(0,4,BUDGET,&mut bytes).is_err());assert!(!right.exists());
        fs::rename(old,right).unwrap();
    }
    // An equal-byte substitute for the *index* also has its own second link.
    let here=path(&root,&idx);let old=t.0.join("old-index");fs::rename(&here,&old).unwrap();
    drop(create(&here,&fs::read(&old).unwrap()));fs::hard_link(&here,t.0.join("foreign-index-witness")).unwrap();
    assert!(LocalVersion::open(&parent,d,original).is_err());assert!(path(&root,&iw).is_file());
    fs::rename(&here,t.0.join("foreign-index")).unwrap();fs::rename(old,here).unwrap();
    // Refetch after changing the actual INDEX name: the stale live stamp
    // must not explain the separate parent-substitution failure below.
    drop(v);let v=LocalVersion::open(&parent,d,original).unwrap();
    v.range_to(0,1,BUDGET,&mut bytes).unwrap();assert_eq!(bytes,b"d");bytes.clear();
    // Reuse the selected parent after a same-path container swap: an old
    // held directory is not consent to reopen the replacement.
    let original_dir=t.0.join("original");fs::rename(&root,&original_dir).unwrap();private(&root);
    assert!(v.range_to(0,1,BUDGET,&mut bytes).is_err());assert!(bytes.is_empty());
    fs::rename(&root,t.0.join("keep-substitute")).unwrap();fs::rename(original_dir,&root).unwrap();
    // A real cold selection after exact restoration still needs both names.
    drop(v);let restored=LocalVersion::open(&parent,d,original).unwrap();
    restored.range_to(0,4,BUDGET,&mut bytes).unwrap();assert_eq!(bytes,b"data");
    let extra=t.0.join("extra");fs::hard_link(path(&root,&a),&extra).unwrap();bytes.clear();
    assert!(restored.range_to(0,4,BUDGET,&mut bytes).is_err());assert!(bytes.is_empty());assert_eq!(fs::read(extra).unwrap(),b"data");
}

#[test]
fn private_parent_and_leaf_facts_are_still_required_for_zero_and_after_slow_flush(){
    struct During<'a,F>{bytes:&'a mut Vec<u8>,done:F}
    impl<F:FnMut()->io::Result<()>> Write for During<'_,F>{
        fn write(&mut self,b:&[u8])->io::Result<usize>{self.bytes.extend_from_slice(b);Ok(b.len())}
        fn flush(&mut self)->io::Result<()>{(self.done)()}
    }
    let t=PrivateTemp::under(&std::env::temp_dir());let root=t.0.join("private");private(&root);
    let r=material(&root,8,b"A",false);let mut plan=Build::new(F).unwrap();
    plan.add(Leaf::Zero,0,1).unwrap();plan.add(Leaf::Data(r.material),0,1).unwrap();
    let d=index(&root,&plan.finish(2).unwrap());let parent=HeldDir::open(&root).unwrap();
    let v=LocalVersion::open(&parent,d,|_|Ok(located(r,false))).unwrap();let mut out=Vec::new();
    let none=ReadBudget{file_descriptors:0,buffer_bytes:0};
    fs::set_permissions(&root,fs::Permissions::from_mode(0o755)).unwrap();
    assert!(v.range_to(0,1,none,&mut out).is_err());assert!(out.is_empty());
    fs::set_permissions(&root,fs::Permissions::from_mode(0o700)).unwrap();
    v.range_to(0,1,none,&mut out).unwrap();assert_eq!(out,[0]);out.clear();
    let mut calls=0;
    assert!(v.range_to(1,1,BUDGET,During{bytes:&mut out,done:||{
        calls+=1;fs::set_permissions(&root,fs::Permissions::from_mode(0o755))
    }}).is_err());assert_eq!(calls,1);assert_eq!(out,b"A");out.clear();
    fs::set_permissions(&root,fs::Permissions::from_mode(0o700)).unwrap();
    let raw=path(&root,&located(r,false).names().0);
    for third_link in [false,true]{
        let extra=t.0.join("late-external-link");
        assert!(v.range_to(1,1,BUDGET,During{bytes:&mut out,done:||{
            if third_link{fs::hard_link(&raw,&extra)}else{fs::set_permissions(&raw,fs::Permissions::from_mode(0o644))}
        }}).is_err());assert_eq!(out,b"A");out.clear();
        if third_link{fs::remove_file(extra).unwrap();}else{fs::set_permissions(&raw,fs::Permissions::from_mode(0o600)).unwrap();}
    }
    v.range_to(1,1,BUDGET,&mut out).unwrap();assert_eq!(out,b"A");
}

fn logical(n:u64,hex:&str)->super::super::logical::LogicalV1{
    let digest: [u8;32]=std::array::from_fn(|i|u8::from_str_radix(&hex[2*i..2*i+2],16).unwrap());
    super::super::logical::LogicalV1::declared(n,digest).unwrap()
}
#[test]
fn the_same_confirmed_content_survives_fragmentation_material_identity_and_storage_shape(){
    let tmp=PrivateTemp::under(&std::env::temp_dir());let stored=tmp.child("whole");let flat=tmp.child("flat");
    let a=material(&stored,8,b"abcdef",true);
    let desc=index(&stored,&Index::full(a.material).unwrap());let parent=HeldDir::open(&stored).unwrap();
    let v=LocalVersion::open(&parent,desc,|_|Ok(located(a,true))).unwrap();
    let expected=logical(6,"d29bab7cd23c8381a5ebfc91ab5f112eee616a8b2f7148804320e46522ae9d18");
    let only_dir=ReadBudget{file_descriptors:4,buffer_bytes:32};
    assert_eq!(v.whole_raw_commitment(only_dir).unwrap(),Some(expected));
    assert!(v.whole_raw_commitment(ReadBudget{file_descriptors:3,buffer_bytes:32}).is_err());
    assert!(v.whole_raw_commitment(ReadBudget{file_descriptors:4,buffer_bytes:31}).is_err());
    v.verify_complete(expected,BUDGET).unwrap();
    assert_ne!(a.directory_sha(),expected.digest()); // original material ID is in its directory commitment
    assert_ne!(crate::hash::digest(b"abcdef").unwrap(),expected.digest()); // not a flat SHA256
    let x=material(&flat,10,b"abc",false);let y=material(&flat,11,b"def",false);
    let mut pieces=Build::new(F).unwrap();
    pieces.add(Leaf::Data(x.material),0,3).unwrap();pieces.add(Leaf::Data(y.material),0,3).unwrap();
    let desc=index(&flat,&pieces.finish(6).unwrap());let f=HeldDir::open(&flat).unwrap();
    let v=LocalVersion::open(&f,desc,|m|Ok(located(if m==x.material{x}else{y},false))).unwrap();
    assert!(v.whole_raw_commitment(only_dir).unwrap().is_none());
    v.verify_complete(expected,BUDGET).unwrap();

    // Offset 1 inside an actual, larger FULL is not an exact complete RAW.
    let subset=tmp.child("offset");let bigger=material(&subset,12,b"!abcdef?",true);
    let mut part=Build::new(F).unwrap();part.add(Leaf::Data(bigger.material),1,6).unwrap();
    let desc=index(&subset,&part.finish(6).unwrap());let sub=HeldDir::open(&subset).unwrap();
    let v=LocalVersion::open(&sub,desc,|_|Ok(located(bigger,true))).unwrap();
    assert!(v.whole_raw_commitment(only_dir).unwrap().is_none());v.verify_complete(expected,BUDGET).unwrap();

    let cropped=tmp.child("cropped-and-extended");let origin=material(&cropped,19,b"abcdef",true);
    let map=Index::full(origin.material).unwrap().edit(0,None,3).unwrap().edit(0,None,6).unwrap();
    let desc=index(&cropped,&map);let held=HeldDir::open(&cropped).unwrap();
    let v=LocalVersion::open(&held,desc,|_|Ok(located(origin,true))).unwrap();
    assert!(v.whole_raw_commitment(only_dir).unwrap().is_none());assert!(v.verify_complete(expected,BUDGET).is_err());
    v.verify_complete(logical(6,"54fdd8139fe8941a8f920419142cc26930a59adfccc76695b3ddb8c2941d103d"),BUDGET).unwrap();
}

#[test]
fn a_mathematically_valid_raw_directory_never_proves_an_independent_physical_readback(){
    let t=PrivateTemp::under(&std::env::temp_dir());let folder=t.child("original");
    let m=material(&folder,26,b"abc",true);let desc=index(&folder,&Index::full(m.material).unwrap());
    let a=located(m,true).names().0;
    fs::write(path(&folder,&a),b"aBc").unwrap(); // same original raw inode, bytes corrupted BEFORE cold open
    let parent=HeldDir::open(&folder).unwrap();let v=LocalVersion::open(&parent,desc,|_|Ok(located(m,true))).unwrap();
    let c=logical(3,"a9f09c67fde300c38b8a22f833a1cbe149cebaef8634d94ab81e0eb4e0037b39");
    assert_eq!(v.whole_raw_commitment(BUDGET).unwrap(),Some(c));assert!(v.verify_complete(c,BUDGET).is_err());
    let empty=t.child("empty-map");let desc=index(&empty,&Build::new(F).unwrap().finish(0).unwrap());
    let held=HeldDir::open(&empty).unwrap();let v=LocalVersion::open(&held,desc,|_|panic!("empty map is not a named RAW")).unwrap();
    let no_data=ReadBudget{file_descriptors:0,buffer_bytes:0};assert!(v.whole_raw_commitment(no_data).unwrap().is_none());
    v.verify_complete(logical(0,"d68d1d5d43fb2740cba3b9e0ab1b519ee92339315e576758c7847c7153147f20"),no_data).unwrap();
}

#[test]
fn exact_block_eof_and_tail_are_independent_of_write_segmentation(){
    use super::super::logical::Whole;
    let zeros=[0u8;8192];let mut full=Whole::new(4*1024*1024).unwrap();let mut plus=Whole::new(4*1024*1024+1).unwrap();
    for _ in 0..512{full.write_all(&zeros).unwrap();plus.write_all(&zeros).unwrap();}
    plus.write_all(b"!").unwrap();
    assert_eq!(full.finish().unwrap(),logical(4194304,"8e7788d23a3fbef45efd39c3ab083a622c474b6b6c2d7a4f634387bb71a5f60d"));
    assert_eq!(plus.finish().unwrap(),logical(4194305,"dc5227d1cd9e764a4577923ca6d5f16640577ce620905ee7f6bbb93c3e8bdfc6"));
    let mut too_short=Whole::new(2).unwrap();too_short.write_all(b"x").unwrap();assert!(too_short.finish().is_err());
    let mut too_long=Whole::new(1).unwrap();assert!(too_long.write_all(b"xx").is_err());too_long.write_all(&[0]).unwrap();
    assert_eq!(too_long.finish().unwrap(),logical(1,"3c59a7155d48f0a51f49d3807825db76506352d0a5485823bfaa7eabe295bd89"));
}
