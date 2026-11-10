use crate::Result;
use data_serialization::{Deserialize, Serialize};
use platform_io::{
    fs_safety::{
        RelativePath,
        linux::{MountPolicy, OpenAt2Root, SingleLinkRequirement},
    },
    secret::SecretString,
};
use rustix::{
    fs::{OFlags, fcntl_getfl, fcntl_setfl},
    process::{Pid, Signal, kill_process_group},
};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, Metadata},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{FileExt, MetadataExt},
            process::CommandExt,
        },
    },
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

const MAX_EXECUTABLE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_REPLY_BYTES: usize = 16 * 1024;
const COMMAND_BUDGET: Duration = Duration::from_secs(30);
const PIPE_RETRY_INTERVAL: Duration = Duration::from_millis(10);
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct FileStamp {
    device: String,
    inode: String,
    bytes: String,
    uid: u32,
    gid: u32,
    mode: u32,
    links: String,
    modified_seconds: String,
    modified_nanos: String,
    changed_seconds: String,
    changed_nanos: String,
}
impl FileStamp {
    pub(crate) fn of(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev().to_string(),
            inode: metadata.ino().to_string(),
            bytes: metadata.len().to_string(),
            uid: metadata.uid(),
            gid: metadata.gid(),
            mode: metadata.mode(),
            links: metadata.nlink().to_string(),
            modified_seconds: metadata.mtime().to_string(),
            modified_nanos: metadata.mtime_nsec().to_string(),
            changed_seconds: metadata.ctime().to_string(),
            changed_nanos: metadata.ctime_nsec().to_string(),
        }
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Snapshot {
    path: String,
    metadata: FileStamp,
    sha256: String,
}
#[derive(Clone, Copy)]
pub(crate) enum Program {
    Testparm,
    Pdbedit,
    Smbpasswd,
    Systemctl,
    SystemdAnalyze,
    SystemdSysusers,
    Smbd,
}
impl Program {
    fn location(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Testparm => ("bin", "testparm", "/usr/bin/testparm"),
            Self::Pdbedit => ("bin", "pdbedit", "/usr/bin/pdbedit"),
            Self::Smbpasswd => ("bin", "smbpasswd", "/usr/bin/smbpasswd"),
            Self::Systemctl => ("bin", "systemctl", "/usr/bin/systemctl"),
            Self::SystemdAnalyze => ("bin", "systemd-analyze", "/usr/bin/systemd-analyze"),
            Self::SystemdSysusers => ("bin", "systemd-sysusers", "/usr/bin/systemd-sysusers"),
            Self::Smbd => ("sbin", "smbd", "/usr/sbin/smbd"),
        }
    }
}
pub(crate) struct Executable {
    usr: OpenAt2Root,
    parent: OpenAt2Root,
    file: File,
    kind: Program,
    snapshot: Snapshot,
}
impl Executable {
    pub(crate) fn open(kind: Program) -> Result<Self> {
        let (directory, name, path) = kind.location();
        let usr = OpenAt2Root::open(Path::new("/usr"), MountPolicy::AllowNestedMounts)?;
        trusted_directory(usr.directory())?;
        let parent = OpenAt2Root::from_directory(
            usr.open_directory(&RelativePath::new(directory)?)?,
            MountPolicy::AllowNestedMounts,
        )?;
        trusted_directory(parent.directory())?;
        let file = parent.open_file(&RelativePath::new(name)?, false)?;
        let snapshot = snapshot(&file, path)?;
        let result = Self {
            usr,
            parent,
            file,
            kind,
            snapshot,
        };
        result.recheck()?;
        Ok(result)
    }
    pub(crate) fn snapshot(&self) -> Snapshot {
        self.snapshot.clone()
    }
    pub(crate) fn recheck(&self) -> Result<()> {
        let (directory, name, path) = self.kind.location();
        let usr = OpenAt2Root::open(Path::new("/usr"), MountPolicy::AllowNestedMounts)?;
        trusted_directory(usr.directory())?;
        let parent = OpenAt2Root::from_directory(
            usr.open_directory(&RelativePath::new(directory)?)?,
            MountPolicy::AllowNestedMounts,
        )?;
        trusted_directory(parent.directory())?;
        if usr.identity() != self.usr.identity()
            || parent.identity() != self.parent.identity()
            || SingleLinkRequirement::check(parent.open_file(&RelativePath::new(name)?, false)?)?
                != SingleLinkRequirement::check(&self.file)?
            || snapshot(&self.file, path)? != self.snapshot
        {
            return Err(
                "fixed system program changed after preparation; no replacement was executed"
                    .into(),
            );
        }
        Ok(())
    }
    fn command(&self, args: &[&str]) -> Result<Command> {
        self.recheck()?;
        let mut command = Command::new(format!("/proc/self/fd/{}", self.file.as_raw_fd()));
        command
            .arg0(&self.snapshot.path)
            .args(args)
            .env_clear()
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .env("PATH", "/usr/bin:/usr/sbin")
            .env("HOME", "/")
            .current_dir("/")
            .process_group(0)
            .stderr(Stdio::null());
        Ok(command)
    }
    pub(crate) fn capture(&self, args: &[&str]) -> Result<Vec<u8>> {
        let (status, bytes) = self.capture_status(args)?;
        if !status.success() {
            return Err("fixed system command did not report success; it may have taken effect, so inspect its current state before an explicit retry".into());
        }
        Ok(bytes)
    }
    pub(crate) fn capture_status(&self, args: &[&str]) -> Result<(ExitStatus, Vec<u8>)> {
        let child = self
            .command(args)?
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()?;
        let mut joined = Joined::new(child);
        let output = joined
            .child
            .stdout
            .take()
            .expect("invariant: capture stdout is piped");
        let bytes = joined.capture(output)?;
        let status = joined.wait()?;
        self.recheck()?;
        Ok((status, bytes))
    }
    pub(crate) fn password(&self, args: &[&str], password: &SecretString) -> Result<()> {
        if !matches!(self.kind, Program::Smbpasswd) {
            return Err("password input is only admitted to the fixed smbpasswd program".into());
        }
        let child = self
            .command(args)?
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()?;
        let mut joined = Joined::new(child);
        let input = joined
            .child
            .stdin
            .take()
            .expect("invariant: password stdin is piped");
        joined.password(input, password)?;
        let status = joined.wait()?;
        if !status.success() {
            return Err("smbpasswd did not report success; password may have changed and the request must not be automatically replayed".into());
        }
        self.recheck()
    }
}
fn trusted_directory(file: &File) -> Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
        return Err("system program parents must be root-owned directories without group/other write permissions".into());
    }
    Ok(())
}
fn snapshot(file: &File, path: &str) -> Result<Snapshot> {
    SingleLinkRequirement::check(file)?;
    let before = file.metadata()?;
    if before.uid() != 0
        || before.mode() & 0o6022 != 0
        || before.mode() & 0o100 == 0
        || before.len() > MAX_EXECUTABLE_BYTES
    {
        return Err("fixed system executable must be root-owned, executable, non-setid, non-writable by others and within its size bound".into());
    }
    let mut header = [0_u8; 4];
    file.read_exact_at(&mut header, 0)?;
    if header != *b"\x7fELF" {
        return Err(
            "fixed system executable must be an ELF program, not a script or PATH wrapper".into(),
        );
    }
    let mut offset = 0_u64;
    let mut hash = Sha256::new();
    let mut block = [0_u8; 32 * 1024];
    loop {
        let count = file.read_at(&mut block, offset)?;
        if count == 0 {
            break;
        }
        offset += count as u64;
        if offset > MAX_EXECUTABLE_BYTES {
            return Err("system executable exceeded its size bound while being read".into());
        }
        hash.update(&block[..count]);
    }
    let metadata = FileStamp::of(&before);
    if FileStamp::of(&file.metadata()?) != metadata || offset != before.len() {
        return Err("system executable changed during preparation".into());
    }
    Ok(Snapshot {
        path: path.into(),
        metadata,
        sha256: platform_io::lower_hex(&hash.finalize()),
    })
}
struct Joined {
    child: Child,
    started: Instant,
    reaped: bool,
}
impl Joined {
    fn new(child: Child) -> Self {
        Self {
            child,
            started: Instant::now(),
            reaped: false,
        }
    }
    fn remaining(&self) -> io::Result<Duration> {
        let elapsed = self.started.elapsed();
        if elapsed >= COMMAND_BUDGET {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "fixed system command exceeded its 30-second execution budget; preserve any original operation, do not automatically replay it",
            ));
        }
        Ok(COMMAND_BUDGET - elapsed)
    }
    fn pause(&self) -> io::Result<()> {
        std::thread::sleep(PIPE_RETRY_INTERVAL.min(self.remaining()?));
        Ok(())
    }
    fn capture(&self, mut output: ChildStdout) -> Result<Vec<u8>> {
        fcntl_setfl(&output, fcntl_getfl(&output)? | OFlags::NONBLOCK)?;
        let mut bytes = Vec::with_capacity(MAX_REPLY_BYTES);
        let mut block = [0_u8; 4096];
        loop {
            self.remaining()?;
            match output.read(&mut block) {
                Ok(0) => return Ok(bytes),
                Ok(count) => {
                    if count > MAX_REPLY_BYTES - bytes.len() {
                        return Err(io::Error::new(io::ErrorKind::InvalidData,
                            "fixed system query exceeded its reply bound; output was not returned or drained").into());
                    }
                    bytes.extend_from_slice(&block[..count]);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => self.pause()?,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    fn password(&self, mut input: ChildStdin, password: &SecretString) -> Result<()> {
        fcntl_setfl(&input, fcntl_getfl(&input)? | OFlags::NONBLOCK)?;
        for _ in 0..2 {
            for mut bytes in [password.expose().as_bytes(), b"\n".as_slice()] {
                while !bytes.is_empty() {
                    self.remaining()?;
                    match input.write(bytes) {
                        Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero,
                            "fixed password command stopped accepting input; preserve the uncertain operation").into()),
                        Ok(count) => bytes = &bytes[count..],
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => self.pause()?,
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        Err(error) => return Err(error.into()),
                    }
                }
            }
        }
        Ok(())
    }
    fn wait(&mut self) -> Result<ExitStatus> {
        loop {
            self.remaining()?;
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.reaped = true;
                    return Ok(status);
                }
                Ok(None) => self.pause()?,
                Err(error) => {
                    if error.raw_os_error() == Some(rustix::io::Errno::CHILD.raw_os_error()) {
                        self.reaped = true;
                    }
                    return Err(error.into());
                }
            }
        }
    }
}
impl Drop for Joined {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        if let Err(error) = kill_process_group(Pid::from_child(&self.child), Signal::KILL)
            && error != rustix::io::Errno::SRCH
        {
            tracing::error!(%error, "helper system program group could not be terminated; preserving ownership until the child exits");
        }
        if let Err(error) = self.child.kill()
            && error.raw_os_error() != Some(rustix::io::Errno::SRCH.raw_os_error())
        {
            tracing::error!(%error, "helper system program could not be terminated; preserve the original operation");
        }
        if let Err(error) = self.child.wait() {
            tracing::error!(%error, "helper system program could not be reaped; preserve the operation before retrying");
        }
    }
}

#[cfg(test)]
#[path = "../../test/helper/unit/system_program.rs"]
mod tests;
