use super::*;
use crate::test_support::PrivateTemp;
use std::fs as stdfs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

#[test]
fn hash_and_id_contracts_reject_truncation_and_noncanonical_spelling(){
    let hex=|bytes:[u8;32]|bytes.iter().map(|b|format!("{b:02x}")).collect::<String>();
    assert_eq!(hex(crate::hash::digest(b"").unwrap()),"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    let mut h=crate::hash::Sha256::new();h.update(b"a").unwrap();h.update(b"bc").unwrap();
    assert_eq!(hex(h.finish()),"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    let long=b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
    assert_eq!(hex(crate::hash::digest(long).unwrap()),"248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
    assert!("00000000000000000000000000000000".parse::<OperationId>().is_err());
    assert!("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".parse::<OperationId>().is_err());
    assert!(Record::read(b"not an original ZXS registration".to_vec()).is_err());
    assert!(Record::read(vec![0u8;MAX_RECORD+1]).is_err());
}

#[test]
fn foreign_label_even_if_empty_held_locks_and_group_writes_never_grant_fresh_admission(){
    let t=PrivateTemp::under(&std::env::temp_dir());let path=t.child("selected");
    let root=HeldDir::open(&path).unwrap();
    root.claim(b"").unwrap();
    assert!(root.require_fresh().is_err());assert_eq!(root.label().unwrap(),Some(Vec::new()));
    let sub=path.join("nested");stdfs::DirBuilder::new().mode(0o700).create(&sub).unwrap();
    let held=HeldDir::open(&sub).unwrap();
    assert!(held.reject_registered_ancestors(&std::collections::BTreeSet::new()).is_err());
    root.exclusive().unwrap();assert!(HeldDir::open(&path).unwrap().share().is_err());
    stdfs::set_permissions(&sub,stdfs::Permissions::from_mode(0o770)).unwrap();
    assert!(HeldDir::open(&sub).is_err());
}

#[test]
#[ignore="requires writable ZXS_DISTINCT_TEST_TMPDIR on a separate test filesystem, with user xattrs and SYSTEM O_TMPFILE"]
fn same_registration_alone_recovers_all_real_boundaries_and_cold_reopens_without_rewriting_data(){
    for stop in [InitStep::Reservation,InitStep::Label(0),InitStep::Label(1),
        InitStep::Label(2),InitStep::BeforePublication,InitStep::RenamedBeforeSync,InitStep::Publication] {
        let a=PrivateTemp::under(&std::env::temp_dir());
        let b=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
        assert_ne!(a.1.device,b.1.device,"requires distinct test filesystems");
        let sys=a.child("system");let data=a.child("ordinary");let bin=b.child("recycle");
        let files=std::slice::from_ref(&data);
        let op=OperationId::new().unwrap();
        let e=initialize_inner(op,&sys,&bin,files,|p|if p==stop{Err(io::Error::other("test interruption"))}else{Ok(())}).unwrap_err();
        assert_eq!(e.to_string(),"test interruption");
        if stop==InitStep::RenamedBeforeSync{
            let error=inspect_with_sync(&sys,|_|Err(io::Error::other("original directory sync failed"))).unwrap_err();
            assert_eq!(error.to_string(),"original directory sync failed");
            // Only an observer which finishes the original directory sync can succeed.
            assert_eq!(inspect(&sys).unwrap().initialized_by,op);
        }
        if stop==InitStep::Reservation{
            let pending=sys.join("INITIALIZING");
            let bytes=stdfs::read(&pending).unwrap();
            let parsed=Record::read(bytes.clone()).unwrap();
            let data_birth=fs::Birth::read(&HeldDir::open(&data).unwrap().file).unwrap();
            for required in [&data_birth,&parsed.origin_birth]{
                // Model reuse of the identical path, device and inode, all of which
                // remain in the actual reservation. Only the original persistent
                // handle differs, and even the checksum is valid.
                let mut other=bytes.clone();
                let end=other.len()-32;
                let hits=other[..end].windows(required.bytes().len()).enumerate()
                    .filter_map(|(i,v)|(v==required.bytes()).then_some(i)).collect::<Vec<_>>();
                assert_eq!(hits.len(),1);
                other[hits[0]+required.bytes().len()-1]^=1;
                let digest=crate::hash::digest(&other[..end]).unwrap();
                other[end..].copy_from_slice(&digest);
                stdfs::write(&pending,&other).unwrap();
                assert!(initialize(op,&sys,&bin,files).is_err());
                for root in [&sys,&bin,&data]{assert!(HeldDir::open(root).unwrap().label().unwrap().is_none());}
                assert_eq!(stdfs::read(&pending).unwrap(),other);
            }
            stdfs::write(&pending,&bytes).unwrap();
        }
        let before:Vec<_>=[&sys,&bin,&data].map(|p|HeldDir::open(p).unwrap().label().unwrap()).to_vec();
        assert!(initialize(OperationId::new().unwrap(),&sys,&bin,files).is_err());
        assert_eq!(before,[&sys,&bin,&data].map(|p|HeldDir::open(p).unwrap().label().unwrap()));
        let info=initialize(op,&sys,&bin,files).unwrap();
        assert_eq!(info.initialized_by,op);
        assert_eq!(inspect(&sys).unwrap(),info);
        assert_eq!(stdfs::read_dir(&data).unwrap().count(),0);
        assert_eq!(stdfs::read_dir(&bin).unwrap().count(),0);
        assert_eq!(stdfs::read_dir(&sys).unwrap().count(),1);
        stdfs::write(data.join("unchanged"),b"existing bytes").unwrap();
        assert_eq!(initialize(op,&sys,&bin,files).unwrap(),info);
        assert_eq!(stdfs::read(data.join("unchanged")).unwrap(),b"existing bytes");
        // Historical output is a receipt, not a new permission to write.
        let moved=a.0.join("offline-original");stdfs::rename(&data,&moved).unwrap();
        assert_eq!(initialize(op,&sys,&bin,files).unwrap(),info);
        assert!(inspect(&sys).is_err());
        a.child("ordinary");
        assert_eq!(initialize(op,&sys,&bin,files).unwrap(),info);
        assert!(inspect(&sys).is_err());
        assert!(HeldDir::open(&data).unwrap().label().unwrap().is_none());
        assert!(initialize(op,&sys,&bin,std::slice::from_ref(&moved)).is_err());
        assert_eq!(stdfs::read(moved.join("unchanged")).unwrap(),b"existing bytes");

        // Equal bytes on a different, equally private inode do not recreate the decision.
        let original=stdfs::read(sys.join("REGISTRATION")).unwrap();
        let named=sys.join("REGISTRATION");let old=sys.join("retained-for-test");
        stdfs::rename(&named,&old).unwrap();
        stdfs::write(&named,&original).unwrap();
        stdfs::set_permissions(&named,stdfs::Permissions::from_mode(0o600)).unwrap();
        assert!(initialize(op,&sys,&bin,files).is_err());
        assert!(inspect(&sys).is_err());
        stdfs::remove_file(&named).unwrap();stdfs::rename(&old,&named).unwrap();
        assert_eq!(initialize(op,&sys,&bin,files).unwrap(),info);
        let mut replaced=original.clone();
        let pos=super::format::MAGIC.len(); replaced[pos]^=1;
        let end=replaced.len()-32;let new=crate::hash::digest(&replaced[..end]).unwrap();
        replaced[end..].copy_from_slice(&new);stdfs::write(sys.join("REGISTRATION"),&replaced).unwrap();
        assert!(inspect(&sys).is_err(),"self-consistent changed catalog lacks original SYSTEM label");
        assert!(initialize(op,&sys,&bin,files).is_err());
        assert_eq!(stdfs::read(sys.join("REGISTRATION")).unwrap(),replaced);
    }
}

#[test]
fn private_record_probe_rejects_fifo_directory_and_symlink_before_content_open(){
    use std::ffi::CString;
    use std::os::{raw::{c_char,c_int,c_uint},unix::{ffi::OsStrExt,fs::symlink}};
    unsafe extern "C" { fn mkfifo(path:*const c_char, mode:c_uint)->c_int; }
    let tmp=PrivateTemp::under(&std::env::temp_dir());
    let sys=tmp.child("system");let held=HeldDir::open(&sys).unwrap();
    let sentinel=tmp.0.join("sentinel");stdfs::write(&sentinel,b"leave this alone").unwrap();
    symlink(&sentinel,sys.join("link")).unwrap();
    stdfs::create_dir(sys.join("dir")).unwrap();
    let named=CString::new(sys.join("pipe").as_os_str().as_bytes()).unwrap();
    // SAFETY: private fixture path is NUL terminated and lives through the call.
    assert_eq!(unsafe{mkfifo(named.as_ptr(),0o600)},0);
    for name in [c"pipe",c"link",c"dir"]{assert!(held.record(name,MAX_RECORD).is_err());}
    assert_eq!(stdfs::read(&sentinel).unwrap(),b"leave this alone");
}

#[test]
#[ignore="native private FILE and dedicated volumes: source-only new genesis and old mark refusal"]
fn old_raw_name_genesis_and_volume_labels_cannot_authorize_the_new_leaf_layout(){
    use std::os::unix::ffi::OsStrExt;
    let t=PrivateTemp::under(&std::env::temp_dir());let r=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=t.child("system");let a=t.child("ordinary");let bin=r.child("recycle");let op=OperationId::new().unwrap();
    let f=initialize(op,&sys,&bin,std::slice::from_ref(&a)).unwrap();let path=sys.join("REGISTRATION");
    let bytes=stdfs::read(&path).unwrap();let rec=Record::read(bytes.clone()).unwrap();
    let emit=|out:&mut Vec<u8>,birth:&fs::Birth|{out.push(birth.bytes().len()as u8);out.extend_from_slice(birth.bytes());};
    // Reconstruct the prior single genesis with these actual identity bytes,
    // without a prior-format reader or writer in the product.
    let mut old=b"ZXS/FILE-POOL\0".to_vec();old.extend_from_slice(&f.id.0);old.extend_from_slice(&op.0);
    old.extend_from_slice(&rec.actor_uid.to_be_bytes());old.extend_from_slice(&rec.origin.device.to_be_bytes());old.extend_from_slice(&rec.origin.inode.to_be_bytes());
    emit(&mut old,&rec.origin_birth);old.extend_from_slice(&(f.volumes.len()as u16).to_be_bytes());
    for(i,v)in f.volumes.iter().enumerate(){
        old.push(match v.volume.role{Role::System=>0,Role::Recycle=>1,Role::File=>2});old.extend_from_slice(&v.id.0);
        let(e,b)=rec.original_root(i);old.extend_from_slice(&e.device.to_be_bytes());old.extend_from_slice(&e.inode.to_be_bytes());emit(&mut old,b);
        let name=v.volume.path.as_os_str().as_bytes();old.extend_from_slice(&(name.len()as u16).to_be_bytes());old.extend_from_slice(name);
        assert_eq!(HeldDir::open(&v.volume.path).unwrap().label().unwrap().unwrap(),rec.label(i));
    }
    let digest=crate::hash::digest(&old).unwrap();old.extend_from_slice(&digest);
    assert!(Record::read(old.clone()).is_err());stdfs::write(&path,&old).unwrap();
    assert!(inspect(&sys).is_err());assert!(initialize(op,&sys,&bin,std::slice::from_ref(&a)).is_err());
    assert_eq!(stdfs::read(&path).unwrap(),old);assert!(stdfs::read_dir(&a).unwrap().next().is_none());
    // An old, otherwise empty labelled volume must not bootstrap or migrate.
    let sys2=t.child("other-system");let a2=t.child("other-file");let bin2=r.child("other-recycle");
    let old_r=HeldDir::open(&bin2).unwrap();let mut label=b"ZXS/VOLUME\0".to_vec();label.push(1);
    label.extend_from_slice(&f.id.0);label.extend_from_slice(&f.volumes[1].id.0);label.extend_from_slice(&op.0);old_r.claim(&label).unwrap();
    assert!(initialize(OperationId::new().unwrap(),&sys2,&bin2,&[a2]).is_err());assert_eq!(old_r.label().unwrap(),Some(label));
    assert!(stdfs::read_dir(&sys2).unwrap().next().is_none());
}
