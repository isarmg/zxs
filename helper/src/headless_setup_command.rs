//! Explicit local first setup. No daemon, server account reuse or credential in argv.
use crate::headless_setup::{FreshSelection, IssueError};
use helper_linux_evidence::prompt_signals::PromptSignals;
use rustix::{
    fs::{OFlags, fcntl_getfl},
    process::{DumpableBehavior, Resource, Rlimit, getpgrp, getsid, getuid, getgid, set_dumpable_behavior, setrlimit},
    termios::{InputModes, LocalModes, OptionalActions, QueueSelector, SpecialCodeIndex, Termios, tcflush,
        tcgetattr, tcgetpgrp, tcgetsid, tcsetattr},
};
use std::{ffi::OsString, io::{self, Write}, net::SocketAddr, os::fd::{AsFd, BorrowedFd}, path::{Path, PathBuf},
    process::ExitCode, time::{Duration, Instant}};
use zeroize::{Zeroize, Zeroizing};

#[derive(Debug)]
pub(crate) struct Setup {
    paths: [PathBuf; 3],
    login: Option<String>,
    web: bool,
    reset: bool,
    preview: Option<SocketAddr>,
}

impl Setup {
    pub(crate) fn parse(args: impl Iterator<Item = OsString>) -> Result<Self, &'static str> {
        let mut args = args.peekable();
        const USAGE: &str = "expected setup-headless check|new|reset SYSTEM DATA RECYCLE [LOGIN], preview-unit IP:PORT SYSTEM DATA RECYCLE, or web new [--preview-unit IP:PORT] SYSTEM DATA RECYCLE LOGIN";
        let mut mode = args.next().ok_or(USAGE)?;
        let web = mode == "web";
        if web { mode = args.next().ok_or(USAGE)?; }
        if web && mode != "new" { return Err(USAGE); }
        let preview_unit = mode == "preview-unit";
        let preview = if preview_unit || web && args.next_if(|word| word.as_os_str() == "--preview-unit").is_some() {
            let listen = args.next().and_then(|arg| arg.to_str()?.parse::<SocketAddr>().ok())
                .filter(|addr| addr.port() != 0).ok_or("expected an explicit numeric HTTP listen address with a nonzero port")?;
            Some(listen)
        } else { None };
        let paths = [
            PathBuf::from(args.next().ok_or(USAGE)?), PathBuf::from(args.next().ok_or(USAGE)?),
            PathBuf::from(args.next().ok_or(USAGE)?),
        ];
        if paths.iter().any(|p| !p.is_absolute()) { return Err(USAGE); }
        let reset = mode == "reset";
        let login = if mode == "check" || reset || preview_unit { None } else if mode == "new" {
            let name = args.next().ok_or(USAGE)?.into_string().map_err(|_| USAGE)?;
            if !crate::headless_setup::login_ok(&name) { return Err("invalid headless login name"); }
            Some(name)
        } else { return Err(USAGE); };
        if args.next().is_some() { return Err(USAGE); }
        Ok(Self { paths, login, web, reset, preview })
    }

    pub(crate) fn run(self) -> io::Result<ExitCode> {
        rustix::thread::set_no_new_privs(true)?;
        crate::maintenance::diagnostic_worker(getuid().as_raw(), getgid().as_raw())?;
        let selected = FreshSelection::select(self.paths.each_ref().map(PathBuf::as_path))?;
        if self.reset {
            let reset = selected.begin_reset()?;
            let [system, data, recycle] = reset.paths();
            let selected_text = format!(
                "Offline password reset for {}\nSYSTEM {system:?}\nDATA {data:?}\nRECYCLE {recycle:?}",
                reset.login());
            let (first, second) = hidden_pair(&selected_text, b"Type RESET to approve: ", b"RESET")?;
            let first = std::str::from_utf8(&first).map_err(|_| invalid("password must be UTF-8"))?;
            let second = std::str::from_utf8(&second).map_err(|_| invalid("password must be UTF-8"))?;
            return match reset.apply(first, second) {
                Ok(()) => {
                    writeln!(io::stdout().lock(), "Password reset confirmed. Login and storage roles are unchanged.")?;
                    Ok(ExitCode::SUCCESS)
                }
                Err(IssueError::Refused(reason)) => Err(reason),
                Err(IssueError::Uncertain(reason)) => {
                    eprintln!("Password reset uncertain; preserve the original state and inspect before retrying ({reason}).");
                    Ok(ExitCode::from(3))
                }
            };
        }
        let current = selected.check_ready()?;
        if !self.web && let Some(listen) = self.preview {
            if !current.is_ready() { return Err(io::Error::other(current.to_string())); }
            let unit = preview_unit(selected.paths(), selected.storage_uid(), listen)?;
            io::stdout().lock().write_all(unit.as_bytes())?;
            return Ok(ExitCode::SUCCESS);
        }
        let Some(login) = self.login else {
            writeln!(io::stdout().lock(), "{current}")?;
            return Ok(if current.is_ready() { ExitCode::SUCCESS } else { ExitCode::from(2) });
        };
        if !current.is_ready() { return Err(io::Error::other(current.to_string())); }
        let input = io::stdin();
        let output = io::stderr();
        let tty = input.as_fd();
        let display = output.as_fd();
        require_terminal(tty, display)?;
        if self.web {
            let unit = self.preview.map(|listen| preview_unit(selected.paths(), selected.storage_uid(), listen)).transpose()?;
            if !crate::disk_web::request_headless_review(unit, || require_terminal(tty, display))? {
                return Err(io::Error::other("setup cancelled; no role changed"));
            }
            require_terminal(tty, display)?;
            let current = selected.check_ready()?;
            if !current.is_ready() { return Err(io::Error::other(current.to_string())); }
        }
        let [system, data, recycle] = selected.paths();
        let selection = format!(
            "Fresh headless setup (no format or service start)\nSYSTEM {system:?}\nDATA {data:?}\nRECYCLE {recycle:?}\nLogin {login}");
        let (first, second) = hidden_pair(&selection, b"Type SETUP to approve: ", b"SETUP")?;
        let first = std::str::from_utf8(&first).map_err(|_| invalid("password must be UTF-8"))?;
        let second = std::str::from_utf8(&second).map_err(|_| invalid("password must be UTF-8"))?;
        match selected.prepare(&login, first, second) {
            Ok(_) => {
                println!("Setup confirmed: the three role markers and server.conf are durable. The server has not started.");
                Ok(ExitCode::SUCCESS)
            }
            Err(IssueError::Refused(reason)) => Err(reason),
            Err(IssueError::Uncertain(reason)) => {
                eprintln!("Setup uncertain: original records may remain; do not overwrite or rerun blindly ({reason}).");
                Ok(ExitCode::from(3))
            }
        }
    }
}

/// Quote one RequiresMountsFor word (systemd unquotes before expanding %).
fn mount_word(path: &Path) -> io::Result<String> {
    let path = path.to_str().filter(|s| !s.chars().any(char::is_control))
        .ok_or_else(|| invalid("systemd preview needs UTF-8 paths without control characters"))?;
    let mut word = String::from("\"");
    for ch in path.chars() {
        match ch {
            '%' => word.push_str("%%"),
            '\\' | '"' => { word.push('\\'); word.push(ch); }
            _ => word.push(ch),
        }
    }
    word.push('"');
    Ok(word)
}

fn preview_unit(paths: [&Path; 3], uid: u32, listen: SocketAddr) -> io::Result<String> {
    let [system, data, recycle] = paths.map(mount_word);
    let (system, data, recycle) = (system?, data?, recycle?);
    Ok(format!(
        "# Preview only. Use after setup, with the ZXS executable installed.\n\
         # Mount ordering by selected path; the server verifies storage identity.\n\
         [Unit]\n\
         Description=ZXS storage server\n\
         RequiresMountsFor={system} {data} {recycle}\n\
         \n\
         [Service]\n\
         Type=simple\n\
         User={uid}\n\
         ExecStart=/zxs/bin/zxs --listen {listen}\n\
         KillMode=mixed\n\
         TimeoutStopSec=infinity\n\
         SendSIGKILL=no\n"
    ))
}

type Secret = Zeroizing<Vec<u8>>;

fn hidden_pair(description: &str, question: &[u8], expected: &[u8]) -> io::Result<(Secret, Secret)> {
    let input = io::stdin();
    let output = io::stderr();
    let tty = input.as_fd();
    let display = output.as_fd();
    require_terminal(tty, display)?;
    setrlimit(Resource::Core, Rlimit { current: Some(0), maximum: Some(0) })?;
    set_dumpable_behavior(DumpableBehavior::NotDumpable)?;
    let mut prompt = output.lock();
    writeln!(prompt, "{description}\nUse a trusted terminal without input recording.")?;
    let signals = PromptSignals::begin()?;
    let mut quiet = Quiet::begin(tty)?;
    let mut answer = read_hidden(&quiet, &signals, display, &mut prompt, question, 16)?;
    if answer.as_slice() != expected { return Err(io::Error::other("operation cancelled; no account changed")); }
    answer.zeroize();
    let first = read_hidden(&quiet, &signals, display, &mut prompt, b"Password: ", crate::fresh_password::MAX_BYTES)?;
    let second = read_hidden(&quiet, &signals, display, &mut prompt, b"Repeat password: ", crate::fresh_password::MAX_BYTES)?;
    quiet.restore()?;
    signals.check()?;
    signals.finish()?;
    Ok((first, second))
}

fn invalid(message: &'static str) -> io::Error { io::Error::new(io::ErrorKind::InvalidInput, message) }

fn require_terminal(input: BorrowedFd<'_>, output: BorrowedFd<'_>) -> io::Result<()> {
    let i = rustix::fs::fstat(input)?;
    let o = rustix::fs::fstat(output)?;
    let session = getsid(None)?;
    let group = getpgrp();
    if (i.st_dev, i.st_ino, i.st_rdev) != (o.st_dev, o.st_ino, o.st_rdev)
        || tcgetsid(input)? != session || tcgetsid(output)? != session
        || tcgetpgrp(input)? != group || tcgetpgrp(output)? != group
        || fcntl_getfl(input)?.contains(OFlags::NONBLOCK)
    { return Err(invalid("stdin and stderr must be the same foreground controlling terminal")); }
    Ok(())
}

struct Quiet<'a> {
    fd: BorrowedFd<'a>,
    old: Termios,
    hidden: Termios,
    active: bool,
}

impl<'a> Quiet<'a> {
    fn begin(fd: BorrowedFd<'a>) -> io::Result<Self> {
        let old = tcgetattr(fd)?;
        if old.line_discipline != 0 || !old.local_modes.contains(LocalModes::ICANON) {
            return Err(invalid("setup requires the ordinary Linux terminal line mode"));
        }
        let mut hidden = old.clone();
        hidden.input_modes = InputModes::empty();
        hidden.local_modes = LocalModes::ISIG | (old.local_modes & LocalModes::TOSTOP);
        hidden.special_codes[SpecialCodeIndex::VMIN] = 0;
        hidden.special_codes[SpecialCodeIndex::VTIME] = 2;
        let armed = Self { fd, old, hidden, active: true };
        tcsetattr(fd, OptionalActions::Now, &armed.hidden)?;
        armed.verify(&armed.hidden)?;
        Ok(armed)
    }

    fn verify(&self, expected: &Termios) -> io::Result<()> {
        let got = tcgetattr(self.fd)?;
        if got.line_discipline != expected.line_discipline
            || got.input_modes != expected.input_modes || got.local_modes != expected.local_modes
            || got.special_codes[SpecialCodeIndex::VMIN] != expected.special_codes[SpecialCodeIndex::VMIN]
            || got.special_codes[SpecialCodeIndex::VTIME] != expected.special_codes[SpecialCodeIndex::VTIME] {
            return Err(invalid("cannot verify terminal input and echo mode"));
        }
        Ok(())
    }

    fn restore(&mut self) -> io::Result<()> {
        if !self.active { return Ok(()); }
        let flush = tcflush(self.fd, QueueSelector::IFlush);
        tcsetattr(self.fd, OptionalActions::Now, &self.old)?;
        self.verify(&self.old)?;
        self.active = false;
        flush.map_err(Into::into)
    }
}

impl Drop for Quiet<'_> {
    fn drop(&mut self) { let _ = self.restore(); }
}

fn read_hidden(
    quiet: &Quiet<'_>, signals: &PromptSignals, display: BorrowedFd<'_>, out: &mut impl Write,
    label: &[u8], bound: usize,
) -> io::Result<Zeroizing<Vec<u8>>> {
    let require = || {
        signals.check()?;
        require_terminal(quiet.fd, display)?;
        quiet.verify(&quiet.hidden)
    };
    require()?;
    tcflush(quiet.fd, QueueSelector::IFlush)?;
    out.write_all(label)?;
    out.flush()?;
    let mut line = Zeroizing::new(Vec::with_capacity(bound));
    let mut byte = Zeroizing::new([0; 1]);
    let until = Instant::now() + Duration::from_secs(120);
    for _ in 0..4096 {
        require()?;
        if Instant::now() >= until { return Err(io::ErrorKind::TimedOut.into()); }
        match rustix::io::read(quiet.fd, &mut byte[..]) {
            Ok(0) => continue,
            Ok(1) => {},
            Err(rustix::io::Errno::INTR) => continue,
            Ok(_) => return Err(invalid("invalid terminal read")),
            Err(e) => return Err(e.into()),
        }
        let key = byte[0];
        byte.zeroize();
        match key {
            b'\n' | b'\r' => { require()?; out.write_all(b"\n")?; return Ok(line); }
            8 | 127 => {
                if !line.is_empty() {
                    let mut start = line.len() - 1;
                    while start > 0 && line[start] & 0xc0 == 0x80 { start -= 1; }
                    line[start..].zeroize();
                    line.truncate(start);
                }
            }
            0..=31 => return Err(invalid("invalid terminal input control")),
            v if line.len() < bound => line.push(v),
            _ => return Err(invalid("terminal input exceeds the setup limit")),
        }
    }
    Err(invalid("too many terminal input events"))
}
