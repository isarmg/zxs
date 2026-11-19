use super::*;
use rustix::{fd::AsFd,net::{SendAncillaryBuffer,SendAncillaryMessage,SendFlags,sendmsg}};
use std::{io::{IoSlice,Write},os::unix::fs::{OpenOptionsExt,PermissionsExt},path::Path};
fn fixture()->(tempfile::TempDir,File,u32,u32,Vec<u8>){
    let d=tempfile::tempdir().unwrap();std::fs::set_permissions(d.path(),std::fs::Permissions::from_mode(0o700)).unwrap();
    let p=File::open(d.path()).unwrap();new_store(d.path());
    let mut b=b"ZXS-SAM/NEW-1\0ZXSLOCAL".to_vec();b.extend([1,4,0,0,0,0,0,5,21,0,0,0,1,0,0,0,2,0,0,0,3,0,0,0]);
    (d,p,rustix::process::getuid().as_raw(),rustix::process::getgid().as_raw(),b)
}
fn new_store(p:&Path){
    let mut f=File::options().write(true).create_new(true).mode(0o600).open(p.join("secrets.tdb")).unwrap();
    f.write_all(b"public test fixture").unwrap();
}
fn send(tx:&File,b:&[u8],fs:&[&File]){
    let fds=fs.iter().map(|f|f.as_fd()).collect::<Vec<_>>();
    let mut buf=[MaybeUninit::uninit();rustix::cmsg_space!(ScmRights(3))];
    let mut a=SendAncillaryBuffer::new(&mut buf);
    if !fds.is_empty(){assert!(a.push(SendAncillaryMessage::ScmRights(&fds)));}
    assert_eq!(sendmsg(tx,&[IoSlice::new(b)],&mut a,SendFlags::NOSIGNAL).unwrap(),b.len());
}
#[test]
fn an_inflight_descriptor_keeps_the_actual_regular_inode_after_the_sender_closes(){
    let (d,p,uid,gid,b)=fixture();let (rx,tx)=OriginChannel::open().unwrap();
    let pin=fixed_name(&p).unwrap();send(&tx,&b,&[&pin]);drop(pin);drop(tx);
    let held=rx.receive(&p,uid,gid,rustix::process::getpid()).unwrap();held.recheck(&p).unwrap();
    std::fs::rename(d.path().join("secrets.tdb"),d.path().join("previous")).unwrap();
    new_store(d.path());
    assert!(held.recheck(&p).is_err()); // same bytes, type and name but different, still-live inode
}
#[test]
fn a_replacement_before_receive_or_a_readable_descriptor_never_matches_the_guest_handoff(){
    let (d,p,uid,gid,b)=fixture();let (rx,tx)=OriginChannel::open().unwrap();
    let pin=fixed_name(&p).unwrap();send(&tx,&b,&[&pin]);drop(pin);drop(tx);
    std::fs::rename(d.path().join("secrets.tdb"),d.path().join("previous")).unwrap();new_store(d.path());
    assert!(rx.receive(&p,uid,gid,rustix::process::getpid()).is_err());
    let (rx,tx)=OriginChannel::open().unwrap();let data=File::open(d.path().join("secrets.tdb")).unwrap();
    send(&tx,&b,&[&data]);drop(tx);assert!(rx.receive(&p,uid,gid,rustix::process::getpid()).is_err());
}
#[test]
fn missing_double_extra_empty_wrong_shape_or_still_open_channels_fail(){
    let (_d,p,uid,gid,b)=fixture();let pin=fixed_name(&p).unwrap();
    for n in 0..7{
        let (rx,tx)=OriginChannel::open().unwrap();
        match n{
            0=>send(&tx,&b,&[]),1=>send(&tx,&b,&[&pin,&pin]),2=>{send(&tx,&b,&[&pin]);send(&tx,&[],&[]);},
            3=>{send(&tx,&b,&[&pin]);send(&tx,&b,&[&pin]);},4=>send(&tx,&[0x33;75],&[&pin]),
            5=>send(&tx,&b[1..],&[&pin]),_=>{send(&tx,&b,&[&pin]);assert!(rx.receive(&p,uid,gid,rustix::process::getpid()).is_err());continue;}
        }
        drop(tx);assert!(rx.receive(&p,uid,gid,rustix::process::getpid()).is_err());
    }
}

#[test]
fn same_uid_and_gid_do_not_authorize_a_different_payload_process(){
    let (_d,p,uid,gid,b)=fixture();let pin=fixed_name(&p).unwrap();
    let self_pid=rustix::process::getpid();
    let other=[1,2].map(|n|rustix::process::Pid::from_raw(n).unwrap()).into_iter().find(|&p|p!=self_pid).unwrap();
    for expected in [other,self_pid]{
        let (rx,tx)=OriginChannel::open().unwrap();send(&tx,&b,&[&pin]);drop(tx);
        assert_eq!(rx.receive(&p,uid,gid,expected).is_ok(),expected==self_pid);
    }
}

#[test]
fn unchanged_original_and_same_inode_changed_after_a_stamp_are_distinct(){
    use std::fs::FileTimes;use std::time::{Duration,UNIX_EPOCH};
    let (d,p,uid,gid,b)=fixture();let (rx,tx)=OriginChannel::open().unwrap();let pin=fixed_name(&p).unwrap();
    send(&tx,&b,&[&pin]);drop(tx);let h=rx.receive(&p,uid,gid,rustix::process::getpid()).unwrap();
    // These are explicit fixture timestamps, not a fake public process-exit
    // proof or a substitute for the real synchronization constructor.
    let file=SyncStamp::on(&h.local).unwrap();let parent=SyncStamp::on(&p).unwrap();
    let s=SyncedStore{original:h,file,parent};s.require(&p).unwrap();
    let same=File::options().write(true).open(d.path().join("secrets.tdb")).unwrap();
    same.set_times(FileTimes::new().set_modified(UNIX_EPOCH+Duration::from_secs(1))).unwrap();
    assert!(s.original.recheck(&p).is_ok());assert!(s.require(&p).is_err());
}
