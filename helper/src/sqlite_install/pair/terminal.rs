//! Local, unbuffered entry on the already-open controlling terminal. Private:
//! this is not maintenance authorization, Samba-domain evidence or an issuer.
mod audit;
mod channel;
pub(super) use channel::PublicConfirmation;
use audit::UnauditedInput;
use super::web_secret::{PreparedWeb,WebWorkspace};
use super::super::approval::{host_admin,same_terminal};
use crate::maintenance::{diagnostic_worker, thread_file};
use helper_linux_evidence::prompt_signals::PromptSignals;
use rustix::{fs::{fcntl_getfl, OFlags}, mm::{mlockall, MlockAllFlags},
    process::{dumpable_behavior, getrlimit, set_dumpable_behavior, setrlimit, DumpableBehavior, Resource, Rlimit},
    termios::{tcflush, tcgetattr, tcsetattr, InputModes, LocalModes, OptionalActions, QueueSelector, SpecialCodeIndex as Cc, Termios}};
use std::{io::{self, Read}, os::fd::{AsFd, BorrowedFd}, time::{Duration, Instant}};
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, Zeroizing};

const MAX: usize = 1024;
type Secret = Zeroizing<Box<[u8]>>;
fn refuse(s: &'static str) -> io::Error { io::Error::new(io::ErrorKind::PermissionDenied, s) }
fn failure(s: &'static str) -> io::Error { io::Error::new(io::ErrorKind::InvalidInput, s) }

/// Only for a fresh, single-threaded process after its final UID/GID transition.
/// Restrictions last until exit; there is no unlock, fork or new privilege here.
/// They do not make the host administrator, PTY master, audit policy, crash
/// capture or hibernation trusted: those are independent deployment gates.
fn protected_process(uid: u32, gid: u32) -> io::Result<()> {
    diagnostic_worker(uid, gid)?;
    protect_memory()
}

fn protect_memory()->io::Result<()>{
    setrlimit(Resource::Core, Rlimit { current: Some(0), maximum: Some(0) })?;
    set_dumpable_behavior(DumpableBehavior::NotDumpable)?;
    let lim = getrlimit(Resource::Core);
    if lim.current != Some(0) || lim.maximum != Some(0)
        || dumpable_behavior()? != DumpableBehavior::NotDumpable {
        return Err(refuse("core dumps must be disabled for the credential worker"));
    }
    let mut status = Vec::new();
    thread_file(c"thread-self/status")?.take(65537).read_to_end(&mut status)?;
    let mut tracer = status.split(|&v| v == b'\n').filter_map(|s| s.strip_prefix(b"TracerPid:"));
    if status.len() > 65536 || !status.ends_with(b"\n")
        || tracer.next().map(<[u8]>::trim_ascii) != Some(b"0".as_slice()) || tracer.next().is_some() {
        return Err(refuse("cannot establish an untraced credential worker"));
    }
    // Failure (including a low configured MEMLOCK allowance) is not an
    // invitation to read first, prompt via a helper, or use an unlocked buffer.
    mlockall(MlockAllFlags::CURRENT | MlockAllFlags::FUTURE)?;
    Ok(())
}

// Both buffers are deliberate, bounded allocations, wiped at end of ownership.
// Limit before accepting a byte; no terminal history, String or line-reader.
struct Entry { bytes: Secret, len: usize, events: usize }
impl Entry {
    fn new() -> Self { Self { bytes: Zeroizing::new(vec![0; MAX].into_boxed_slice()), len: 0, events: 0 } }
    fn key(&mut self, b: u8) -> io::Result<bool> {
        self.events += 1;
        if self.events > 4096 { return Err(failure("too many input events")); }
        match b {
            b'\r' | b'\n' => {
                if self.len == 0 || std::str::from_utf8(&self.bytes[..self.len]).is_err() {
                    return Err(failure("a nonempty UTF-8 passphrase is required"));
                }
                Ok(true)
            }
            8 | 127 => {
                if self.len != 0 {
                    let mut start = self.len - 1;
                    while start > 0 && self.bytes[start] & 0xc0 == 0x80 { start -= 1; }
                    self.bytes[start..self.len].zeroize(); self.len = start;
                }
                Ok(false)
            }
            0..=31 => Err(failure("unsupported terminal control character; entry cancelled")),
            _ if self.len == MAX => Err(failure("passphrase exceeds 1024 bytes; no truncated value accepted")),
            _ => { self.bytes[self.len] = b; self.len += 1; Ok(false) }
        }
    }
    fn bytes(&self)->&[u8] { &self.bytes[..self.len] }
    fn take(self) -> Secret {
        let mut exact = Zeroizing::new(vec![0u8; self.len].into_boxed_slice());
        exact.copy_from_slice(&self.bytes[..self.len]); exact
    }
}

fn same_mode(a: &Termios, b: &Termios) -> bool {
    // All fields we change, plus the untouched discipline, character format,
    // output translation and speeds. We never change any other control key.
    a.input_modes == b.input_modes && a.local_modes == b.local_modes
        && a.output_modes == b.output_modes && a.control_modes == b.control_modes
        && a.line_discipline == b.line_discipline && a.input_speed() == b.input_speed()
        && a.output_speed() == b.output_speed()
        && [Cc::VMIN, Cc::VTIME].into_iter().all(|i| a.special_codes[i] == b.special_codes[i])
}

struct Quiet<'a> { fd: BorrowedFd<'a>, original: Termios, current: Termios, active: bool }
impl<'a> Quiet<'a> {
    fn begin(fd: BorrowedFd<'a>) -> io::Result<Self> {
        let old = tcgetattr(fd)?;
        if old.line_discipline != 0 || !old.local_modes.contains(LocalModes::ICANON) ||
            fcntl_getfl(fd)?.contains(OFlags::NONBLOCK) {
            return Err(refuse("a blocking foreground Linux N_TTY in ordinary line mode is required"));
        }
        let mut silent = old.clone();
        silent.input_modes = InputModes::empty(); // no stripping, CR folding or software flow control
        silent.local_modes = LocalModes::ISIG | (old.local_modes & LocalModes::TOSTOP);
        silent.special_codes[Cc::VMIN] = 0;
        silent.special_codes[Cc::VTIME] = 2; // wait at most two tenths per kernel read
        let q = Self { fd, original: old, current: silent, active: true };
        tcsetattr(fd, OptionalActions::Now, &q.current)?;
        q.verify()?;
        tcflush(fd, QueueSelector::IFlush)?; // remove type-ahead BEFORE any prompt
        Ok(q)
    }
    fn next_prompt(&self)->io::Result<()> {self.verify()?;tcflush(self.fd,QueueSelector::IFlush)?;Ok(())}
    fn verify(&self) -> io::Result<()> {
        if !same_mode(&tcgetattr(self.fd)?, &self.current) {
            return Err(refuse("terminal did not retain verified hidden-input settings"));
        }
        Ok(())
    }
    fn restore(&mut self) -> io::Result<()> {
        if !self.active { return Ok(()); }
        // Attempt restoration even if clearing queued bytes fails. Never accept
        // a result if either step or the real mode readback fails.
        let queued = tcflush(self.fd, QueueSelector::IFlush);
        tcsetattr(self.fd, OptionalActions::Now, &self.original)?;
        if !same_mode(&tcgetattr(self.fd)?, &self.original) {
            return Err(refuse("could not verify terminal restoration"));
        }
        queued?; self.active = false; Ok(())
    }
}
impl Drop for Quiet<'_> { fn drop(&mut self) { let _ = self.restore(); } }

fn capture(entries:&mut [Entry],offset:usize,selected:&SelectedInput,fd:BorrowedFd<'_>,
           verify:impl Fn()->io::Result<()>,signals:&PromptSignals)->io::Result<()> {
    let check=|| {signals.check()?; selected.audit.require_current()?; selected.channel.require()?;verify()};
    let mut scratch=Zeroizing::new([0u8;1]);
    for i in 0..entries.len() {
        check()?;tcflush(fd,QueueSelector::IFlush)?; // never accept leftovers from another prompt
        let end=Instant::now()+Duration::from_secs(120);
        selected.channel.prompt(offset+i,end,&check)?;
        loop {
            check()?;
            if Instant::now()>=end{return Err(io::Error::new(io::ErrorKind::TimedOut,"credential prompt expired"));}
            let n=match rustix::io::read(fd,&mut scratch[..]) {
                Err(rustix::io::Errno::INTR)=>continue,other=>other?,
            };
            if n==0 {continue;} // VMIN=0/VTIME=2: timeout or hangup, never a complete password
            let done=entries[i].key(scratch[0]);scratch.zeroize();
            if done? {break;}
        }
        if i==1&&!bool::from(entries[0].bytes().ct_eq(entries[1].bytes())){return Err(failure("the two protected password entries did not match"));}
    }
    check()
}

/// No secret is requested at selection. The additional private, nonblocking
/// write-only open is the exact already-writable stderr terminal, never a new
/// device capability for the caller. It cannot be extracted from Channel.
pub(super) struct SelectedInput {audit:UnauditedInput, channel:channel::Channel}
impl SelectedInput {
    pub(super) fn before_drop()->io::Result<Self>{
        let host=host_admin()?;
        let channel=channel::Channel::select()?;
        let audit=UnauditedInput::query_current()?;
        drop(host);Ok(Self{audit,channel})
    }
    /// Actual offline native process only, after loading the approved native
    /// closed role and applying its process restrictions.
    pub(super) fn smb_only(&self,ready:helper_linux_evidence::first_smb::Enrollment)
        ->io::Result<helper_linux_evidence::first_smb::FirstSlot>{
        let [smb,_]=self.pair_for(2)?;
        ready.prepare(&smb.take())
    }
    pub(super) fn lock_native_memory(&self)->io::Result<()>{
        self.audit.require_current()?;
        self.channel.require()?;
        protect_memory()
    }
    pub(super) fn web_only(&self,uid:u32,gid:u32)->io::Result<PreparedWeb>{
        let WebEntry{workspace,..}=self.reserve_web(uid,gid)?;
        let [web,_]=self.pair_for(0)?;
        PreparedWeb::fresh(web.take(),workspace)
    }
    pub(super) fn reserve_web(&self,uid:u32,gid:u32)->io::Result<WebEntry<'_>>{
        self.audit.require_current()?;self.channel.require()?;
        protected_process(uid,gid)?;
        let workspace=WebWorkspace::reserve()?;
        Ok(WebEntry{selected:self,workspace})
    }
    pub(super) fn hold_for_birth<'a>(&'a self,tty:&'a io::Stdin)->io::Result<HeldQuiet<'a>>{
        self.audit.require_current()?;self.channel.require()?;
        let signals=PromptSignals::begin()?;
        let tty=Quiet::begin(tty.as_fd())?;
        Ok(HeldQuiet{tty,signals,selected:self})
    }
    fn pair_for(&self,offset:usize)->io::Result<[Entry;2]>{
        if offset!=0&&offset!=2{return Err(failure("wrong fixed credential role"));}
        let mut entries=std::array::from_fn(|_|Entry::new());
        self.capture(&mut entries,offset)?;
        Ok(entries)
    }
    fn capture(&self,entries:&mut [Entry],offset:usize)->io::Result<()>{
        self.audit.require_current()?;self.channel.require()?;
        let stdin=io::stdin();let signals=PromptSignals::begin()?;
        let mut tty=Quiet::begin(stdin.as_fd())?;
        let captured=capture(entries,offset,self,tty.fd,||tty.verify(),&signals);
        let restored=tty.restore();drop(tty);
        let pending=signals.check();let mask=signals.finish();
        restored?;mask?;pending?;captured
    }
}

// Quiet is dropped BEFORE the original signal mask (including errors).
pub(super) struct HeldQuiet<'a>{tty:Quiet<'a>,signals:PromptSignals,selected:&'a SelectedInput}
impl HeldQuiet<'_>{
    pub(super) fn require(&self)->io::Result<()>{
        self.signals.check()?;self.selected.audit.require_current()?;self.selected.channel.require()?;self.tty.verify()
    }
    pub(super) fn smb(&self,provider:helper_linux_evidence::first_smb::Enrollment)
        ->io::Result<helper_linux_evidence::first_smb::FirstSlot>{
        self.require()?;
        let mut entries=std::array::from_fn::<_,2,_>(|_|Entry::new());
        capture(&mut entries,2,self.selected,self.tty.fd,||self.tty.verify(),&self.signals)?;
        let [smb,_]=entries;provider.prepare(&smb.take())
    }
    pub(super) fn finish(mut self)->io::Result<()>{
        self.tty.restore()?;let pending=self.signals.check();self.signals.finish()?;pending
    }
}

pub(super) struct WebEntry<'a>{selected:&'a SelectedInput,workspace:WebWorkspace}
impl WebEntry<'_>{
    // Parent owns the quiet terminal across both protocol prompts.
    // This child verifies the actual selected mode but cannot restore it or
    // pass a Web secret back. The parent collects this original child first.
    pub(super) fn read_held(self)->io::Result<PreparedWeb>{
        let stdin=io::stdin();let f=stdin.as_fd();
        let state=tcgetattr(f)?;
        if state.line_discipline!=0||state.input_modes!=InputModes::empty()
            ||state.local_modes!=(LocalModes::ISIG|(state.local_modes&LocalModes::TOSTOP))
            ||state.special_codes[Cc::VMIN]!=0||state.special_codes[Cc::VTIME]!=2||fcntl_getfl(f)?.contains(OFlags::NONBLOCK){
            return Err(refuse("the exact parent-held hidden input mode is not in force"));
        }
        let same=|| {
            if same_mode(&state,&tcgetattr(f)?){Ok(())}else{Err(refuse("parent-held hidden mode changed"))}
        };
        let signals=PromptSignals::begin()?;
        let mut entries=std::array::from_fn::<_,2,_>(|_|Entry::new());
        capture(&mut entries,0,self.selected,f,same,&signals)?;
        signals.check()?;signals.finish()?;
        let [web,_]=entries;PreparedWeb::fresh(web.take(),self.workspace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Test-created devpts pair only. Neither standard input nor the host's
    // actual controlling terminal is used, and no password is requested.
    #[test]
    fn owned_empty_pty_flushes_old_input_reads_without_echo_and_restores_settings() {
        use rustix::{fs::fcntl_setfl, pty::{grantpt, ioctl_tiocgptpeer, openpt, unlockpt, OpenptFlags}};
        let flags = OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC;
        let master=openpt(flags).unwrap(); grantpt(&master).unwrap(); unlockpt(&master).unwrap();
        let slave=ioctl_tiocgptpeer(&master,flags).unwrap();
        let original=tcgetattr(&slave).unwrap();
        assert!(original.local_modes.contains(LocalModes::ICANON | LocalModes::ECHO));
        fcntl_setfl(&master,fcntl_getfl(&master).unwrap() | OFlags::NONBLOCK).unwrap();
        assert_eq!(rustix::io::write(&master,b"stale").unwrap(),5);
        let mut q=Quiet::begin(slave.as_fd()).unwrap();
        let mut buf=[0u8;32]; assert_eq!(rustix::io::read(&slave,&mut buf[..]).unwrap(),0);
        loop {
            match rustix::io::read(&master,&mut buf[..]) {
                Ok(n) if n>0 => (), Err(rustix::io::Errno::AGAIN) => break,
                other => panic!("unexpected disposable master drain: {other:?}"),
            }
        }
        assert_eq!(rustix::io::write(&master,b"x").unwrap(),1);
        assert_eq!(rustix::io::read(&slave,&mut buf[..]).unwrap(),1); assert_eq!(buf[0],b'x');
        assert_eq!(rustix::io::read(&master,&mut buf[..]),Err(rustix::io::Errno::AGAIN));
        // Between two prompts, early bytes are still not echoed. Discard
        // them in the same guard, then accept only new input on this PTY.
        assert_eq!(rustix::io::write(&master,b"early\n").unwrap(),6);
        assert_eq!(rustix::io::read(&master,&mut buf[..]),Err(rustix::io::Errno::AGAIN));
        q.next_prompt().unwrap();assert_eq!(rustix::io::read(&slave,&mut buf[..]).unwrap(),0);
        assert_eq!(rustix::io::write(&master,b"y").unwrap(),1);
        assert_eq!(rustix::io::read(&slave,&mut buf[..]).unwrap(),1);assert_eq!(buf[0],b'y');
        q.restore().unwrap(); assert!(same_mode(&original,&tcgetattr(&slave).unwrap()));
    }
    #[test]
    fn terminal_bytes_are_bounded_and_erasure_clears_the_whole_utf8_character() {
        let mut value=Entry::new();
        for &b in " x中".as_bytes() { assert!(!value.key(b).unwrap()); }
        assert!(!value.key(127).unwrap());
        assert_eq!(value.len, 2); assert_eq!(&value.bytes[2..5], &[0;3]);
        assert!(!value.key(b' ').unwrap()); assert!(value.key(b'\r').unwrap());
        assert!(&**value.take() == b" x ".as_slice());
        let mut limit=Entry::new();
        for _ in 0..MAX { assert!(!limit.key(b'x').unwrap()); }
        assert!(limit.key(b'\n').unwrap()); assert!(limit.key(b'x').is_err());
        let mut invalid=Entry::new(); assert!(invalid.key(b'\n').is_err());
        assert!(invalid.key(0).is_err()); assert!(!invalid.key(0xff).unwrap()); assert!(invalid.key(b'\n').is_err());
    }
}
