use super::*;
use crate::{system::reader::{InitialReader,ordinary_pool,smb::SmbSession},test_support::PrivateTemp};
use std::{fs::OpenOptions,os::unix::fs::OpenOptionsExt};

fn ram_draft(eof:u64)->DraftStage{
    // SAFETY: fixed NUL-terminated name; a successful memfd_create
    // hands this fixture one fresh owned descriptor.
    let fd=unsafe{libc::memfd_create(c"zxs-stage-test".as_ptr(),libc::MFD_CLOEXEC)};
    assert!(fd>=0);
    let file=unsafe{File::from_raw_fd(fd)};
    file.set_permissions(Permissions::from_mode(0o600)).unwrap();
    let charge=Charge{amount:0,writer:Rc::new(Cell::new(0))};
    DraftStage::new(Scratch(Spool{file,charge,ram:true,len:0,poisoned:false}),eof)
}

#[test]
fn truncated_ram_prefix_flushes_the_same_inode_within_the_original_quota(){
    let temp=PrivateTemp::under(&std::env::temp_dir());
    let parent=HeldDir::open(&temp.0).unwrap();
    let mut stage=ram_draft(0);
    let chunk=vec![0x42;1024*1024];
    let size=chunk.len() as u64;
    for part in 0..24{
        stage.write(part*size,&chunk,||Ok(()),||unreachable!()).unwrap();
    }
    assert_eq!(stage.spool.charge.amount,24*size);
    let final_size=20*size;
    stage.resize(final_size,||Ok(())).unwrap();
    let inode=stage.spool.file.metadata().unwrap().ino();
    let quota=stage.spool.charge.writer.clone();
    let mut original=None;
    let failed=stage.flush(0,&parent,||unreachable!(),|batch|{
        let found=batch.input.metadata().unwrap();
        assert_eq!((found.ino(),found.len(),quota.get()),(inode,final_size,final_size));
        assert_eq!(batch.eof,final_size);
        let mut tail=[0];
        batch.input.read_exact_at(&mut tail,final_size-1).unwrap();
        assert_eq!(tail,[0x42]);
        original=Some(batch.op);
        Err(io::ErrorKind::WouldBlock.into())
    });
    assert_eq!(failed.unwrap_err().kind(),io::ErrorKind::WouldBlock);
    assert_eq!(stage.flush_op,original);
    assert_eq!((stage.spool.len,stage.live_bytes),(final_size,final_size));
    assert!(stage.flush(0,&parent,||unreachable!(),|batch|{
        assert_eq!(Some(batch.op),original);
        assert_eq!(batch.input.metadata().unwrap().ino(),inode);
        Ok(())
    }).unwrap());
    assert_eq!(stage.refresh_op,original);
    assert_eq!((stage.spool.len,stage.live_bytes,quota.get()),(0,0,0));
}

#[test]
fn staging_live_count_tracks_sparse_append_reuse_resize_and_rejected_write(){
    let temp=PrivateTemp::under(&std::env::temp_dir());
    let parent=HeldDir::open(&temp.0).unwrap();
    let mut stage=ram_draft(0);
    let live=|s:&DraftStage|(s.live_bytes,s.spool.len);
    for (at,data,expected) in [
        (0,&b"ab"[..],(2,2)),(8,&b"cd"[..],(4,4)),
        (1,&b"XY"[..],(5,6)),(16,&b"q"[..],(6,6)),
    ]{
        stage.write(at,data,||Ok(()),||unreachable!()).unwrap();
        assert_eq!(live(&stage),expected);
    }
    assert_eq!(stage.parts.last().unwrap().stored,1);
    stage.resize(9,||Ok(())).unwrap();
    assert_eq!(live(&stage),(4,6));
    let selection=stage.parts.clone();
    let rejected=||Err(io::ErrorKind::PermissionDenied.into());
    assert_eq!(stage.write(16,b"xyz",rejected,||unreachable!()).unwrap_err().kind(),io::ErrorKind::PermissionDenied);
    assert_eq!(live(&stage),(4,6));
    assert_eq!(stage.parts,selection);
    assert!(stage.flush(0,&parent,||Ok(u64::MAX),|batch|{
        let mut packed=[0;4];
        batch.input.read_exact_at(&mut packed,0).unwrap();
        assert_eq!((&packed,batch.eof),(b"aXYc",9));
        Ok(())
    }).unwrap());
    assert_eq!(live(&stage),(0,0));
}

#[test]
fn truncate_to_empty_keeps_the_effective_eof_change(){
    let temp=PrivateTemp::under(&std::env::temp_dir());
    let parent=HeldDir::open(&temp.0).unwrap();
    let mut stage=ram_draft(1);
    stage.write(0,b"ab",||Ok(()),||unreachable!()).unwrap();
    stage.resize(0,||Ok(())).unwrap();
    let inode=stage.spool.file.metadata().unwrap().ino();
    assert!(stage.flush(1,&parent,||unreachable!(),|batch|{
        let found=batch.input.metadata().unwrap();
        assert!(batch.spans.is_empty());
        assert_eq!((found.ino(),found.len(),batch.eof),(inode,0,0));
        Ok(())
    }).unwrap());
}

#[test]
fn packed_ram_and_original_are_charged_to_the_same_writer(){
    let writer=Rc::new(Cell::new(0));
    let mut input=Charge{amount:0,writer:writer.clone()};
    let mut packed=Charge{amount:0,writer:writer.clone()};
    input.reserve_to(RAM_HANDLE-2).unwrap();
    assert!(packed.reserve_to(3).is_err());packed.reserve_to(2).unwrap();
    assert_eq!(writer.get(),RAM_HANDLE);
    drop(packed);assert_eq!(writer.get(),RAM_HANDLE-2);
    drop(input);assert_eq!(writer.get(),0);
}

#[test]
#[ignore="source-only qualification on two actual ordinary leases and isolated SQL/files, no listener"]
fn one_original_writer_sees_private_ranges_and_authorizes_a_real_flush_once() {
    let temp=PrivateTemp::under(&std::env::temp_dir());let(p,pool,_f,rw,ro)=ordinary_pool(&temp);
    let source=InitialReader::open(&p,pool).unwrap();
    let session=SmbSession::fixture_authenticated(&source,"first.admin").unwrap();
    let write=session.connect(rw).unwrap();let read=session.connect(ro).unwrap();
    let name="staging.txt";let first=OperationId([0xf1;16]);
    let file=write.create_file(OperationId([0xf0;16]),name).unwrap().file;
    let mut input=OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(temp.0.join("packed-original")).unwrap();
    input.write_all(b"ABCDEFGH").unwrap();input.sync_all().unwrap();
    let w=write.open_writer(name,file).unwrap();
    w.save(first,None,Some(0),8,&input).unwrap();drop(w);
    let old=read.open_current(name).unwrap();
    let mut stage=Staged::open(write.open_writer(name,file).unwrap()).unwrap();
    let live=||Ok(());
    stage.write(2,b"1234",&live).unwrap();
    stage.write(3,b"x",&live).unwrap();
    stage.resize(6,&live).unwrap();stage.resize(9,&live).unwrap();stage.write(7,b"z",&live).unwrap();
    let high=stage.draft.spool.len;
    for _ in 0..16{stage.write(3,b"x",&live).unwrap();}
    assert!(stage.draft.spool.len<=high+1);
    let event=Cell::new(0);
    let stops_after_spool=||{
        let n=event.get()+1;event.set(n);
        if n==2{Err(io::ErrorKind::NotConnected.into())}else{Ok(())}
    };
    assert!(stage.write(0,b"!",&stops_after_spool).is_err());assert_eq!(event.get(),2);
    let overlay=stage.read_window(0,9,&live,|b|Ok(b.to_vec())).unwrap();
    assert_eq!(overlay,b"AB1x34\0z\0");
    assert_eq!(read.open_current(name).unwrap().working_original(),Some(first));
    assert_eq!(stage.base().working_original(),Some(first));
    let cut=Cell::new(0);
    let canceled_before_sql=||{
        let n=cut.get()+1;cut.set(n);
        if n==5{Err(io::ErrorKind::NotConnected.into())}else{Ok(())}
    };
    assert!(stage.flush(&canceled_before_sql).is_err());assert_eq!(cut.get(),5);
    assert_eq!(read.open_current(name).unwrap().working_original(),Some(first));
    let original=stage.draft.flush_op.unwrap();
    source.at(|db,_|{
        let q=db.prepare(c"SELECT state FROM main.operation WHERE op_id=?1 AND kind=10")?;
        q.bind_bytes(1,&original.0)?;
        assert!(crate::system::reader::work_record::start_one(&q)?);assert_eq!(q.byte(0)?,0);q.done()
    }).unwrap();
    assert!(stage.write(0,b"!",&live).is_err()); // do not change the same admitted original
    stage.flush(&live).unwrap();
    assert_eq!(stage.base().working_original(),Some(original));
    assert_eq!(stage.base().working_revision(),2);
    stage.flush(&live).unwrap();
    assert_eq!(stage.base().working_original(),Some(original));
    let mut held=Vec::new();old.range_to(0,8,&mut held).unwrap();assert_eq!(held,b"ABCDEFGH");
    let mut latest=Vec::new();read.open_current(name).unwrap().range_to(0,stage.eof(),&mut latest).unwrap();
    assert_eq!(latest,overlay);
}
