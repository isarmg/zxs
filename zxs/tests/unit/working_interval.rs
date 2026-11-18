use super::*;
use crate::{create_empty,current_info,commit_first,first_commit_receipt,initialize,read_current_to,read_node_to,save_first,
    DraftClaim,FormalNode,LengthPolicy,Role,VolumeId,DRAFT_CHUNK,test_support::PrivateTemp};
use std::{fs as osfs,io::Cursor,os::unix::fs::{FileExt,MetadataExt,PermissionsExt},path::PathBuf};

struct Fixtures{root:PrivateTemp,_recycle:PrivateTemp,system:PathBuf,data:PathBuf,volume:VolumeId,base:FormalNode}
fn claim(bytes:&[u8])->DraftClaim{DraftClaim::new(bytes.len() as u64,crate::hash::digest(bytes).unwrap()).unwrap()}
fn edit(offset:u64,bytes:&[u8],policy:LengthPolicy)->SpanEdit{SpanEdit::new(offset,claim(bytes),policy).unwrap()}
impl Fixtures{
    fn new(bytes:&[u8])->Self{
        let root=PrivateTemp::under(&std::env::temp_dir());let system=root.child("system");let data=root.child("data");
        let recycle=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));let r=recycle.child("recycle");
        let pool=initialize(OperationId::new().unwrap(),&system,&r,std::slice::from_ref(&data)).unwrap();
        let volume=pool.volumes.iter().find(|r|r.volume.role==Role::File).unwrap().id;
        let file=create_empty(&system,OperationId::new().unwrap(),volume,"original").unwrap();
        save_first(&system,OperationId::new().unwrap(),file.file,claim(bytes),Cursor::new(bytes)).unwrap();
        let base=commit_first(&system,OperationId::new().unwrap(),file.file,1).unwrap();Self{root,_recycle:recycle,system,data,volume,base}
    }
    fn dir(&self)->PathBuf{self.data.join("original")}
    fn raw(&self)->PathBuf{self.dir().join(format!("{}-{}",self.base.storage,self.base.creation_name))}
    fn extent(&self,op:OperationId)->PathBuf{self.dir().join(format!("working-{op}"))}
}
struct Never;
impl Read for Never{fn read(&mut self,_:&mut[u8])->io::Result<usize>{panic!("a retained original must not seek or re-consume input")}}

#[test]
fn checked_range_and_explicit_size_shape_including_a_hundred_gib_base(){
    let l=100u64*1024*1024*1024;let one=claim(b"a");
    assert_eq!(SpanEdit::new(l-1,one,LengthPolicy::KeepOrGrow).unwrap().result(l).unwrap(),l);
    assert_eq!(SpanEdit::new(l+16,one,LengthPolicy::KeepOrGrow).unwrap().result(l).unwrap(),l+17);
    assert!(SpanEdit::new(u64::MAX,one,LengthPolicy::KeepOrGrow).is_err());
    assert!(SpanEdit::new(8,one,LengthPolicy::Set(8)).is_err());
    assert!(SpanEdit::new(0,claim(b""),LengthPolicy::KeepOrGrow).is_err());
    assert!(SpanEdit::new(1,claim(b""),LengthPolicy::Set(10)).is_err());
    assert!(SpanEdit::new(0,claim(b""),LengthPolicy::Set(l)).unwrap().result(l).is_err());
}
#[test]
#[ignore="two explicitly selected private native filesystems"]
fn overwrite_append_zero_gap_resize_grow_and_truncate_choose_only_real_contributing_bytes(){
    for (at,extra,policy,expected) in [
        (2,b"XY".as_slice(),LengthPolicy::KeepOrGrow,b"abXYef".as_slice()),
        (6,b"g".as_slice(),LengthPolicy::KeepOrGrow,b"abcdefg".as_slice()),
        (8,b"q".as_slice(),LengthPolicy::KeepOrGrow,b"abcdef\0\0q".as_slice()),
        (2,b"XY".as_slice(),LengthPolicy::Set(4),b"abXY".as_slice()),
        (0,b"".as_slice(),LengthPolicy::Set(3),b"abc".as_slice()),
        (0,b"".as_slice(),LengthPolicy::Set(0),b"".as_slice()),
        (0,b"".as_slice(),LengthPolicy::Set(8),b"abcdef\0\0".as_slice()),
        (2,b"p".as_slice(),LengthPolicy::Set(8),b"abpdef\0\0".as_slice()),
    ]{
        let t=Fixtures::new(b"abcdef");let id=t.base.file;let op=OperationId::new().unwrap();let e=edit(at,extra,policy);
        let r=save_interval(&t.system,op,id,1,e,Cursor::new(extra)).unwrap();
        assert_eq!(r.revision,2);assert_eq!(r.base_node,t.base.node);assert_eq!(r.base_version,t.base.version);
        assert_eq!(r.logical_bytes,expected.len() as u64);assert_eq!(r.edit.input,claim(extra));
        let mut full=Vec::new();let status=read_current_to(&t.system,id,&mut full).unwrap();
        assert_eq!(status.edit,Some(r.clone()));assert_eq!(status.head,Some(t.base.clone()));assert_eq!(full,expected);
        for start in 0..=expected.len(){let mut got=Vec::new();read_current_range_to(&t.system,id,start as u64,(expected.len()-start) as u64,&mut got).unwrap();assert_eq!(got,&expected[start..]);}
        let mut original=Vec::new();read_node_to(&t.system,id,t.base.node,&mut original).unwrap();assert_eq!(original,b"abcdef");
        assert!(save_interval(&t.system,OperationId::new().unwrap(),id,2,e,Never).is_err());
        assert!(save_interval(&t.system,OperationId::new().unwrap(),id,1,e,Never).is_err());
        assert_eq!(save_interval(&t.system,op,id,1,e,Never).unwrap(),r);
    }
}
#[test]
#[ignore="two supplied private native filesystems"]
fn small_change_on_large_formal_base_does_not_clone_or_read_untouched_body_blocks(){
    let n=3*DRAFT_CHUNK+9;let base=vec![0x73;n];let t=Fixtures::new(&base);
    let id=t.base.file;let raw=t.raw();let before=osfs::metadata(&raw).unwrap();
    // Damage an unrelated leading block in this fixture. A real full scan
    // would detect it; a tail-only edit is not allowed to claim to have done one.
    let h=osfs::OpenOptions::new().write(true).open(&raw).unwrap();h.write_all_at(&[0x12],7).unwrap();h.sync_all().unwrap();drop(h);
    let after_damage=osfs::metadata(&raw).unwrap();
    let op=OperationId::new().unwrap();let change=edit((n-4) as u64,b"xy",LengthPolicy::KeepOrGrow);
    struct Forward<'a>{ctx:&'a Fixtures,r:Cursor<&'a[u8]>,first:bool}
    impl Read for Forward<'_>{fn read(&mut self,b:&mut[u8])->io::Result<usize>{
        if self.first{self.first=false;create_empty(&self.ctx.system,OperationId::new()?,self.ctx.volume,"sibling")?;}
        self.r.read(b)
    }}
    let r=save_interval(&t.system,op,id,1,change,Forward{ctx:&t,r:Cursor::new(b"xy".as_slice()),first:true}).unwrap();
    assert_eq!(r.logical_bytes,n as u64);assert_eq!(r.decision_sequence,5);
    let physical=osfs::metadata(t.extent(op)).unwrap();let end=osfs::metadata(&raw).unwrap();
    assert_eq!((end.ino(),end.len(),end.blocks()),(before.ino(),before.len(),before.blocks()));
    assert_eq!((end.mtime(),end.mtime_nsec()),(after_damage.mtime(),after_damage.mtime_nsec()));
    assert!(physical.len()<4096);assert!(physical.blocks()<before.blocks()/4);
    assert_eq!(osfs::read_dir(t.dir()).unwrap().count(),4);
    let mut out=Vec::new();read_current_range_to(&t.system,id,(n-6) as u64,6,&mut out).unwrap();assert_eq!(out,b"ssxyss");
    let mut damaged=Vec::new();assert!(read_current_range_to(&t.system,id,7,1,&mut damaged).is_err());assert!(damaged.is_empty());
    assert_eq!(first_commit_receipt(&t.system,t.base.created_by).unwrap().unwrap().published,Some(t.base.clone()));
}
#[test]
#[ignore="two private registered native filesystems"]
fn one_original_span_is_confirmed_once_and_reopened_at_every_publication_cut(){
    for stop in [Cut::Reservation,Cut::BeforeIntent,Cut::Intent,Cut::FrameSynced,Cut::MarkedBeforeSync,
        Cut::Decision,Cut::Installation,Cut::IntentReleased,Cut::Release]{
        let t=Fixtures::new(b"abcdef");let id=t.base.file;let op=OperationId::new().unwrap();let e=edit(1,b"!!",LengthPolicy::KeepOrGrow);
        save_inner(&t.system,op,id,1,e,Cursor::new(b"!!"),|c|if c==stop{Err(io::Error::other("interrupt original changed extent"))}else{Ok(())}).unwrap_err();
        let ack=matches!(stop,Cut::MarkedBeforeSync|Cut::Decision|Cut::Installation|Cut::IntentReleased|Cut::Release);
        let observed=interval_receipt(&t.system,op).unwrap().unwrap();assert_eq!(observed.published.is_some(),ack);
        let mut got=Vec::new();read_current_to(&t.system,id,&mut got).unwrap();
        assert_eq!(got,if ack{b"a!!def".as_slice()}else{b"abcdef".as_slice()});
        let r=if stop==Cut::Reservation{save_interval(&t.system,op,id,1,e,Cursor::new(b"!!")).unwrap()}
            else{save_interval(&t.system,op,id,1,e,Never).unwrap()};
        assert_eq!(r.decision_sequence,4);assert_eq!(r.revision,2);
        assert_eq!(osfs::read_dir(t.dir()).unwrap().count(),4);assert_eq!(osfs::read_dir(&t.system).unwrap().count(),1);
        let other=edit(1,b"xx",LengthPolicy::KeepOrGrow);assert!(save_interval(&t.system,op,id,1,other,Never).is_err());
        osfs::rename(&t.data,t.root.0.join("offline-root")).unwrap();t.root.child("data");
        assert!(current_info(&t.system,id).is_err());assert_eq!(save_interval(&t.system,op,id,1,e,Never).unwrap(),r);
    }
}
#[test]
#[ignore="two private registered native filesystems"]
fn absent_or_replaced_extent_never_rolls_current_back_to_history_and_wrong_input_never_gets_named(){
    let t=Fixtures::new(b"history");let file=t.base.file;let op=OperationId::new().unwrap();let e=edit(2,b"new",LengthPolicy::KeepOrGrow);
    let before=osfs::read(t.system.join("REGISTRATION")).unwrap();
    for possible in [b"n".as_slice(),b"abcd".as_slice(),b"bad".as_slice()]{
        assert!(save_interval(&t.system,op,file,1,e,Cursor::new(possible)).is_err());assert!(interval_receipt(&t.system,op).unwrap().is_none());
    }
    assert_eq!(before,osfs::read(t.system.join("REGISTRATION")).unwrap());assert!(!t.extent(op).exists());
    let receipt=save_interval(&t.system,op,file,1,e,Cursor::new(b"new")).unwrap();let original=t.extent(op);
    let data=osfs::read(&original).unwrap();osfs::rename(&original,t.root.0.join("held-old-extent")).unwrap();
    assert!(read_current_range_to(&t.system,file,0,1,io::sink()).is_err());
    let mut history=Vec::new();read_node_to(&t.system,file,t.base.node,&mut history).unwrap();assert_eq!(history,b"history");
    osfs::write(&original,&data).unwrap();osfs::set_permissions(&original,osfs::Permissions::from_mode(0o600)).unwrap();
    let mut current=Vec::new();assert!(read_current_to(&t.system,file,&mut current).is_err());assert!(current.is_empty());
    assert_eq!(save_interval(&t.system,op,file,1,e,Never).unwrap(),receipt);
}

#[test]
#[ignore="explicit manual 100 GiB source fixture on a test filesystem with room for original work and independent FULL; distinct private recycle filesystem"]
fn four_kib_overwrite_on_a_real_hundred_gib_formal_file_uses_only_one_changed_extent(){
    assert!(matches!(std::env::var("ZXS_ENABLE_100G_DRAFT_FIXTURE"),Ok(v) if v=="yes"));
    let large=std::env::var_os("ZXS_LARGE_FILE_TEST_TMPDIR").expect("test-only filesystem path required");
    let p=PrivateTemp::under(Path::new(&large));let system=p.child("system");let data=p.child("data");
    let r=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));let recycle=r.child("recycle");
    let pool=initialize(OperationId::new().unwrap(),&system,&recycle,std::slice::from_ref(&data)).unwrap();
    let vol=pool.volumes.iter().find(|r|r.volume.role==Role::File).unwrap().id;
    let file=create_empty(&system,OperationId::new().unwrap(),vol,"large").unwrap().file;
    let total=100u64*1024*1024*1024;
    struct Repeated(u64);
    impl Read for Repeated{fn read(&mut self,b:&mut[u8])->io::Result<usize>{
        let count=self.0.min(b.len() as u64) as usize;b[..count].fill(0x61);self.0-=count as u64;Ok(count)
    }}
    let mut hash=crate::hash::Sha256::new();let block=vec![0x61;DRAFT_CHUNK];let mut left=total;
    while left>0{let count=left.min(block.len() as u64) as usize;hash.update(&block[..count]).unwrap();left-=count as u64;}drop(block);
    save_first(&system,OperationId::new().unwrap(),file,DraftClaim::new(total,hash.finish()).unwrap(),Repeated(total)).unwrap();
    let base=commit_first(&system,OperationId::new().unwrap(),file,1).unwrap();
    let root=data.join("large");let raw=root.join(format!("{}-large",base.storage));let original=osfs::metadata(&raw).unwrap();
    let at=99u64*1024*1024*1024+19;let change=[0x70;4096];let op=OperationId::new().unwrap();
    save_interval(&system,op,file,1,edit(at,&change,LengthPolicy::KeepOrGrow),Cursor::new(&change[..])).unwrap();
    let now=osfs::metadata(&raw).unwrap();assert_eq!((now.len(),now.ino(),now.blocks()),(total,original.ino(),original.blocks()));
    let changed=osfs::metadata(root.join(format!("working-{op}"))).unwrap();
    assert!(changed.len()<8*1024);assert!(changed.blocks()*512<total/1024);
    let mut got=Vec::new();read_current_range_to(&system,file,at-1,4098,&mut got).unwrap();
    assert_eq!(got.len(),4098);assert_eq!(got[0],0x61);assert_eq!(got[4097],0x61);assert_eq!(&got[1..4097],&change[..]);
    let mut old=Vec::new();crate::read_node_range_to(&system,file,base.node,at,4096,&mut old).unwrap();assert_eq!(old,vec![0x61;4096]);
}
#[test]
#[ignore="two private native test filesystems"]
fn reverting_only_the_confirmed_span_frame_cannot_hide_it_or_rebind_its_original_operation(){
    let t=Fixtures::new(b"published formal");let file=t.base.file;let registry=t.system.join("REGISTRATION");
    let old=osfs::read(&registry).unwrap();let op=OperationId::new().unwrap();let e=edit(0,b"dirty",LengthPolicy::KeepOrGrow);
    save_interval(&t.system,op,file,1,e,Cursor::new(b"dirty")).unwrap();let extent=osfs::read(t.extent(op)).unwrap();
    osfs::write(&registry,old).unwrap();assert!(interval_receipt(&t.system,op).is_err());
    assert!(save_interval(&t.system,op,file,1,e,Never).is_err());assert!(read_current_range_to(&t.system,file,0,1,io::sink()).is_err());
    assert_eq!(osfs::read(t.extent(op)).unwrap(),extent);
}
#[test]
#[ignore="two explicit private native filesystems"]
fn an_equal_named_replacement_during_slow_input_does_not_authorize_an_extent_or_take_the_old_source(){
    let t=Fixtures::new(b"actual formal material");let file=t.base.file;let op=OperationId::new().unwrap();
    let original_reg=osfs::read(t.system.join("REGISTRATION")).unwrap();let path=t.raw();let displaced=t.root.0.join("actual-full");
    struct Replace<'a>{from:&'a Path,to:&'a Path,replaced:bool,input:Cursor<&'a[u8]>}
    impl Read for Replace<'_>{fn read(&mut self,b:&mut[u8])->io::Result<usize>{
        if !self.replaced{self.replaced=true;osfs::rename(self.from,self.to)?;osfs::write(self.from,osfs::read(self.to)?)?;
            osfs::set_permissions(self.from,osfs::Permissions::from_mode(0o600))?;}
        self.input.read(b)
    }}
    let e=edit(4,b"x",LengthPolicy::KeepOrGrow);
    assert!(save_interval(&t.system,op,file,1,e,Replace{from:&path,to:&displaced,replaced:false,input:Cursor::new(b"x".as_slice())}).is_err());
    assert!(interval_receipt(&t.system,op).unwrap().is_none());assert!(!t.extent(op).exists());
    assert_eq!(osfs::read(t.system.join("REGISTRATION")).unwrap(),original_reg);
    assert_eq!(osfs::read(path).unwrap(),b"actual formal material");assert_eq!(osfs::read(displaced).unwrap(),b"actual formal material");
}

#[test]
#[ignore="two explicit private native filesystems"]
fn an_unselected_original_changed_by_the_sink_never_confirms_a_composed_read(){
    use std::io::Write;
    struct Damage{victim:PathBuf,on_flush:bool,zero_only:bool,received:usize,changed:bool}
    impl Damage{fn change(&mut self)->io::Result<()>{
        if !self.changed{self.changed=true;osfs::OpenOptions::new().write(true).open(&self.victim)?.set_len(0)?;}Ok(())
    }}
    impl Write for Damage{
        fn write(&mut self,buf:&[u8])->io::Result<usize>{
            if self.zero_only{assert!(buf.iter().all(|x|*x==0));}
            self.received+=buf.len();if !self.on_flush{self.change()?;}Ok(buf.len())
        }
        fn flush(&mut self)->io::Result<()>{if self.on_flush{self.change()?;}Ok(())}
    }
    let patch=256*1024;
    for (at,len,base_victim,on_flush,zeros) in [
        (1,128*1024+1,false,false,true), // loss of extent during first of three zero buffers
        (0,1,false,false,false),          // actual base output, extent changed
        (patch,1,true,false,false),       // actual extent output, formal RAW changed
        (1,128*1024+1,false,true,true),   // all zero buffers delivered, but sink flush changes extent
    ]{
        let t=Fixtures::new(b"b");let id=t.base.file;let op=OperationId::new().unwrap();
        let r=save_interval(&t.system,op,id,1,edit(patch,b"x",LengthPolicy::KeepOrGrow),Cursor::new(b"x")).unwrap();
        let mut sink=Damage{victim:if base_victim{t.raw()}else{t.extent(op)},on_flush,zero_only:zeros,received:0,changed:false};
        assert!(read_current_range_to(&t.system,id,at,len,&mut sink).is_err());
        assert!(sink.changed);assert_eq!(sink.received,if zeros&&!on_flush{64*1024}else{len as usize});
        assert_eq!(interval_receipt(&t.system,op).unwrap().unwrap().published,Some(r));
    }
}
#[test]
#[ignore="two explicit private native filesystems"]
fn same_inode_and_length_do_not_authenticate_a_damaged_selected_extent_or_its_link(){
    for link in [false,true]{
        let t=Fixtures::new(b"history");let id=t.base.file;let op=OperationId::new().unwrap();
        save_interval(&t.system,op,id,1,edit(2,b"new",LengthPolicy::KeepOrGrow),Cursor::new(b"new")).unwrap();
        let path=t.extent(op);let before=osfs::metadata(&path).unwrap();
        let f=osfs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
        let off=if link{before.len()-1}else{before.len()-(3+32)};
        let mut byte=[0];f.read_exact_at(&mut byte,off).unwrap();byte[0]^=0x80;f.write_all_at(&byte,off).unwrap();f.sync_all().unwrap();
        let after=osfs::metadata(&path).unwrap();assert_eq!((after.ino(),after.len()),(before.ino(),before.len()));
        let mut selected=Vec::new();assert!(read_current_range_to(&t.system,id,2,1,&mut selected).is_err());assert!(selected.is_empty());
        let mut old=Vec::new();read_node_to(&t.system,id,t.base.node,&mut old).unwrap();assert_eq!(old,b"history");
    }
}
