use super::*;
use crate::{create_empty,initialize,inspect,read_initial_file,test_support::PrivateTemp,Role,VolumeId};
use std::{fs as osfs,io::Cursor,os::unix::fs::{FileExt,PermissionsExt},path::PathBuf};

struct Fixture{_one:PrivateTemp,_two:PrivateTemp,sys:PathBuf,data:PathBuf,volume:VolumeId,file:FileInfo}
impl Fixture{
    fn new()->Self{Self::under(&std::env::temp_dir())}
    fn under(primary:&Path)->Self{
        let one=PrivateTemp::under(primary);
        let two=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
        let sys=one.child("system");let data=one.child("data");let bin=two.child("recycle");
        let pool=initialize(OperationId::new().unwrap(),&sys,&bin,std::slice::from_ref(&data)).unwrap();
        let volume=pool.volumes.iter().find(|r|r.volume.role==Role::File).unwrap().id;
        let file=create_empty(&sys,OperationId::new().unwrap(),volume,"first").unwrap();
        Self{_one:one,_two:two,sys,data,volume,file}
    }
}
struct NeverRead;
impl Read for NeverRead{fn read(&mut self,_:&mut[u8])->io::Result<usize>{panic!("receipt or retained working material must not consume input")}}
fn claim(bytes:&[u8])->DraftClaim{DraftClaim::new(bytes.len() as u64,crate::hash::digest(bytes).unwrap()).unwrap()}

#[test]
fn four_mib_is_a_chunk_not_a_100_gib_file_limit(){
    let size=100u64*1024*1024*1024;
    let encoded=material::physical_size(size).unwrap();
    assert!(encoded>size);assert!(encoded-size<1024*1024);
    assert!(DraftClaim::new(size,[11;32]).is_ok());assert!(DraftClaim::new(u64::MAX,[11;32]).is_err());
}

#[test]
#[ignore="two private explicitly supplied native filesystems"]
fn one_original_first_save_reopens_at_every_cut_without_reading_the_caller_twice(){
    for stop in [Cut::Reservation,Cut::BeforeIntent,Cut::Intent,Cut::FrameSynced,Cut::MarkedBeforeSync,
        Cut::Decision,Cut::Installation,Cut::IntentReleased,Cut::Release]{
        let t=Fixture::new();let op=OperationId::new().unwrap();let data=b"confirmed working draft, not a formal node";
        let expected=claim(data);
        save_inner(&t.sys,op,t.file.file,expected,Cursor::new(data),|cut|if cut==stop{Err(io::Error::other("interrupted original first SAVE"))}else{Ok(())}).unwrap_err();
        let observed=first_save_receipt(&t.sys,op).unwrap().unwrap();
        assert_eq!(observed.file,t.file.file);assert_eq!(observed.body,expected);
        let mut old=Vec::new();let view=read_current_to(&t.sys,t.file.file,&mut old).unwrap();
        let ack=matches!(stop,Cut::MarkedBeforeSync|Cut::Decision|Cut::Installation|Cut::IntentReleased|Cut::Release);
        assert_eq!(view.working.is_some(),ack);
        if ack{assert_eq!(old.as_slice(),data);}else{assert!(old.is_empty());}
        if stop==Cut::FrameSynced{assert_eq!(observed.status,CreationStatus::DecisionAwaitingConfirmation);}
        let r=if stop==Cut::Reservation{save_first(&t.sys,op,t.file.file,expected,Cursor::new(data)).unwrap()}
            else{save_first(&t.sys,op,t.file.file,expected,NeverRead).unwrap()};
        assert_eq!(r.revision,1);assert_eq!(r.decision_sequence,2);assert_eq!(r.file,t.file.file);
        assert_eq!(r,save_first(&t.sys,op,t.file.file,expected,NeverRead).unwrap());
        assert!(save_first(&t.sys,op,t.file.file,claim(b"wrong"),NeverRead).is_err());
        assert!(save_first(&t.sys,OperationId::new().unwrap(),t.file.file,expected,NeverRead).is_err());
        assert!(read_initial_file(&t.sys,t.file.file).is_err());
        let mut bytes=Vec::new();let current=read_current_to(&t.sys,t.file.file,&mut bytes).unwrap();
        assert_eq!(current.working.as_ref(),Some(&r));assert_eq!(bytes.as_slice(),data);assert_eq!(current.file,t.file);
        assert_eq!(osfs::read_dir(t.data.join("first")).unwrap().count(),2);assert_eq!(osfs::read_dir(&t.sys).unwrap().count(),1);
    }
}

#[test]
#[ignore="two private actual native test filesystems"]
fn invalid_short_trailing_and_bad_streams_never_leave_named_work_or_an_original_reservation(){
    let t=Fixture::new();let claim=claim(b"abc");let before=osfs::read(t.sys.join("REGISTRATION")).unwrap();
    for bad in [b"ab".as_slice(),b"abcd".as_slice(),b"abd".as_slice()]{
        let op=OperationId::new().unwrap();assert!(save_first(&t.sys,op,t.file.file,claim,Cursor::new(bad)).is_err());
        assert!(first_save_receipt(&t.sys,op).unwrap().is_none());
        assert_eq!(osfs::read_dir(t.data.join("first")).unwrap().count(),1);
        assert_eq!(osfs::read(t.sys.join("REGISTRATION")).unwrap(),before);
    }
}

struct OnePass<'a>{once:bool,t:&'a Fixture,inner:Cursor<Vec<u8>>}
impl Read for OnePass<'_>{
    fn read(&mut self,b:&mut[u8])->io::Result<usize>{
        if !self.once{
            self.once=true;
            // A distinct normal FILE on the same root is genuinely writable
            // while this nonseekable stream is feeding the first draft.
            create_empty(&self.t.sys,OperationId::new()?,self.t.volume,"sibling")?;
        }
        self.inner.read(b)
    }
}
#[test]
#[ignore="actual multi-chunk saved material on two private test filesystems"]
fn closed_original_chunks_are_pinned_before_output_and_corrupt_later_bytes_are_not_returned(){
    let t=Fixture::new();let op=OperationId::new().unwrap();
    let mut data=vec![0x61;crate::DRAFT_CHUNK];data.extend_from_slice(b"verified suffix");
    let claim=claim(&data);let reader=OnePass{once:false,t:&t,inner:Cursor::new(data.clone())};
    let saved=save_first(&t.sys,op,t.file.file,claim,reader).unwrap();assert_eq!(saved.decision_sequence,3);
    let mut got=Vec::new();read_current_to(&t.sys,t.file.file,&mut got).unwrap();assert_eq!(got,data);
    let leaf=t.data.join("first").join(format!("working-{op}"));let saved_bytes=osfs::read(&leaf).unwrap();
    let raw=osfs::OpenOptions::new().write(true).open(&leaf).unwrap();
    raw.write_all_at(&[0],raw.metadata().unwrap().len()-33).unwrap();raw.sync_all().unwrap();drop(raw);
    let mut verified=Vec::new();assert!(read_current_to(&t.sys,t.file.file,&mut verified).is_err());
    assert_eq!(verified.as_slice(),&data[..crate::DRAFT_CHUNK]);
    // Replacing the entire named inode with equal valid bytes is not a repair.
    osfs::rename(&leaf,t._one.0.join("actual")).unwrap();osfs::write(&leaf,&saved_bytes).unwrap();
    osfs::set_permissions(&leaf,osfs::Permissions::from_mode(0o600)).unwrap();
    let mut none=Vec::new();assert!(read_current_to(&t.sys,t.file.file,&mut none).is_err());assert!(none.is_empty());
    assert_eq!(save_first(&t.sys,op,t.file.file,claim,NeverRead).unwrap(),saved);
    let offline=t._one.0.join("offline");osfs::rename(&t.data,&offline).unwrap();t._one.child("data");
    assert!(inspect(&t.sys).is_err());assert!(current_info(&t.sys,t.file.file).is_err());
    assert_eq!(save_first(&t.sys,op,t.file.file,claim,NeverRead).unwrap(),saved);
}

#[test]
#[ignore="two explicitly owned native filesystems"]
fn a_whole_confirmed_save_cannot_be_dropped_back_to_its_still_valid_creation_prefix(){
    let t=Fixture::new();let op=OperationId::new().unwrap();
    let register=t.sys.join("REGISTRATION");let birth=osfs::read(&register).unwrap();
    let bytes=b"keep this working material";let a=claim(bytes);
    save_first(&t.sys,op,t.file.file,a,Cursor::new(bytes)).unwrap();
    let physical=t.data.join("first").join(format!("working-{op}"));
    let saved=osfs::read(&physical).unwrap();osfs::write(&register,&birth).unwrap();
    assert!(first_save_receipt(&t.sys,op).is_err());assert!(save_first(&t.sys,op,t.file.file,a,NeverRead).is_err());
    assert!(read_current_to(&t.sys,t.file.file,io::sink()).is_err());
    assert_eq!(osfs::read(&register).unwrap(),birth);assert_eq!(osfs::read(&physical).unwrap(),saved);
}

#[test]
#[ignore="manual opt-in: 100 GiB plus working overhead on ZXS_LARGE_FILE_TEST_TMPDIR and a distinct second private test filesystem"]
fn one_hundred_gib_first_save_and_cold_read_use_forward_only_bounded_buffers(){
    assert!(matches!(std::env::var("ZXS_ENABLE_100G_DRAFT_FIXTURE"),Ok(s) if s=="yes"));
    let primary=std::env::var_os("ZXS_LARGE_FILE_TEST_TMPDIR").expect("explicit dedicated test filesystem required");
    let t=Fixture::under(Path::new(&primary));let op=OperationId::new().unwrap();let length=100u64*1024*1024*1024;
    struct Repeated(u64);
    impl Read for Repeated{fn read(&mut self,b:&mut[u8])->io::Result<usize>{
        let n=self.0.min(b.len() as u64) as usize;b[..n].fill(0x6d);self.0-=n as u64;Ok(n)
    }}
    struct Count(u64);
    impl Write for Count{fn write(&mut self,b:&[u8])->io::Result<usize>{
        if b.iter().any(|v|*v!=0x6d){return Err(io::Error::other("unexpected restored large-file data"));}
        self.0+=b.len() as u64;Ok(b.len())
    }fn flush(&mut self)->io::Result<()>{Ok(())}}
    let mut h=crate::hash::Sha256::new();let block=vec![0x6d;crate::DRAFT_CHUNK];let mut left=length;
    while left!=0{let n=left.min(block.len() as u64) as usize;h.update(&block[..n]).unwrap();left-=n as u64;}drop(block);
    let wanted=DraftClaim::new(length,h.finish()).unwrap();let r=save_first(&t.sys,op,t.file.file,wanted,Repeated(length)).unwrap();
    assert_eq!(r.body,wanted);assert_eq!(r.revision,1);
    let mut count=Count(0);let actual=read_current_to(&t.sys,t.file.file,&mut count).unwrap();
    assert_eq!(actual.working,Some(r));assert_eq!(count.0,length);
}

#[test]
#[ignore="two private native test filesystems"]
fn zero_length_confirmed_revision_is_not_a_never_saved_file_or_a_formal_version(){
    let t=Fixture::new();let id=t.file.file;
    assert!(current_info(&t.sys,id).unwrap().working.is_none());
    let expected=claim(b"");let op=OperationId::new().unwrap();
    let first=save_first(&t.sys,op,id,expected,Cursor::new(b"")).unwrap();
    assert_eq!(first.revision,1);assert_eq!(first.body,expected);
    let mut empty=Vec::new();let current=read_current_to(&t.sys,id,&mut empty).unwrap();
    assert!(empty.is_empty());assert_eq!(current.working,Some(first.clone()));
    assert_eq!(save_first(&t.sys,op,id,expected,NeverRead).unwrap(),first);
    assert!(read_initial_file(&t.sys,id).is_err());
}

#[test]
#[ignore="two explicitly owned private test filesystems"]
fn replacing_the_selected_container_during_forward_input_cannot_retarget_the_save(){
    let t=Fixture::new();let existing=t.data.join("first");let old=t._one.0.join("pinned-original");
    let before=osfs::read(t.sys.join("REGISTRATION")).unwrap();
    let op=OperationId::new().unwrap();let contents=b"only for the selected native FILE";
    struct Substitute<'a>{from:&'a Path,to:&'a Path,bytes:Cursor<&'a[u8]>,done:bool}
    impl Read for Substitute<'_>{fn read(&mut self,buf:&mut[u8])->io::Result<usize>{
        if !self.done{
            self.done=true;osfs::rename(self.from,self.to)?;osfs::create_dir(self.from)?;
            osfs::set_permissions(self.from,osfs::Permissions::from_mode(0o700))?;
            osfs::write(self.from.join("FILE"),osfs::read(self.to.join("FILE"))?)?;
            osfs::set_permissions(self.from.join("FILE"),osfs::Permissions::from_mode(0o600))?;
        }
        self.bytes.read(buf)
    }}
    let source=Substitute{from:&existing,to:&old,bytes:Cursor::new(contents.as_slice()),done:false};
    assert!(save_first(&t.sys,op,t.file.file,claim(contents),source).is_err());
    assert!(first_save_receipt(&t.sys,op).unwrap().is_none());
    assert_eq!(osfs::read(t.sys.join("REGISTRATION")).unwrap(),before);
    for dir in [&existing,&old]{assert_eq!(osfs::read_dir(dir).unwrap().count(),1);assert!(dir.join("FILE").is_file());}
}

#[test]
#[ignore="two private explicitly owned native filesystems"]
fn an_unlinked_reserved_retry_keeps_sibling_reads_but_refuses_new_writes_until_original_finishes(){
    let t=Fixture::new();
    let other=create_empty(&t.sys,OperationId::new().unwrap(),t.volume,"sibling").unwrap();
    let original=OperationId::new().unwrap();let new_item=OperationId::new().unwrap();let sibling_save=OperationId::new().unwrap();
    let bytes=b"original reserved input";let wanted=claim(bytes);let sibling_wants=claim(b"sibling");
    save_inner(&t.sys,original,t.file.file,wanted,Cursor::new(bytes),|phase|{
        if phase==Cut::Reservation{Err(io::Error::other("lost private unnamed input"))}else{Ok(())}
    }).unwrap_err();
    assert_eq!(first_save_receipt(&t.sys,original).unwrap().unwrap().status,CreationStatus::Reserved);
    struct Waiting<'a>{fixture:&'a Fixture,other:FileId,create:OperationId,save:OperationId,other_wants:DraftClaim,
        bytes:Cursor<&'a[u8]>,checked:bool}
    impl Read for Waiting<'_>{fn read(&mut self,dst:&mut[u8])->io::Result<usize>{
        if !self.checked{
            self.checked=true;
            let mut old=Vec::new();let sibling=read_current_to(&self.fixture.sys,self.other,&mut old)?;
            assert!(sibling.working.is_none()&&old.is_empty());
            assert!(create_empty(&self.fixture.sys,self.create,self.fixture.volume,"later").is_err());
            assert!(save_first(&self.fixture.sys,self.save,self.other,self.other_wants,NeverRead).is_err());
            assert!(crate::creation_receipt(&self.fixture.sys,self.create)?.is_none());
            assert!(first_save_receipt(&self.fixture.sys,self.save)?.is_none());
        }
        self.bytes.read(dst)
    }}
    let twice=Waiting{fixture:&t,other:other.file,create:new_item,save:sibling_save,other_wants:sibling_wants,
        bytes:Cursor::new(bytes.as_slice()),checked:false};
    let original_result=save_first(&t.sys,original,t.file.file,wanted,twice).unwrap();
    assert_eq!(original_result.decision_sequence,3);
    create_empty(&t.sys,new_item,t.volume,"later").unwrap();
    let sibling_result=save_first(&t.sys,sibling_save,other.file,sibling_wants,Cursor::new(b"sibling")).unwrap();
    assert_eq!(sibling_result.decision_sequence,5);assert_eq!(sibling_result.file,other.file);
}

#[test]
#[ignore="two explicit registered native filesystems"]
fn a_slow_named_save_recovery_verifier_holds_the_same_work_lease_as_an_open_reader(){
    let t=Fixture::new();let file=t.file.file;let input=b"old named preparation still being authenticated";
    let expected=claim(input);let save=OperationId::new().unwrap();
    save_inner(&t.sys,save,file,expected,Cursor::new(input),|s|if s==Cut::BeforeIntent{
        Err(io::Error::other("leave the exact named working preparation"))
    }else{Ok(())}).unwrap_err();
    let pending={
        let (pool,_,state)=LocalPool::with_snapshot(&t.sys,Access::Read).unwrap();
        let a=state.saving.as_ref().unwrap();let c=state.file(file).unwrap();let volume=pool.ordinary(t.volume).unwrap();
        let original=source_dir(&volume,c,&state,&a.original).unwrap();PendingReader::open(&original,a).unwrap().unwrap()
    };
    let r=save_first(&t.sys,save,file,expected,NeverRead).unwrap();
    assert_eq!(r.revision,1);
    let op=OperationId::new().unwrap();let path=t.data.join("first").join(format!("working-{save}"));
    assert!(crate::commit_first(&t.sys,op,file,1).is_err());
    let receipt=crate::first_commit_receipt(&t.sys,op).unwrap().unwrap();
    assert_eq!(receipt.status,CreationStatus::PublishedInstallationUnconfirmed);let formal=receipt.published.unwrap();
    assert!(path.is_file());
    let verified=pending.verify().unwrap(); // same named inode, still open throughout verification
    assert!(crate::commit_first(&t.sys,op,file,1).is_err());assert!(path.is_file());
    drop(verified);
    assert_eq!(crate::commit_first(&t.sys,op,file,1).unwrap(),formal);assert!(!path.exists());
    let mut got=Vec::new();crate::read_node_to(&t.sys,file,formal.node,&mut got).unwrap();assert_eq!(got,input);
}
