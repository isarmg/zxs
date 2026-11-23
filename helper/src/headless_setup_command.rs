//! Explicit local first setup. No daemon, server account reuse or credential in argv.
use crate::{
    headless_setup::{IssueError, MAX_DATA, SelectedRoles, parse_paths},
    path_rules::QuotedPath,
};
use helper_linux_evidence::prompt_signals::PromptSignals;
use rustix::{
    fs::{OFlags, fcntl_getfl},
    process::{
        DumpableBehavior, Resource, Rlimit, getgid, getpgrp, getsid, getuid, set_dumpable_behavior,
        setrlimit,
    },
    termios::{
        InputModes, LocalModes, OptionalActions, QueueSelector, SpecialCodeIndex, Termios, tcflush,
        tcgetattr, tcgetpgrp, tcgetsid, tcsetattr,
    },
};
use std::{
    ffi::OsString,
    io::{self, Write},
    net::SocketAddr,
    os::{
        fd::{AsFd, BorrowedFd},
        unix::ffi::OsStrExt,
    },
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, Instant},
};
use zeroize::{Zeroize, Zeroizing};

#[derive(Debug)]
pub(crate) struct Setup {
    paths: Vec<PathBuf>,
    action: Action,
}

#[derive(Debug)]
enum Action {
    Check,
    PreviewUnit(SocketAddr),
    New { login: String, review: Review },
    ResetPassword,
    AddData(PathBuf),
    InitData(PathBuf),
    RetireData(PathBuf),
}

#[derive(Clone, Copy, Debug)]
enum Review {
    Terminal,
    Browser(Option<SocketAddr>),
}

impl Setup {
    pub(crate) fn parse(args: impl Iterator<Item = OsString>) -> Result<Self, &'static str> {
        const USAGE: &str = "expected setup-headless check|new|reset SYSTEM RECYCLE DATA [DATA ...] [LOGIN], preview-unit IP:PORT SYSTEM RECYCLE DATA [DATA ...], web new [--preview-unit IP:PORT] SYSTEM RECYCLE DATA [DATA ...] LOGIN, or add-data|init-data|retire-data TARGET SYSTEM RECYCLE DATA [DATA ...]";
        let mut args = args.peekable();
        let mode = args.next().ok_or(USAGE)?;
        let action = match mode.to_str() {
            Some("check") => Action::Check,
            Some("reset") => Action::ResetPassword,
            Some("preview-unit") => Action::PreviewUnit(parse_listen(args.next())?),
            Some("new" | "web") => {
                let review = if mode == "web" {
                    if args.next().is_none_or(|w| w != "new") {
                        return Err(USAGE);
                    }
                    let preview = if args.next_if(|w| w == "--preview-unit").is_some() {
                        Some(parse_listen(args.next())?)
                    } else {
                        None
                    };
                    Review::Browser(preview)
                } else {
                    Review::Terminal
                };
                // Include one extra to reject (not truncate) an oversized roster.
                let mut values: Vec<_> = args.take(MAX_DATA + 4).collect();
                let login = values
                    .pop()
                    .ok_or(USAGE)?
                    .into_string()
                    .map_err(|_| USAGE)?;
                if !crate::headless_setup::login_ok(&login) {
                    return Err("invalid headless login name");
                }
                return Ok(Self {
                    paths: parse_paths(values.into_iter())?,
                    action: Action::New { login, review },
                });
            }
            Some("add-data" | "init-data" | "retire-data") => {
                let target = PathBuf::from(
                    args.next()
                        .ok_or("expected operated DATA directory before current roots")?,
                );
                crate::path_rules::Kind::StorageRoot.check_path(target.as_os_str().as_bytes())?;
                if mode == "add-data" {
                    Action::AddData(target)
                } else if mode == "init-data" {
                    Action::InitData(target)
                } else {
                    Action::RetireData(target)
                }
            }
            _ => return Err(USAGE),
        };
        Ok(Self {
            paths: parse_paths(args)?,
            action,
        })
    }

    pub(crate) fn run(self) -> io::Result<ExitCode> {
        rustix::thread::set_no_new_privs(true)?;
        crate::maintenance::diagnostic_worker(getuid().as_raw(), getgid().as_raw())?;
        let selected = SelectedRoles::select(&self.paths)?;
        match self.action {
            Action::Check => {
                let current = selected.check_ready()?;
                writeln!(io::stdout().lock(), "{current}")?;
                return Ok(if current.is_ready() {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::from(2)
                });
            }
            Action::New { login, review } => return install(selected, &login, review),
            Action::ResetPassword => return reset_password(selected),
            Action::PreviewUnit(listen) => {
                require_ready(&selected)?;
                let unit = preview_unit(selected.paths(), selected.storage_uid(), listen)?;
                io::stdout().lock().write_all(unit.as_bytes())?;
            }
            Action::AddData(path) => {
                selected.add_data(&path)?;
                writeln!(io::stdout().lock(), "Data disk registered.")?;
            }
            Action::InitData(path) => {
                selected.init_data(&path)?;
                writeln!(
                    io::stdout().lock(),
                    "Selected empty DATA layout confirmed; no service started."
                )?;
            }
            Action::RetireData(path) => {
                selected.retire_data(&path)?;
                writeln!(
                    io::stdout().lock(),
                    "Data disk retired; files on disk unchanged."
                )?;
            }
        }
        Ok(ExitCode::SUCCESS)
    }
}

fn parse_listen(arg: Option<OsString>) -> Result<SocketAddr, &'static str> {
    arg.and_then(|v| v.to_str()?.parse::<SocketAddr>().ok())
        .filter(|addr| addr.port() != 0)
        .ok_or("expected an explicit numeric HTTP listen address with a nonzero port")
}

fn require_ready(roles: &SelectedRoles) -> io::Result<()> {
    let readiness = roles.check_ready()?;
    if readiness.is_ready() {
        Ok(())
    } else {
        Err(io::Error::other(readiness.to_string()))
    }
}

fn install(selected: SelectedRoles, login: &str, review: Review) -> io::Result<ExitCode> {
    require_ready(&selected)?;
    let input = io::stdin();
    let output = io::stderr();
    let tty = input.as_fd();
    let display = output.as_fd();
    require_terminal(tty, display)?;
    if let Review::Browser(preview) = review {
        let unit = preview
            .map(|listen| preview_unit(selected.paths(), selected.storage_uid(), listen))
            .transpose()?;
        if !crate::disk_web::request_headless_review(unit, || require_terminal(tty, display))? {
            return Err(io::Error::other("setup cancelled; no role changed"));
        }
        require_terminal(tty, display)?;
        require_ready(&selected)?;
    }
    let selection = format!(
        "Fresh headless setup (no format or service start)\n{}\nLogin {login}",
        summarize_paths(selected.paths())
    );
    let (first, second) = hidden_pair(&selection, b"Type SETUP to approve: ", b"SETUP")?;
    let first = std::str::from_utf8(&first).map_err(|_| invalid("password must be UTF-8"))?;
    let second = std::str::from_utf8(&second).map_err(|_| invalid("password must be UTF-8"))?;
    match selected.prepare(login, first, second) {
        Ok(_) => {
            println!(
                "Setup confirmed: the selected role markers and server.conf are durable. The server has not started."
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(IssueError::Refused(reason)) => Err(reason),
        Err(IssueError::Uncertain(reason)) => {
            eprintln!(
                "Setup uncertain: original records may remain; do not overwrite or rerun blindly ({reason})."
            );
            Ok(ExitCode::from(3))
        }
    }
}

fn reset_password(selected: SelectedRoles) -> io::Result<ExitCode> {
    let session = selected.open_durable()?;
    let selected_text = format!(
        "Offline password reset for {}\n{}",
        session.login(),
        summarize_paths(session.paths())
    );
    let (first, second) = hidden_pair(&selected_text, b"Type RESET to approve: ", b"RESET")?;
    let first = std::str::from_utf8(&first).map_err(|_| invalid("password must be UTF-8"))?;
    let second = std::str::from_utf8(&second).map_err(|_| invalid("password must be UTF-8"))?;
    match session.reset_password(first, second) {
        Ok(()) => {
            writeln!(
                io::stdout().lock(),
                "Password reset confirmed. Login and storage roles are unchanged."
            )?;
            Ok(ExitCode::SUCCESS)
        }
        Err(IssueError::Refused(reason)) => Err(reason),
        Err(IssueError::Uncertain(reason)) => {
            eprintln!(
                "Password reset uncertain; preserve the original state and inspect before retrying ({reason})."
            );
            Ok(ExitCode::from(3))
        }
    }
}

fn summarize_paths<'a>(paths: impl Iterator<Item = &'a Path>) -> String {
    paths
        .enumerate()
        .map(|(i, path)| {
            let path = QuotedPath(path);
            match i {
                0 => format!("SYSTEM {path}"),
                1 => format!("RECYCLE {path}"),
                _ => format!("DATA {} {path}", i - 1),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Quote one `RequiresMountsFor` word (systemd unquotes before expanding %).
fn mount_word(path: &Path) -> io::Result<String> {
    let path = path
        .to_str()
        .filter(|s| !s.chars().any(char::is_control))
        .ok_or_else(|| invalid("systemd preview needs UTF-8 paths without control characters"))?;
    let mut word = String::from("\"");
    for ch in path.chars() {
        match ch {
            '%' => word.push_str("%%"),
            '\\' | '"' => {
                word.push('\\');
                word.push(ch);
            }
            _ => word.push(ch),
        }
    }
    word.push('"');
    Ok(word)
}

fn preview_unit<'a>(
    paths: impl Iterator<Item = &'a Path>,
    uid: u32,
    listen: SocketAddr,
) -> io::Result<String> {
    let mounts = paths
        .map(mount_word)
        .collect::<io::Result<Vec<_>>>()?
        .join(" ");
    Ok(format!(
        "# Preview only. Use after setup, with the ZXS executable installed.\n\
         # Mount ordering by selected path; the server verifies storage identity.\n\
         [Unit]\n\
         Description=ZXS storage server\n\
         RequiresMountsFor={mounts}\n\
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

fn hidden_pair(
    description: &str,
    question: &[u8],
    expected: &[u8],
) -> io::Result<(Secret, Secret)> {
    let input = io::stdin();
    let output = io::stderr();
    let tty = input.as_fd();
    let display = output.as_fd();
    require_terminal(tty, display)?;
    setrlimit(
        Resource::Core,
        Rlimit {
            current: Some(0),
            maximum: Some(0),
        },
    )?;
    set_dumpable_behavior(DumpableBehavior::NotDumpable)?;
    let mut prompt = output.lock();
    writeln!(
        prompt,
        "{description}\nUse a trusted terminal without input recording."
    )?;
    let signals = PromptSignals::begin()?;
    let mut quiet = Quiet::begin(tty)?;
    let mut answer = read_hidden(&quiet, &signals, display, &mut prompt, question, 16)?;
    if answer.as_slice() != expected {
        return Err(io::Error::other("operation cancelled; no account changed"));
    }
    answer.zeroize();
    let first = read_hidden(
        &quiet,
        &signals,
        display,
        &mut prompt,
        b"Password: ",
        crate::fresh_password::MAX_BYTES,
    )?;
    let second = read_hidden(
        &quiet,
        &signals,
        display,
        &mut prompt,
        b"Repeat password: ",
        crate::fresh_password::MAX_BYTES,
    )?;
    quiet.restore()?;
    signals.check()?;
    signals.finish()?;
    Ok((first, second))
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn require_terminal(input: BorrowedFd<'_>, output: BorrowedFd<'_>) -> io::Result<()> {
    let i = rustix::fs::fstat(input)?;
    let o = rustix::fs::fstat(output)?;
    let session = getsid(None)?;
    let group = getpgrp();
    if (i.st_dev, i.st_ino, i.st_rdev) != (o.st_dev, o.st_ino, o.st_rdev)
        || tcgetsid(input)? != session
        || tcgetsid(output)? != session
        || tcgetpgrp(input)? != group
        || tcgetpgrp(output)? != group
        || fcntl_getfl(input)?.contains(OFlags::NONBLOCK)
    {
        return Err(invalid(
            "stdin and stderr must be the same foreground controlling terminal",
        ));
    }
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
            return Err(invalid(
                "setup requires the ordinary Linux terminal line mode",
            ));
        }
        let mut hidden = old.clone();
        hidden.input_modes = InputModes::empty();
        hidden.local_modes = LocalModes::ISIG | (old.local_modes & LocalModes::TOSTOP);
        hidden.special_codes[SpecialCodeIndex::VMIN] = 0;
        hidden.special_codes[SpecialCodeIndex::VTIME] = 2;
        let armed = Self {
            fd,
            old,
            hidden,
            active: true,
        };
        tcsetattr(fd, OptionalActions::Now, &armed.hidden)?;
        armed.verify(&armed.hidden)?;
        Ok(armed)
    }

    fn verify(&self, expected: &Termios) -> io::Result<()> {
        let got = tcgetattr(self.fd)?;
        if got.line_discipline != expected.line_discipline
            || got.input_modes != expected.input_modes
            || got.local_modes != expected.local_modes
            || got.special_codes[SpecialCodeIndex::VMIN]
                != expected.special_codes[SpecialCodeIndex::VMIN]
            || got.special_codes[SpecialCodeIndex::VTIME]
                != expected.special_codes[SpecialCodeIndex::VTIME]
        {
            return Err(invalid("cannot verify terminal input and echo mode"));
        }
        Ok(())
    }

    fn restore(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        let flush = tcflush(self.fd, QueueSelector::IFlush);
        tcsetattr(self.fd, OptionalActions::Now, &self.old)?;
        self.verify(&self.old)?;
        self.active = false;
        flush.map_err(Into::into)
    }
}

impl Drop for Quiet<'_> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn read_hidden(
    quiet: &Quiet<'_>,
    signals: &PromptSignals,
    display: BorrowedFd<'_>,
    out: &mut impl Write,
    label: &[u8],
    bound: usize,
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
        if Instant::now() >= until {
            return Err(io::ErrorKind::TimedOut.into());
        }
        match rustix::io::read(quiet.fd, &mut byte[..]) {
            Ok(0) | Err(rustix::io::Errno::INTR) => continue,
            Ok(1) => {}
            Ok(_) => return Err(invalid("invalid terminal read")),
            Err(e) => return Err(e.into()),
        }
        let key = byte[0];
        byte.zeroize();
        match key {
            b'\n' | b'\r' => {
                require()?;
                out.write_all(b"\n")?;
                return Ok(line);
            }
            8 | 127 => {
                if !line.is_empty() {
                    let mut start = line.len() - 1;
                    while start > 0 && line[start] & 0xc0 == 0x80 {
                        start -= 1;
                    }
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

#[cfg(test)]
mod setup_actions {
    use super::*;

    #[test]
    fn only_fresh_setup_accepts_the_browser_and_login() {
        let command =
            |words: &[&str]| Setup::parse(words.iter().map(OsString::from)).map(|cmd| cmd.action);
        let roles = ["/s", "/r", "/d"];
        let new = |words: &[&str]| command(&[words, &roles, &["login"]].concat());
        assert!(matches!(
            new(&["new"]),
            Ok(Action::New {
                review: Review::Terminal,
                ..
            })
        ));
        assert!(matches!(
            new(&["web", "new"]),
            Ok(Action::New {
                review: Review::Browser(None),
                ..
            })
        ));
        let listen = "127.0.0.1:8123".parse::<SocketAddr>().unwrap();
        assert!(
            matches!(new(&["web", "new", "--preview-unit", "127.0.0.1:8123"]), Ok(Action::New {review: Review::Browser(Some(addr)), ..}) if addr == listen)
        );
        for mode in [
            "reset",
            "check",
            "add-data",
            "retire-data",
            "init-data",
            "preview-unit",
        ] {
            assert!(command(&[&["web", mode], &roles[..]].concat()).is_err());
        }
        assert!(command(&[&["reset"], &roles[..], &["login"]].concat()).is_err());
        assert!(new(&["web", "new", "--preview-unit", "127.0.0.1:0"]).is_err());
        let mut too_many = vec!["new", "/s", "/r"];
        too_many.extend(std::iter::repeat_n("/d", MAX_DATA + 1));
        too_many.push("login");
        assert!(command(&too_many).is_err());
    }
}
