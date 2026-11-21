//! Single nonsecret host-installation confirmation on this exact foreground
//! Linux N_TTY. Hidden input is a separate confined worker scope.
use super::{Channel,Surface,failure,fs,nonblocking_prompt,own_descriptor_table,refuse};
use helper_linux_evidence::prompt_signals::PromptSignals;
use rustix::{fs::{Mode,OFlags},io::Errno,rand::{GetRandomFlags,getrandom},
    termios::{LocalModes,QueueSelector,tcflush,tcgetattr}};
use std::{fs::File,io,os::fd::AsFd,time::{Duration,Instant}};

pub(in crate::sqlite_install) struct PublicConfirmation{terminal:Channel,reader:File}
fn readable(f:OFlags)->bool{
    matches!(f&OFlags::ACCMODE,OFlags::RDONLY|OFlags::RDWR)&&!f.contains(OFlags::PATH)
}
fn challenge()->io::Result<String>{
    let mut bytes=[0u8;16];
    loop{match getrandom(&mut bytes[..],GetRandomFlags::NONBLOCK){
        Ok(16) if bytes!=[0;16]=>return Ok(bytes.iter().map(|b|format!("{b:02x}")).collect()),
        Err(Errno::INTR)=>continue,Err(e)=>return Err(e.into()),_=>return Err(failure("missing complete fresh confirmation challenge")),
    }}
}
fn clear(f:&File)->io::Result<()>{Ok(tcflush(f,QueueSelector::IFlush)?)}
impl PublicConfirmation{
    pub(in crate::sqlite_install) fn select()->io::Result<Self>{
        let terminal=Channel::select()?; // rejects /dev/tty aliases; holds original stderr slave/VT
        if !readable(fs::fcntl_getfl(&io::stdin())?){return Err(refuse("standard input must already have read access"));}
        let (_proc,table)=own_descriptor_table()?;
        let link=File::from(fs::openat(&table,c"0",OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty())?);
        if !link.metadata()?.is_symlink()||fs::fstatfs(&link)?.f_type!=fs::PROC_SUPER_MAGIC
            ||crate::pinned::Mount::on(&link)?!=crate::pinned::Mount::on(&table)?{return Err(refuse("not the genuine original input link"));}
        let pin=fs::openat(&table,c"0",OFlags::PATH|OFlags::CLOEXEC,Mode::empty())?;
        if Surface::at(&pin)?!=terminal.original{return Err(refuse("not the original foreground input device"));}
        let reader=File::from(fs::openat(&table,c"0",OFlags::RDONLY|OFlags::NONBLOCK|OFlags::NOCTTY|OFlags::CLOEXEC,Mode::empty())?);
        let selected=Self{terminal,reader};selected.check()?;Ok(selected)
    }
    pub(in crate::sqlite_install) fn check(&self)->io::Result<()>{
        self.terminal.require()?;
        let key=tcgetattr(&self.reader)?;
        if !readable(fs::fcntl_getfl(&io::stdin())?)||Surface::at(&self.reader)?!=self.terminal.original
            ||fs::fcntl_getfl(&self.reader)?&(OFlags::NONBLOCK|OFlags::ACCMODE)!=(OFlags::NONBLOCK|OFlags::RDONLY)
            ||key.line_discipline!=0||!key.local_modes.contains(LocalModes::ICANON){
            return Err(refuse("the original nonblocking foreground canonical terminal is required"));}
        Ok(())
    }
    pub(in crate::sqlite_install) fn confirm_init(&self,fresh:bool,preview:String)->io::Result<()>{
        self.ask(if fresh{"INIT"}else{"RESUME"},Some(preview),1100*1024)
    }
    pub(in crate::sqlite_install) fn confirm_v18_initialization(&self,review:String)->io::Result<()>{
        self.ask("INIT-ZXS-V18",Some(review),1100*1024)
    }
    pub(in crate::sqlite_install::pair) fn confirm_fresh_zxs(&self,review:String)->io::Result<()>{
        self.ask("ISSUE-ZXS-V18",Some(review),1100*1024)
    }
    pub(in crate::sqlite_install::pair) fn confirm_ext_repair(&self,preview:String)->io::Result<()>{
        self.ask("REPAIR-EXT-COPY",Some(preview),64*1024)
    }
    pub(in crate::sqlite_install::pair) fn confirm_exfat_repair(&self,preview:String)->io::Result<()>{
        self.ask("REPAIR-EXFAT-COPY",Some(preview),64*1024)
    }
    pub(in crate::sqlite_install::pair) fn confirm_fat32_repair(&self,preview:String)->io::Result<()>{
        self.ask("REPAIR-FAT32-COPY",Some(preview),64*1024)
    }
    pub(in crate::sqlite_install::pair) fn confirm_shared_account_reset(&self,review:String)->io::Result<()>{
        self.ask("RESET-ZXS-ACCOUNT",Some(review),96*1024)
    }
    pub(in crate::sqlite_install::pair) fn report(&self,line:&str)->io::Result<()>{
        nonblocking_prompt(self.terminal.out.as_fd(),line.as_bytes(),Instant::now()+Duration::from_secs(2),&||self.check())
    }
    fn ask(&self,verb:&'static str,preview:Option<String>,limit:usize)->io::Result<()>{
        let fresh=challenge()?;let answer=format!("{verb} {fresh}\n");
        let prompt=format!("同意以上完整范围时，输入 {verb} {fresh} 并回车。此处不输入口令。\n");
        let sig=PromptSignals::begin()?;let end=Instant::now()+Duration::from_secs(120);
        let check=||{sig.check()?;self.check()};
        let result=(||->io::Result<()>{
            check()?;clear(&self.reader)?; // stale lines cannot provide this confirmation
            if let Some(text)=preview.as_ref(){
                // The bounded action preview and challenge share one
                // nonblocking OFD and the same real deadline.
                if text.len()>limit{return Err(failure("fixed installation preview exceeds its budget"));}
                nonblocking_prompt(self.terminal.out.as_fd(),text.as_bytes(),end,&check)?;
                clear(&self.reader)?;
            }
            nonblocking_prompt(self.terminal.out.as_fd(),prompt.as_bytes(),end,&check)?;
            let mut line=[0u8;96];
            while Instant::now()<end{
                check()?;
                // Canonical mode: one terminated record, or a short/overlength
                // fragment that cannot equal the exact challenge line.
                match rustix::io::read(&self.reader,&mut line[..]){
                    Ok(n) if &line[..n]==answer.as_bytes()=>return Ok(()),
                    Ok(_)=>return Err(refuse("this exact foreground installation was not approved")),
                    Err(Errno::INTR)=>continue,Err(Errno::AGAIN)=>std::thread::sleep(Duration::from_millis(20)),
                    Err(e)=>return Err(e.into()),
                }
            }
            Err(io::Error::new(io::ErrorKind::TimedOut,"installation confirmation expired"))
        })();
        // Includes rejection, overlong input, timeout and catchable signal:
        // do not return a remainder to a root shell. Do this before unmasking.
        let discarded=clear(&self.reader);let stop=sig.check();let restored=sig.finish();
        discarded?;restored?;stop?;result
    }
}
#[cfg(test)]
mod tests{
    use super::*;
    use rustix::{fs::{fcntl_getfl,fcntl_setfl},pty::{OpenptFlags,grantpt,ioctl_tiocgptpeer,openpt,unlockpt}};
    #[test]
    fn stale_complete_lines_and_the_tail_of_an_overlong_rejection_are_cleared_on_a_disposable_pty(){
        let flags=OpenptFlags::RDWR|OpenptFlags::NOCTTY|OpenptFlags::CLOEXEC;
        let master=openpt(flags).unwrap();grantpt(&master).unwrap();unlockpt(&master).unwrap();
        let slave=File::from(ioctl_tiocgptpeer(&master,flags).unwrap());
        fcntl_setfl(&slave,fcntl_getfl(&slave).unwrap()|OFlags::NONBLOCK).unwrap();
        let mut buf=[0u8;96];assert!(tcgetattr(&slave).unwrap().local_modes.contains(LocalModes::ICANON));
        let old=format!("INIT {}\n","a".repeat(64)); // old exact request SHA was repeatable
        rustix::io::write(&master,old.as_bytes()).unwrap();clear(&slave).unwrap();
        assert_eq!(rustix::io::read(&slave,&mut buf[..]),Err(Errno::AGAIN));
        let mut text=vec![b'x';140];text.extend(b"\na later line\n");rustix::io::write(&master,&text).unwrap();
        assert_eq!(rustix::io::read(&slave,&mut buf[..]).unwrap(),96);clear(&slave).unwrap();
        assert_eq!(rustix::io::read(&slave,&mut buf[..]),Err(Errno::AGAIN));
        let fresh=format!("INIT {}\n","b".repeat(32));
        rustix::io::write(&master,fresh.as_bytes()).unwrap();
        let n=rustix::io::read(&slave,&mut buf[..]).unwrap();
        assert_eq!(&buf[..n],fresh.as_bytes());assert_ne!(fresh,old);
    }
}
