//! Internal online-launch handoff, separate from offline SAM creation and from Web.
//! No service entry point, spawn, signalling or wait/reap of another worker here.
//! A later trusted single-purpose supervisor must create its own two roles and
//! retain the ORIGINAL, unreaped process pidfd from smbd birth even on any error.
mod process;
use crate::fs_tools::{RuntimeFile,hash_upto};
use crate::pinned::{Entity,Mount};
use helper_linux_evidence::smb_process::initially_no_sender_pidfd;
use rustix::{fs::{OFlags,fcntl_getfl,fstatfs},net::{AddressFamily,SocketFlags,SocketType,
    socketpair,sockopt::{socket_acceptconn,socket_passcred,socket_peercred,socket_type}},
    process::{getegid,geteuid,getpid,getuid}};
use std::{fs::File,io::{self,Seek},os::{fd::{AsFd,BorrowedFd},unix::fs::{FileExt,MetadataExt}}};

const MAX_IMAGE:u64=128<<20;
const DAEMON:&[&str]=&["usr","lib","zxs-smb-runtime","debian13-amd64","sbin","smbd"];
fn no()->io::Error {io::Error::other("original dedicated SMB launch could not be established")}

pub(crate) struct Image(RuntimeFile);
impl Image {
    /// Expected bytes are supplied from the separately approved online release,
    /// never by smbd, the Web request, a filename search or this file's own hash.
    pub(crate) fn selected(expected:[u8;32])->io::Result<Self>{
        if expected==[0;32]{return Err(no());}
        let mut f=RuntimeFile::open(DAEMON,true,MAX_IMAGE)?;
        let m=f.file.metadata()?;
        let mut hdr=[0;64];f.file.read_exact_at(&mut hdr,0)?;
        if m.gid()!=0 || m.nlink()!=1 || !matches!(m.mode()&0o7777,0o555|0o755)
            || &hdr[..7]!=b"\x7fELF\x02\x01\x01" || !matches!(u16::from_le_bytes([hdr[16],hdr[17]]),2|3)
            || hdr[18..20]!=[62,0] {return Err(no());}
        f.file.rewind()?;
        if hash_upto(&mut f.file,m.len(),MAX_IMAGE)?!=expected{return Err(no());}
        f.file.rewind()?;f.recheck()?;
        Ok(Self(f))
    }
    pub(crate) fn original(&self)->io::Result<BorrowedFd<'_>>{
        self.0.recheck()?; Ok(self.0.file.as_fd())
    }
}

pub(crate) struct Control { daemon:File, storage:File }
impl Control{
    /// Fresh kernel peers with no public address, not a listener or a connection
    /// supplied by a socket path/client. No protocol data is consumed here.
    pub(crate) fn fresh()->io::Result<Self>{
        if getuid().as_raw()!=0||geteuid().as_raw()!=0{return Err(no());}
        let (l,r)=socketpair(AddressFamily::UNIX,SocketType::SEQPACKET,SocketFlags::CLOEXEC,None)?;
        let v=Self{daemon:File::from(l),storage:File::from(r)};v.untouched()?;Ok(v)
    }
    fn untouched(&self)->io::Result<()>{
        for fd in [&self.daemon,&self.storage]{
            let who=socket_peercred(fd)?;
            if who.pid!=getpid()||who.uid.as_raw()!=0||who.gid!=getegid()||
                fstatfs(fd)?.f_type as u64!=0x534f434b||socket_type(fd)?!=SocketType::SEQPACKET
                ||socket_acceptconn(fd)?||socket_passcred(fd)?||fcntl_getfl(fd)?.contains(OFlags::NONBLOCK){
                return Err(no());
            }
            initially_no_sender_pidfd(fd)?;
        }
        if Entity::of(&self.daemon.metadata()?)?==Entity::of(&self.storage.metadata()?)?{return Err(no());}
        Ok(())
    }
    /// Sole dispatch of the first endpoint to the selected foreground ELF is
    /// the launcher's job. Do not pass the storage side or other inherited fds.
    pub(crate) fn first_endpoint(&self)->io::Result<BorrowedFd<'_>>{
        self.untouched()?;Ok(self.daemon.as_fd())
    }
    /// Provenance at creation is a precondition, not reconstructed from waitid
    /// or PPid: a subreaper can also wait for a child it adopted. No caller or
    /// enrollment interface for arbitrary pidfds exists. The process ref stays with its
    /// supervisor, so failure cannot consume its ability to stop/reap the child.
    pub(crate) fn for_storage<'a>(self,birth:&'a File,image:&Image)->io::Result<StorageSide<'a>>{
        self.untouched()?;image.0.recheck()?;
        process::qualify(birth,&image.0.file,&self.daemon,&self.storage)?;
        self.untouched()?;image.0.recheck()?;process::waitable_live(birth)?;
        // No extra daemon-side reference is passed to the storage child.
        drop(self.daemon);
        Ok(StorageSide{endpoint:self.storage,master:birth})
    }
}
/// The storage program additionally needs its own independently verified BOUND
/// fd4, restricted identity/namespace and approved launch profile. No raw endpoint
/// or pidfd here is an account, pool write permission, readiness or drain proof.
pub(crate) struct StorageSide<'a>{endpoint:File,master:&'a File}
impl StorageSide<'_>{
    pub(crate) fn original_3_5(&self)->io::Result<(BorrowedFd<'_>,BorrowedFd<'_>)>{
        process::waitable_live(self.master)?;Ok((self.endpoint.as_fd(),self.master.as_fd()))
    }
}
fn same(a:&File,b:&File)->io::Result<bool>{
    let sa=Entity::of(&a.metadata()?)?;let sb=Entity::of(&b.metadata()?)?;
    let ma=Mount::on(a)?.ok_or_else(no)?;let mb=Mount::on(b)?.ok_or_else(no)?;
    Ok(sa==sb&&ma.id==mb.id)
}
