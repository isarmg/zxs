//! A single startup fence for the two fixed ZXS roles, not a listener or
//! a site/credential grant. Only the original authenticated START and the
//! matching two PREPARED receipts can reach the common terminal seal.
use super::{Fault,Result,kernel};
#[cfg(all(target_arch="x86_64",target_pointer_width="64"))]
use super::custody::Parent;
use std::{
    ffi::{CStr,CString},fs::{File,Permissions},io::Write,mem::{size_of,zeroed},os::fd::{AsRawFd,OwnedFd},
    os::unix::fs::{FileExt,MetadataExt,PermissionsExt},time::{Duration,Instant},
};

const MAGIC:&[u8;16]=b"ZXS-ADMIT-PAIR1\0";
type GateBody=[u8;80];
const HELD:libc::c_int=libc::F_SEAL_EXEC|libc::F_SEAL_WRITE|libc::F_SEAL_GROW|libc::F_SEAL_SHRINK;
const RELEASED:libc::c_int=HELD|libc::F_SEAL_SEAL;
#[cfg(all(target_arch="x86_64",target_pointer_width="64"))]
const PIN:libc::c_int=libc::O_PATH|libc::O_NOFOLLOW|libc::O_CLOEXEC;
#[cfg(all(target_arch="x86_64",target_pointer_width="64"))]
const PROC:libc::c_long=libc::PROC_SUPER_MAGIC as libc::c_long;
pub(super) const MAX_PAIR_WAIT:Duration=Duration::from_secs(45);
const POLL_STEP:Duration=Duration::from_millis(5);

/// Public comparison values scoped by the actual authenticated START.
/// Digest covers all 80 immutable bytes, not just the embedded cohort.
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(super) struct GateStamp{
    pub(super) dev:u64,pub(super) ino:u64,pub(super) digest:[u8;32],
}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
enum SealState{Held,Released}
fn seal_state(seals:libc::c_int)->Result<SealState>{
    match seals{HELD=>Ok(SealState::Held),RELEASED=>Ok(SealState::Released),_=>Err(Fault::Handoff)}
}
fn body(nonce:[u8;32],cohort:[u8;32])->Result<GateBody>{
    if nonce==[0;32]||cohort==[0;32]{return Err(Fault::Shape);}
    let mut bytes=[0;80];
    bytes[..16].copy_from_slice(MAGIC);
    bytes[16..48].copy_from_slice(&nonce);
    bytes[48..80].copy_from_slice(&cohort);
    Ok(bytes)
}
fn digest(bytes:&GateBody)->Result<[u8;32]>{crate::hash::digest(bytes).map_err(|_|Fault::Handoff)}
fn matches_body(raw:&GateBody,expected:&GateBody,sha:[u8;32])->Result<()>{
    if raw!=expected||digest(raw)?!=sha{return Err(Fault::Handoff);}
    Ok(())
}
fn identity(file:&File)->Result<(u64,u64)>{
    let m=file.metadata().map_err(|_|Fault::Handoff)?;
    if kernel::fs_magic(file.as_raw_fd())?!=libc::TMPFS_MAGIC as libc::c_long||
        m.mode()!=0o100400||m.uid()!=0||m.gid()!=0||m.nlink()!=0||m.len()!=80||m.ino()==0{
        return Err(Fault::Handoff);
    }
    Ok((m.dev(),m.ino()))
}
fn checked(file:&File,access:libc::c_int,stamp:GateStamp,expected:&GateBody)->Result<SealState>{
    let fd=file.as_raw_fd();
    kernel::cloexec(fd)?;
    if kernel::get_flags(fd,libc::F_GETFL)?&(libc::O_ACCMODE|libc::O_PATH)!=access||
        identity(file)?!=(stamp.dev,stamp.ino){return Err(Fault::Handoff);}
    let first=seal_state(kernel::get_flags(fd,libc::F_GET_SEALS)?)?;
    let mut raw=[0;80];
    if file.read_at(&mut raw,0).map_err(|_|Fault::Handoff)?!=raw.len(){return Err(Fault::Handoff);}
    matches_body(&raw,expected,stamp.digest)?;
    if identity(file)?!=(stamp.dev,stamp.ino){return Err(Fault::Handoff);}
    let last=seal_state(kernel::get_flags(fd,libc::F_GET_SEALS)?)?;
    if first==SealState::Released&&last!=first{return Err(Fault::Handoff);}
    Ok(last)
}

#[cfg(all(target_arch="x86_64",target_pointer_width="64"))]
#[must_use]
pub(super) struct GateOwner<'a>{parent:&'a Parent,original:File,stamp:GateStamp,expected:GateBody}
/// Exactly one read-only loan for each original child. Neither is a dup of
/// the writer. Child setup closes inherited descriptors besides fd3/fd4;
/// these loans belong in the post-exec authenticated START messages.
#[cfg(all(target_arch="x86_64",target_pointer_width="64"))]
#[must_use]
pub(super) struct ReadLoans{pub(super) web:File,pub(super) smb:File}

#[cfg(all(target_arch="x86_64",target_pointer_width="64"))]
impl<'a> GateOwner<'a>{
    pub(super) fn prepare(parent:&'a Parent,nonce:[u8;32],cohort:[u8;32])->Result<(Self,ReadLoans)>{
        let expected=body(nonce,cohort)?;
        let task=parent.own_task()?;
        // SAFETY: fixed name/flags and one newly owned, non-executable anonymous FD.
        let mut original=unsafe{
            let raw=libc::memfd_create(c"zxs-pair-admission".as_ptr(),
                libc::MFD_CLOEXEC|libc::MFD_ALLOW_SEALING|libc::MFD_NOEXEC_SEAL);
            File::from(kernel::new_fd(raw.into())?)
        };
        let fd=original.as_raw_fd();
        if kernel::get_flags(fd,libc::F_GET_SEALS)?!=libc::F_SEAL_EXEC{return Err(Fault::Handoff);}
        original.set_permissions(Permissions::from_mode(0o400)).map_err(|_|Fault::Handoff)?;
        original.write_all(&expected).map_err(|_|Fault::Handoff)?;
        add_seals(&original,HELD&!libc::F_SEAL_EXEC)?;
        let (dev,ino)=identity(&original)?;
        let stamp=GateStamp{dev,ino,digest:digest(&expected)?};
        if checked(&original,libc::O_RDWR,stamp,&expected)?!=SealState::Held{return Err(Fault::Handoff);}
        let web=reopen_own(&task,&original,stamp,&expected)?;
        let smb=reopen_own(&task,&original,stamp,&expected)?;
        parent.require()?;
        Ok((Self{parent,original,stamp,expected},ReadLoans{web,smb}))
    }
    pub(super) fn stamp(&self)->GateStamp{self.stamp}

    /// Only the actually verified original Web + SMB pair may supply this
    /// final vote. No fallible step follows a successful one-shot seal.
    pub(super) fn seal_after_pair(self,prepared:super::custody::PreparedPair<'_,'_>)->Result<()>{
        self.parent.require()?;
        if checked(&self.original,libc::O_RDWR,self.stamp,&self.expected)?!=SealState::Held{return Err(Fault::Handoff);}
        prepared.last_vote(self.stamp)?;
        add_seals(&self.original,libc::F_SEAL_SEAL)
    }
}
#[cfg(all(target_arch="x86_64",target_pointer_width="64"))]
fn add_seals(original:&File,seals:libc::c_int)->Result<()>{
    // SAFETY: F_ADD_SEALS takes exactly this promoted int on the live gate FD.
    if unsafe{libc::fcntl(original.as_raw_fd(),libc::F_ADD_SEALS,seals)}==0{Ok(())}else{Err(Fault::Handoff)}
}

#[cfg(all(target_arch="x86_64",target_pointer_width="64"))]
fn proc_pin(dir:&File,name:&CStr)->Result<File>{
    if kernel::fs_magic(dir.as_raw_fd())?!=PROC{return Err(Fault::Handoff);}
    // SAFETY: open_how contains only integer fields; all unused values stay zero.
    let mut how:libc::open_how=unsafe{zeroed()};
    how.flags=PIN as u64;
    how.resolve=libc::RESOLVE_BENEATH|libc::RESOLVE_NO_XDEV|libc::RESOLVE_NO_MAGICLINKS;
    // SAFETY: a single exact component relative to the checked own proc
    // task or fd table, with no link follow, submount or directory trigger.
    let f=unsafe{File::from(kernel::new_fd(libc::syscall(libc::SYS_openat2,dir.as_raw_fd(),
        name.as_ptr(),&raw const how,size_of::<libc::open_how>()))?)};
    if kernel::fs_magic(f.as_raw_fd())?!=PROC{return Err(Fault::Handoff);}
    Ok(f)
}
#[cfg(all(target_arch="x86_64",target_pointer_width="64"))]
fn link_stamp(link:&File)->Result<(u64,u64)>{
    let m=link.metadata().map_err(|_|Fault::Handoff)?;
    if !m.file_type().is_symlink()||m.ino()==0{return Err(Fault::Handoff);}
    Ok((m.dev(),m.ino()))
}
#[cfg(all(target_arch="x86_64",target_pointer_width="64"))]
fn reopen_own(task:&File,original:&File,stamp:GateStamp,expected:&GateBody)->Result<File>{
    let table=proc_pin(task,c"fd")?;
    if !table.metadata().map_err(|_|Fault::Handoff)?.is_dir(){return Err(Fault::Handoff);}
    let name=CString::new(original.as_raw_fd().to_string()).map_err(|_|Fault::Handoff)?;
    let link=proc_pin(&table,&name)?;
    let pinned=link_stamp(&link)?;
    let follow=|flags:libc::c_int|->Result<File>{
        // SAFETY: only the literal decimal link for this same retained
        // original in the verified single-thread supervisor task. First
        // follow O_PATH, then reopen without changing original access.
        let raw=unsafe{libc::openat(table.as_raw_fd(),name.as_ptr(),flags|libc::O_CLOEXEC)};
        Ok(File::from(unsafe{kernel::new_fd(raw.into())?}))
    };
    let target=follow(libc::O_PATH)?;
    if identity(&target)?!=(stamp.dev,stamp.ino)||link_stamp(&proc_pin(&table,&name)?)?!=pinned{
        return Err(Fault::Handoff);
    }
    let reader=follow(libc::O_RDONLY|libc::O_NONBLOCK|libc::O_NOATIME|libc::O_NOCTTY)?;
    if checked(&reader,libc::O_RDONLY,stamp,expected)?!=SealState::Held||
        identity(original)?!=(stamp.dev,stamp.ino)||link_stamp(&proc_pin(&table,&name)?)?!=pinned{
        return Err(Fault::Handoff);
    }
    Ok(reader)
}

/// The low role has only this already received, truly read-only FD. An
/// expected stamp must come from that role's own authenticated START.
#[must_use]
pub(super) struct WaitingGate{file:File,stamp:GateStamp,expected:GateBody,expires:Instant}
impl WaitingGate{
    pub(super) fn from_start(fd:OwnedFd,stamp:GateStamp,nonce:[u8;32],cohort:[u8;32])->Result<Self>{
        let file=File::from(fd);let expected=body(nonce,cohort)?;
        let expires=Instant::now().checked_add(MAX_PAIR_WAIT).ok_or(Fault::Handoff)?;
        if checked(&file,libc::O_RDONLY,stamp,&expected)?!=SealState::Held{return Err(Fault::Handoff);}
        Ok(Self{file,stamp,expected,expires})
    }
    /// The live probe must be the role's actual original supervisor pidfd
    /// and TERM check; it must not block, grant rights, or run any business.
    pub(super) fn wait(self,until:Instant,live:impl Fn()->Result<()>)->Result<()>{
        let until=until.min(self.expires);
        loop{
            let left=until.checked_duration_since(Instant::now()).filter(|d|!d.is_zero()).ok_or(Fault::Handoff)?;
            live()?;
            if seal_state(kernel::get_flags(self.file.as_raw_fd(),libc::F_GET_SEALS)?)?==SealState::Released{
                if checked(&self.file,libc::O_RDONLY,self.stamp,&self.expected)?!=SealState::Released{
                    return Err(Fault::Handoff);
                }
                live()?;
                return if Instant::now()<until{Ok(())}else{Err(Fault::Handoff)};
            }
            std::thread::sleep(left.min(POLL_STEP));
        }
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn exact_pair_body_and_exact_seal_transitions(){
        let nonce=[1;32];let cohort=[2;32];let expected=body(nonce,cohort).unwrap();
        assert_eq!(&expected[..16],b"ZXS-ADMIT-PAIR1\0");
        assert_eq!(&expected[16..48],&nonce);
        assert_eq!(&expected[48..80],&cohort);
        let sha=digest(&expected).unwrap();
        assert!(matches_body(&expected,&expected,sha).is_ok());
        assert!(matches_body(&expected,&expected,[0;32]).is_err());
        for ix in [0usize,15,16,47,48,79]{
            let mut foreign=expected;foreign[ix]^=1;
            assert!(matches_body(&foreign,&expected,digest(&foreign).unwrap()).is_err());
        }
        assert!(body([0;32],cohort).is_err());
        assert!(body(nonce,[0;32]).is_err());
        assert_eq!(seal_state(HELD),Ok(SealState::Held));
        assert_eq!(seal_state(RELEASED),Ok(SealState::Released));
        for malformed in [0,libc::F_SEAL_EXEC,libc::F_SEAL_SEAL,HELD&!libc::F_SEAL_WRITE,RELEASED|libc::F_SEAL_FUTURE_WRITE]{
            assert_eq!(seal_state(malformed),Err(Fault::Handoff));
        }
    }
}
