//! An anonymous pair for one trusted child only. Receiving an inode and domain
//! proves continuity, not authorization, child exit, durability or login.
use super::LocalDomain;
use crate::{pinned::{Entity,Mount},sqlite_install::bad};
use rustix::{fd::OwnedFd,fs::{Mode,OFlags,fcntl_getfl,openat},net::{AddressFamily,RecvAncillaryBuffer,
    RecvAncillaryMessage,RecvFlags,ReturnFlags,SocketFlags,SocketType,UCred,recvmsg,socketpair,sockopt}};
use sha2::{Digest,Sha256};
use std::{fs::File,io::{self,IoSliceMut},mem::MaybeUninit,os::unix::fs::MetadataExt};

pub(super) struct OriginChannel { rx:OwnedFd }
pub(super) struct HeldOrigin {
    pub(super) domain:LocalDomain,
    guest:File, // remains O_PATH; never read, upgraded or used to enumerate the private store
    local:File, // same inode independently named on the HOST-selected mount
    parent:Entity, mount:u64, uid:u32, payload:rustix::process::Pid,
}
fn held_mount(fd:&File)->io::Result<u64>{Mount::on(fd)?.map(|m|m.id).ok_or_else(||bad("unavailable original mount"))}
fn entry(fd:&File,dev:u64,uid:u32)->io::Result<Entity>{
    let m=fd.metadata()?;
    if !fcntl_getfl(fd)?.contains(OFlags::PATH)||!m.is_file()||m.mode()&0o7777!=0o600||m.uid()!=uid
        ||m.nlink()!=1||m.dev()!=dev||m.len()==0 {return Err(bad("not the dedicated original store handle"));}
    Entity::of(&m)
}
fn fixed_name(parent:&File)->io::Result<File>{
    Ok(File::from(openat(parent,c"secrets.tdb",OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty())?))
}
// Both private protocol legs are one bounded record plus true EOF. Only the
// first leg contains the Samba guest's one O_PATH; the second transfers two
// already-held lease OFDs and a host O_PATH to the original supervisor.
#[derive(Default)]
struct Ancillary {fds:Vec<OwnedFd>, credentials:Option<UCred>}
fn packet<const N:usize>(socket:&OwnedFd)->io::Result<([u8;N],usize,Ancillary)>{
    let mut data=[0;N];
    let mut space=[MaybeUninit::uninit();rustix::cmsg_space!(ScmRights(4),ScmCredentials(1))];
    let mut control=RecvAncillaryBuffer::new(&mut space);
    let msg=recvmsg(socket,&mut [IoSliceMut::new(&mut data)],&mut control,
        RecvFlags::CMSG_CLOEXEC|RecvFlags::DONTWAIT)?;
    if msg.flags.intersects(ReturnFlags::TRUNC|ReturnFlags::CTRUNC)||msg.bytes>data.len(){
        return Err(bad("oversize dedicated SAM result"));}
    let mut c=Ancillary::default();
    for item in control.drain(){match item{
        RecvAncillaryMessage::ScmRights(fds)=>{
            if !c.fds.is_empty(){return Err(bad("repeated dedicated descriptors"));}
            c.fds.extend(fds);
        }
        RecvAncillaryMessage::ScmCredentials(who)=>{
            if c.credentials.replace(who).is_some(){return Err(bad("repeated sender credentials"));}
        }
        _=>return Err(bad("unexpected SAM ancillary data")),
    }}
    Ok((data,msg.bytes,c))
}
impl OriginChannel {
    pub(super) fn open()->io::Result<(Self,File)>{
        let (rx,tx)=socketpair(AddressFamily::UNIX,SocketType::SEQPACKET,SocketFlags::CLOEXEC,None)?;
        // A zero-length *record* also has SCM_CREDENTIALS; a true peer EOF
        // does not. Do not accept an extra empty packet as the end of the wire.
        sockopt::set_socket_passcred(&rx,true)?;
        if !sockopt::socket_passcred(&rx)?{return Err(bad("cannot attribute the dedicated handoff"));}
        Ok((Self{rx},File::from(tx)))
    }
    // N and K are fixed by each private direction, never supplied by a packet.
    pub(super) fn exact<const N:usize,const K:usize>(self,expected:UCred)->io::Result<([u8;N],[File;K])>{
        let (data,n,c)=packet::<N>(&self.rx)?;
        if n!=N||!c.credentials.is_some_and(|w|w.pid==expected.pid&&w.uid==expected.uid&&w.gid==expected.gid){
            return Err(bad("incomplete packet or wrong local handoff subject"));}
        let fds:[OwnedFd;K]=c.fds.try_into().map_err(|_|bad("wrong number of original handles"))?;
        let (_,tail,end)=packet::<1>(&self.rx)?;
        if tail!=0||!end.fds.is_empty()||end.credentials.is_some(){return Err(bad("extra or unfinished SAM handoff"));}
        Ok((data,fds.map(File::from)))
    }
    /// The original PAYLOAD PID comes only from its independent supervisor,
    /// never the monitor, a PID from the wire or a subsequent process lookup.
    /// EOF is NOT a process-exit notification.
    pub(super) fn receive(self,parent:&File,uid:u32,gid:u32,guest_pid:rustix::process::Pid)->io::Result<HeldOrigin>{
        let (data,[guest])=self.exact::<46,1>(UCred{pid:guest_pid,uid:rustix::process::Uid::from_raw(uid),gid:rustix::process::Gid::from_raw(gid)})?;
        let domain=LocalDomain::guest_bytes(&data)?;
        let original=HeldOrigin{domain,guest,local:fixed_name(parent)?,parent:Entity::of(&parent.metadata()?)?,
                                mount:held_mount(parent)?,uid,payload:guest_pid};
        original.recheck(parent)?;Ok(original)
    }
}
// Keep the evidence straddling the real sync, not just the inode: reopening
// the same name or sampling the attributes for the first time later is not a
// continuation of that observation.
#[derive(Copy,Clone,Eq,PartialEq)]
struct SyncStamp{entity:Entity,mount:u64,links:u64,len:u64,mode:u32,uid:u32,mtime:(i64,i64),ctime:(i64,i64)}
impl SyncStamp{
    fn on(f:&File)->io::Result<Self>{let m=f.metadata()?;Ok(Self{entity:Entity::of(&m)?,mount:held_mount(f)?,
        links:m.nlink(),len:m.len(),mode:m.mode(),uid:m.uid(),mtime:(m.mtime(),m.mtime_nsec()),ctime:(m.ctime(),m.ctime_nsec())})}
}
// Ephemeral, nonsecret IPC comparison only. It is never a persistent
// original ID, a substitute for the simultaneously held files, or authority.
fn stamp_pair(file:SyncStamp,dir:SyncStamp)->[u8;32]{
    let mut h=Sha256::new();h.update(b"helper: original SAM file and parent at sync\0");
    for s in [file,dir] {
        for n in [s.entity.dev,s.entity.ino,s.mount,s.links,s.len]{h.update(n.to_be_bytes());}
        h.update(s.mode.to_be_bytes());h.update(s.uid.to_be_bytes());
        for n in [s.mtime.0,s.mtime.1,s.ctime.0,s.ctime.1]{h.update(n.to_be_bytes());}
    }h.finalize().into()
}
pub(super) struct SyncedStore{original:HeldOrigin,file:SyncStamp,parent:SyncStamp}
impl SyncedStore{
    /// The second local handoff passes only an O_PATH, never a readable TDB.
    pub(super) fn feedback(&self,dir:&File)->io::Result<(&File,[u8;32])>{
        self.require(dir)?;Ok((&self.original.local,stamp_pair(self.file,self.parent)))
    }
    pub(super) fn verify_feedback(pin:&File,dir:&File,uid:u32,expected:&[u8;32])->io::Result<()>{
        let p=dir.metadata()?;let m=held_mount(dir)?;
        if !p.is_dir()||p.uid()!=uid||p.mode()&0o7777!=0o700||p.nlink()==0||held_mount(pin)?!=m{
            return Err(bad("wrong original private mount or directory"));}
        let named=fixed_name(dir)?;
        if entry(pin,p.dev(),uid)?!=entry(&named,p.dev(),uid)?||held_mount(&named)?!=m
            ||stamp_pair(SyncStamp::on(pin)?,SyncStamp::on(dir)?)!=*expected {
            return Err(bad("returned private inode or original sync evidence changed"));}
        Ok(())
    }
    pub(super) fn require(&self,dir:&File)->io::Result<&HeldOrigin>{
        self.original.recheck(dir)?;
        if SyncStamp::on(&self.original.local)?!=self.file||SyncStamp::on(dir)?!=self.parent{
            return Err(bad("private store or parent no longer matches the completed synchronization"));}
        Ok(&self.original)
    }
}
impl HeldOrigin {
    /// The private store is touched only AFTER the real payload and its
    /// entire original process group have been successfully collected.
    pub(super) fn sync_finished(self,parent:&File,ended:&super::provision::Finished)->io::Result<SyncedStore>{
        if ended.pid()!=self.payload{return Err(bad("different original provisioning payload"));}
        self.recheck(parent)?;
        let file=SyncStamp::on(&self.local)?;let parent_stamp=SyncStamp::on(parent)?;
        crate::pinned::sync_sam_store(&self.local,self.uid,self.mount)?;
        parent.sync_all()?;
        let synced=SyncedStore{original:self,file,parent:parent_stamp};
        synced.require(parent)?;Ok(synced)
    }
    pub(super) fn recheck(&self,parent:&File)->io::Result<()>{
        let p=parent.metadata()?;
        if !p.is_dir()||p.mode()&0o7777!=0o700||p.uid()!=self.uid||p.nlink()==0
            ||Entity::of(&p)?!=self.parent||held_mount(parent)?!=self.mount{return Err(bad("dedicated private directory changed"));}
        let id=entry(&self.guest,p.dev(),self.uid)?;
        let now=fixed_name(parent)?;
        for f in [&self.local,&now]{
            if entry(f,p.dev(),self.uid)?!=id||held_mount(f)?!=self.mount{return Err(bad("private host name does not refer to the guest's original inode"));}
        }
        // Guest's bind mount has its own ID. Its live inode is evidence only
        // because it is the SAME inode as our separately selected original.
        Ok(())
    }
}
#[cfg(test)]
#[path="../../../../tests/sqlite/samba_handoff.rs"]
mod tests;
