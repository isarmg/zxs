//! A private nonblocking prompt channel to the same actual foreground tty.
//! No terminal path, device selection, mutable standard-stream flags, shell,
//! caller-supplied output or secret output. Keep one original slave/VT open.
use super::{own_descriptor_table,same_terminal};
use crate::disk_web::require_terminal;
use rustix::{fs::{self,AtFlags,Mode,OFlags,StatxFlags},io::Errno,termios::{tcgetpgrp,tcgetsid}};
use std::{fs::File,io,os::fd::{AsFd,BorrowedFd},time::{Duration,Instant}};

mod confirmation;
pub(crate) use confirmation::PublicConfirmation;

fn refuse(s: &'static str) -> io::Error { io::Error::new(io::ErrorKind::PermissionDenied, s) }
fn failure(s: &'static str) -> io::Error { io::Error::new(io::ErrorKind::InvalidInput, s) }

#[derive(Clone,Copy)]
pub(crate) enum Prompt { Joint }
impl Prompt {
    fn wording(self)->[&'static [u8];2] {
        match self {
            Self::Joint=>[
                "请输入 Web 与 SMB 共用的同一账户口令（不回显），然后按回车。\n".as_bytes(),
                "请再次输入同一账户口令。\n".as_bytes(),
            ],
        }
    }
}

fn already_writable(flags:OFlags)->bool {
    let access=flags&OFlags::ACCMODE;
    !flags.contains(OFlags::PATH)&&(access==OFlags::WRONLY||access==OFlags::RDWR)
}

#[derive(PartialEq,Eq)]
struct Surface { dev:u64,ino:u64,rdev:u64,mount:u64 }
impl Surface {
    fn at(fd:impl AsFd)->io::Result<Self> {
        let st=fs::fstat(&fd)?;
        let x=fs::statx(&fd,c"",AtFlags::EMPTY_PATH|AtFlags::STATX_DONT_SYNC|AtFlags::NO_AUTOMOUNT,StatxFlags::MNT_ID)?;
        // /dev/tty (5:0) is an indirect "current controlling tty", not a
        // pin of one particular slave. Absolute names are never a fallback.
        if st.st_mode&0o170_000!=0o020_000||st.st_ino==0
            ||(fs::major(st.st_rdev),fs::minor(st.st_rdev))==(5,0)
            ||x.stx_mask&StatxFlags::MNT_ID.bits()==0||x.stx_mnt_id==0 {
            return Err(refuse("need the actual original foreground slave or VT, not a generic tty alias"));
        }
        Ok(Self{dev:st.st_dev,ino:st.st_ino,rdev:st.st_rdev,mount:x.stx_mnt_id})
    }
}
pub(crate) struct Channel { out:File, original:Surface, session:i32, group:i32 }
impl Channel {
    pub(crate) fn select()->io::Result<Self> {
        let err=io::stderr();same_terminal(&io::stdin(),&err)?;
        let original=Surface::at(&err)?;
        if !already_writable(fs::fcntl_getfl(&err)?) {
            return Err(refuse("original foreground standard error must already be writable"));
        }
        let session=tcgetsid(&err)?.as_raw_pid();let group=tcgetpgrp(&err)?.as_raw_pid();
        let (proc,table)=own_descriptor_table()?;
        let lk=File::from(fs::openat(&table,c"2",OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty())?);
        if !lk.metadata()?.is_symlink()||fs::fstatfs(&lk)?.f_type!=fs::PROC_SUPER_MAGIC
            ||crate::pinned::Mount::on(&lk)?!=crate::pinned::Mount::on(&table)?{
            return Err(refuse("not the genuine current descriptor link for standard error"));
        }
        let pin=fs::openat(&table,c"2",OFlags::PATH|OFlags::CLOEXEC,Mode::empty())?;
        if Surface::at(&pin)?!=original{return Err(refuse("changed foreground terminal"));}
        // New OFD, not dup: NONBLOCK never changes stdin, stderr or the shell.
        // This is only write access to the terminal already held for writing.
        let out=File::from(fs::openat(&table,c"2",OFlags::WRONLY|OFlags::NONBLOCK|OFlags::NOCTTY|OFlags::CLOEXEC,Mode::empty())?);
        drop(proc);drop(table);
        let this=Self{out,original,session,group};this.require()?;Ok(this)
    }
    pub(crate) fn require(&self)->io::Result<()> {
        let input=io::stdin();let stdout=io::stdout();let err=io::stderr(); same_terminal(&input,&err)?;require_terminal(&self.out)?;
        for fd in [self.out.as_fd(),input.as_fd(),stdout.as_fd(),err.as_fd()] {
            if Surface::at(fd)?!=self.original{return Err(refuse("selected terminal object or mount changed"));}
        }
        if !already_writable(fs::fcntl_getfl(&err)?)||tcgetsid(&self.out)?.as_raw_pid()!=self.session||tcgetpgrp(&self.out)?.as_raw_pid()!=self.group
            ||fs::fcntl_getfl(&self.out)?&(OFlags::ACCMODE|OFlags::NONBLOCK)!=(OFlags::WRONLY|OFlags::NONBLOCK){
            return Err(refuse("selected terminal session, group or private prompt fd changed"));
        }Ok(())
    }
    pub(crate) fn prompt(&self,prompt:Prompt,index:usize,end:Instant,check:impl Fn()->io::Result<()>)->io::Result<()> {
        let bytes=*prompt.wording().get(index).ok_or_else(||failure("unknown credential prompt"))?;
        nonblocking_prompt(self.out.as_fd(),bytes,end,check)
    }
}

// This is not a poll-then-blocking-write trick: each actual write uses the
// private NONBLOCK OFD. Retry only while time, operator, tty and signal agree.
fn nonblocking_prompt(fd:BorrowedFd<'_>, mut msg:&[u8],end:Instant,check:impl Fn()->io::Result<()>)->io::Result<()> {
    while !msg.is_empty() {
        check()?;
        if Instant::now()>=end{return Err(io::Error::new(io::ErrorKind::TimedOut,"credential prompt could not be delivered in time"));}
        match rustix::io::write(fd,msg) {
            Ok(0)=>return Err(failure("no terminal output progress")),Ok(n)=>msg=&msg[n..],
            Err(Errno::INTR)=>continue,Err(Errno::AGAIN)=>std::thread::sleep(Duration::from_millis(20)),
            Err(e)=>return Err(e.into()),
        }
    }Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn no_new_write_right_from_a_reserved_access_mode_or_path_only_descriptor() {
        for f in [OFlags::WRONLY, OFlags::RDWR|OFlags::NONBLOCK]{assert!(already_writable(f));}
        for f in [OFlags::RDONLY, OFlags::ACCMODE, OFlags::PATH, OFlags::PATH|OFlags::WRONLY]{assert!(!already_writable(f));}
    }
    #[test] fn joint_account_secret_has_one_shared_prompt_and_one_repeat_not_two_roles() {
        let [first,repeat]=Prompt::Joint.wording();
        assert_eq!(first,"请输入 Web 与 SMB 共用的同一账户口令（不回显），然后按回车。\n".as_bytes());
        assert_eq!(repeat,"请再次输入同一账户口令。\n".as_bytes());

    }
    #[test] fn full_nonblocking_output_can_time_out_without_a_reader_or_a_blocking_write() {
        use rustix::pipe::{pipe_with,PipeFlags};
        let (_held_reader,writer)=pipe_with(PipeFlags::CLOEXEC|PipeFlags::NONBLOCK).unwrap();
        while let Ok(n)=rustix::io::write(&writer,&[0;4096]) {assert!(n>0);}
        while let Ok(n)=rustix::io::write(&writer,&[0]) {assert!(n>0);}
        assert_eq!(nonblocking_prompt(writer.as_fd(),b"fixed\n",Instant::now()+Duration::from_millis(10),||Ok(())).unwrap_err().kind(),io::ErrorKind::TimedOut);
        assert_eq!(nonblocking_prompt(writer.as_fd(),b"fixed\n",Instant::now()+Duration::from_secs(1),
            ||Err(io::Error::from(io::ErrorKind::Interrupted))).unwrap_err().kind(),io::ErrorKind::Interrupted);
    }
}
