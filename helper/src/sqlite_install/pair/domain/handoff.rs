//! An anonymous pair for one trusted child only. Receiving an inode and domain
//! proves continuity, not authorization, child exit, durability or login.
use super::LocalDomain;
use crate::{pinned::{Entity,Mount},sqlite_install::bad};
use rustix::{fd::OwnedFd,fs::{Mode,OFlags,fcntl_getfl,openat},net::{AddressFamily,RecvAncillaryBuffer,
    RecvAncillaryMessage,RecvFlags,ReturnFlags,SocketFlags,SocketType,UCred,recvmsg,socketpair,sockopt}};
use std::{fs::File,io::{self,IoSliceMut},mem::MaybeUninit,os::unix::fs::MetadataExt};

pub(super) struct OriginChannel { rx:OwnedFd }
pub(super) struct HeldOrigin {
    pub(super) domain:LocalDomain,
    guest:File, // remains O_PATH; never read, upgraded or used to enumerate the private store
    local:File, // same inode independently named on the HOST-selected mount
    parent:Entity, mount:u64, uid:u32,
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
#[derive(Default)]
struct Ancillary {fd:Option<OwnedFd>, credentials:Option<UCred>}
fn packet(socket:&OwnedFd)->io::Result<([u8;64],usize,Ancillary)>{
    let mut data=[0;64];
    // Extra room lets us diagnose duplicate descriptors; truncation always fails.
    let mut space=[MaybeUninit::uninit();rustix::cmsg_space!(ScmRights(2),ScmCredentials(1))];
    let mut control=RecvAncillaryBuffer::new(&mut space);
    let msg=recvmsg(socket,&mut [IoSliceMut::new(&mut data)],&mut control,
        RecvFlags::CMSG_CLOEXEC|RecvFlags::DONTWAIT)?;
    if msg.flags.intersects(ReturnFlags::TRUNC|ReturnFlags::CTRUNC)||msg.bytes>data.len(){
        return Err(bad("oversize dedicated SAM result"));}
    let mut c=Ancillary::default();
    for item in control.drain(){match item{
        RecvAncillaryMessage::ScmRights(mut fds)=>{
            let fd=fds.next().ok_or_else(||bad("no original handle"))?;
            if c.fd.replace(fd).is_some()||fds.next().is_some(){return Err(bad("unexpected extra SAM handles"));}
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
    /// No blocking read. A still open channel, missing credential, second
    /// record, wrong origin or incomplete child output cannot mint even this
    /// limited held-object evidence. EOF is NOT proof of process termination.
    // guest_pid is the original PAYLOAD in the receiver's PID namespace,
    // supplied only by its independent process supervisor (not the monitor,
    // a wire field, a user-supplied PID, or a subsequent process lookup).
    pub(super) fn receive(self,parent:&File,uid:u32,gid:u32,guest_pid:rustix::process::Pid)->io::Result<HeldOrigin>{
        let (data,n,c)=packet(&self.rx)?;
        if !c.credentials.is_some_and(|w|w.pid==guest_pid&&w.uid.as_raw()==uid&&w.gid.as_raw()==gid){return Err(bad("missing or wrong local handoff subject"));}
        let guest=File::from(c.fd.ok_or_else(||bad("guest did not transfer the original handle"))?);
        let domain=LocalDomain::guest_bytes(&data[..n])?;
        let (_,tail,end)=packet(&self.rx)?;
        if tail!=0||end.fd.is_some()||end.credentials.is_some(){return Err(bad("more than one guest handoff record"));}
        let original=HeldOrigin{domain,guest,local:fixed_name(parent)?,parent:Entity::of(&parent.metadata()?)?,
                                mount:held_mount(parent)?,uid};
        original.recheck(parent)?;Ok(original)
    }
}
impl HeldOrigin {
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
