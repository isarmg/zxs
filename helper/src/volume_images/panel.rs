//! A single terminal-selected operand. The HTTP process has no command builder
//! with user options, and never installs the checker's process-wide limits.
use super::{
    CheckFormat, Format, LeasedFormat, Outcome, Report, Request, btrfs, exfat, ext4,
    image::{ExfatSealed, Sealed},
    leased, ntfs, runner,
    runtime::Runtime,
    xfs,
};
use crate::pinned::{helper_elf::ForegroundElf, invalid};
use rustix::{
    io::fcntl_dupfd_cloexec,
    pipe::{PipeFlags, pipe_with},
    process::{Pid, Signal, getppid, kill_process, parent_process_death_signal},
};
use std::{
    fs::File,
    io::{self, Read, Write},
    process::{Child, Command, Stdio},
};

#[derive(Clone, Copy)]
pub(crate) enum Detail {
    Fat32Clusters(u32),
    Exfat,
    Ext4,
    Xfs,
    Ntfs,
    Btrfs,
}
#[derive(Clone, Copy)]
pub(crate) struct Info {
    pub bytes: u64,
    pub sector: Option<u16>,
    pub detail: Detail,
    pub digest: [u8; 32],
}
impl Info {
    pub(crate) fn format_fields(self) -> (&'static str, String) {
        match self.detail {
            Detail::Fat32Clusters(n) => ("FAT32", n.to_string()),
            Detail::Exfat => ("exFAT", "-".into()),
            Detail::Ext4 => ("ext4", "-".into()),
            Detail::Xfs => ("XFS", "-".into()),
            Detail::Ntfs => ("NTFS", "-".into()),
            Detail::Btrfs => ("Btrfs", "-".into()),
        }
    }
}
// Leased operands are not members of the two-format sealed protocol.
enum Image {
    Fat32(Sealed),
    Exfat(ExfatSealed),
    Leased(LeasedFormat, leased::Selected),
}
enum Expected {
    Fat32(u32),
    Exfat(exfat::Framing, [u8; 32]),
    Leased(LeasedFormat, leased::Selected, [u8; 32]),
}
pub(crate) enum Receipt {
    Fat32(Report),
    Exfat(exfat::Report),
    Ext4(ext4::Report),
    Xfs(xfs::Report),
    Ntfs(ntfs::Report),
    Btrfs(btrfs::Report),
}
impl Receipt {
    pub(crate) fn text(&self) -> String {
        match self {
            Receipt::Exfat(r) => r.panel_text(),
            Receipt::Ext4(r) => r.panel_text(),
            Receipt::Xfs(r) => r.panel_text(),
            Receipt::Ntfs(r) => r.panel_text(),
            Receipt::Btrfs(r) => r.panel_text(),
            Receipt::Fat32(r) => {
                let status = match r.state {
                    Outcome::NoFindingsReported => "上游检查器未报告问题",
                    Outcome::ReportedMessages => "已完成，但上游检查器还给出了提示",
                    Outcome::FindingsOrToolError => "有发现或检查器内部不一致，不能判为正常",
                };
                format!(
                    "FAT32：{status}。文件数：{}；已用簇 / 总簇：{} / {}。只检查这份副本，未修复。",
                    r.files, r.used, r.total
                )
            }
        }
    }
}
pub(crate) struct Operand {
    image: Image,
    expected: [u8; 32],
    helper: ForegroundElf,
}
impl Operand {
    pub(crate) fn capture(job: Request) -> io::Result<Self> {
        let helper = ForegroundElf::local()?;
        let image = match job.format {
            CheckFormat::Sealed(Format::Fat32) => {
                Image::Fat32(Sealed::capture(&job.image, &job.digest)?)
            }
            CheckFormat::Sealed(Format::Exfat) => {
                Image::Exfat(ExfatSealed::capture(&job.image, &job.digest)?)
            }
            CheckFormat::Leased(kind) => Image::Leased(kind, leased::Selected::select(&job.image)?),
        };
        helper.require()?;
        Ok(Self {
            image,
            expected: job.digest,
            helper,
        })
    }
    pub(crate) fn info(&self) -> Info {
        let (bytes, sector, detail) = match &self.image {
            Image::Fat32(s) => {
                let g = s.geometry();
                (
                    g.length,
                    Some(g.sector_bytes),
                    Detail::Fat32Clusters(g.data_clusters),
                )
            }
            Image::Exfat(s) => {
                let g = s.frame();
                (g.length, Some(g.sector), Detail::Exfat)
            }
            Image::Leased(kind, s) => (
                s.len(),
                None,
                match kind {
                    LeasedFormat::Ext4 => Detail::Ext4,
                    LeasedFormat::Xfs => Detail::Xfs,
                    LeasedFormat::Ntfs => Detail::Ntfs,
                    LeasedFormat::Btrfs => Detail::Btrfs,
                },
            ),
        };
        Info {
            bytes,
            sector,
            detail,
            digest: self.expected,
        }
    }
    pub(crate) fn launch(self) -> io::Result<Running> {
        // The authenticated panel already consumed this ticket. Only here
        // does a leased operand open the selected inode for data access.
        let (input, expected) = match self.image {
            Image::Fat32(s) => (
                s.file().try_clone()?,
                Expected::Fat32(s.geometry().data_clusters),
            ),
            Image::Exfat(s) => (
                s.file().try_clone()?,
                Expected::Exfat(s.frame(), self.expected),
            ),
            Image::Leased(kind, s) => (s.reader()?, Expected::Leased(kind, s, self.expected)),
        };
        let (read, write) = pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK)?;
        let mut cmd = Command::new("/no-path-execution-for-helper-worker");
        cmd.env_clear()
            .current_dir("/")
            .stdin(Stdio::from(input))
            .stdout(Stdio::from(File::from(write)))
            .stderr(Stdio::piped());
        helper_linux_evidence::diagnostic_process::require_waitable_children()?;
        self.helper.require()?;
        expected.prepare_exec(&mut cmd, self.helper.descriptor(), &self.expected)?;
        if helper_linux_evidence::diagnostic_process::cancelled() {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let child = cmd.spawn()?;
        // Do not keep an alias of the child's readonly OFD (and therefore
        // its lease) in the HTTP process. Expected holds only the O_PATH.
        drop(cmd);
        Ok(Running {
            child,
            read: File::from(read),
            expected,
        })
    }
}

// Wire over a dedicated server-owned pipe: magic, result kind, 3 big-endian
// counts. A valid packet and a matching *actual* exit status are both required.
// stdout of fsck itself never reaches the panel.
const SIZE: usize = 17;
fn packet(r: &Report) -> [u8; SIZE] {
    let mut p = [0; SIZE];
    p[..4].copy_from_slice(b"FCR1");
    p[4] = match r.state {
        Outcome::NoFindingsReported => 0,
        Outcome::ReportedMessages => 1,
        Outcome::FindingsOrToolError => 2,
    };
    p[5..9].copy_from_slice(&r.files.to_be_bytes());
    p[9..13].copy_from_slice(&r.used.to_be_bytes());
    p[13..17].copy_from_slice(&r.total.to_be_bytes());
    p
}
fn result(p: &[u8], status: Option<i32>, clusters: u32) -> io::Result<Report> {
    let bad = || invalid("no complete result from the selected copy checker");
    if p.len() != SIZE || &p[..4] != b"FCR1" {
        return Err(bad());
    }
    let state = match (p[4], status) {
        (0, Some(0)) => Outcome::NoFindingsReported,
        (1, Some(1)) => Outcome::ReportedMessages,
        (2, Some(1)) => Outcome::FindingsOrToolError,
        _ => return Err(bad()),
    };
    let field = |i: usize| u32::from_be_bytes(p[i..i + 4].try_into().expect("four-byte field"));
    let (files, used, total) = (field(5), field(9), field(13));
    if used > total || total != clusters {
        return Err(bad());
    }
    Ok(Report {
        state,
        files,
        used,
        total,
    })
}

pub(crate) fn worker(format: Format, expected: [u8; 32]) -> io::Result<u8> {
    if parent_process_death_signal()? != Some(Signal::TERM) {
        return Err(invalid("no supervised copy-check parent"));
    }
    let parent = getppid().ok_or_else(|| invalid("no original copy-check parent"))?;
    ForegroundElf::local()?.parent_is_same_image(parent)?;
    runner::prepare_check(CheckFormat::Sealed(format))?;
    let file = File::from(fcntl_dupfd_cloexec(std::io::stdin(), 3)?);
    let mut out = io::stdout().lock();
    let code = match format {
        Format::Fat32 => {
            let image = Sealed::from_sealed(file, &expected)?;
            let report = runner::check(&Runtime::open()?, &image)?;
            out.write_all(&packet(&report))?;
            report.exit()
        }
        Format::Exfat => {
            let image = ExfatSealed::from_sealed(file, &expected)?;
            let report = runner::check_exfat(&Runtime::exfat()?, &image)?;
            out.write_all(&report.packet(&expected))?;
            report.exit()
        }
    };
    out.flush()?;
    Ok(code)
}

pub(crate) fn leased_worker(kind: LeasedFormat, expected: [u8; 32]) -> io::Result<u8> {
    if parent_process_death_signal()? != Some(Signal::TERM) {
        return Err(invalid("no supervised leased copy-check parent"));
    }
    let parent = getppid().ok_or_else(|| invalid("no original leased copy-check parent"))?;
    ForegroundElf::local()?.parent_is_same_image(parent)?;
    runner::prepare_check(CheckFormat::Leased(kind))?;
    let file = File::from(fcntl_dupfd_cloexec(std::io::stdin(), 3)?);
    let mut out = io::stdout().lock();
    // Check the exact installation before obtaining the operand lease.
    let code = match kind {
        LeasedFormat::Ext4 => {
            let runtime = Runtime::ext4()?;
            let image = ext4::Image::from_panel_reader(file, &expected, ext4::read_frame)?;
            let report = runner::check_ext4(&runtime, &image)?;
            image.require()?;
            out.write_all(&report.packet(&expected))?;
            out.flush()?;
            image.require()?;
            report.exit()
        }
        LeasedFormat::Xfs => {
            let runtime = Runtime::xfs()?;
            let image = xfs::Image::from_panel_reader(file, &expected, xfs::read_frame)?;
            let report = runner::check_xfs(&runtime, &image)?;
            image.require()?;
            out.write_all(&report.packet(&expected))?;
            out.flush()?;
            image.require()?;
            report.exit()
        }
        LeasedFormat::Ntfs => {
            let runtime = Runtime::ntfs()?;
            let image = ntfs::Image::from_panel_reader(file, &expected, ntfs::read_frame)?;
            let report = runner::observe_ntfs(&runtime, &image)?;
            image.require()?;
            out.write_all(&report.packet(&expected))?;
            out.flush()?;
            image.require()?;
            report.exit()
        }
        LeasedFormat::Btrfs => {
            let runtime = Runtime::btrfs()?;
            let image = btrfs::Image::from_panel_reader(file, &expected, btrfs::read_frame)?;
            let report = runner::check_btrfs(&runtime, &image)?;
            image.require()?;
            out.write_all(&report.packet(&expected))?;
            out.flush()?;
            image.require()?;
            report.exit()
        }
    };
    Ok(code)
}

impl Expected {
    fn prepare_exec(&self, cmd: &mut Command, helper: &File, digest: &[u8; 32]) -> io::Result<()> {
        use helper_linux_evidence::diagnostic_process::prepare_panel_worker_exec;
        let format = match self {
            Self::Fat32(_) => CheckFormat::Sealed(Format::Fat32),
            Self::Exfat(..) => CheckFormat::Sealed(Format::Exfat),
            Self::Leased(kind, ..) => CheckFormat::Leased(*kind),
        };
        prepare_panel_worker_exec(cmd, helper, format, digest)
    }
    fn size(&self) -> usize {
        match self {
            Self::Fat32(_) => SIZE,
            Self::Exfat(..) => exfat::Report::WIRE_SIZE,
            Self::Leased(LeasedFormat::Ext4, ..) => ext4::Report::WIRE_SIZE,
            Self::Leased(LeasedFormat::Xfs, ..) => xfs::Report::WIRE_SIZE,
            Self::Leased(LeasedFormat::Ntfs, ..) => ntfs::Report::WIRE_SIZE,
            Self::Leased(LeasedFormat::Btrfs, ..) => btrfs::Report::WIRE_SIZE,
        }
    }
    fn decode(&self, bytes: &[u8], status: Option<i32>) -> io::Result<Receipt> {
        // What was selected in the terminal, not a tag supplied by HTTP/the child,
        // chooses the decoder. No attempt to recognize or try another protocol.
        match self {
            Self::Fat32(clusters) => result(bytes, status, *clusters).map(Receipt::Fat32),
            Self::Exfat(frame, sha) => {
                exfat::Report::from_packet(bytes, status, *frame, sha).map(Receipt::Exfat)
            }
            Self::Leased(kind, original, sha) => {
                original.require()?;
                match kind {
                    LeasedFormat::Ext4 => {
                        ext4::Report::from_packet(bytes, status, original.len(), sha)
                            .map(Receipt::Ext4)
                    }
                    LeasedFormat::Xfs => {
                        xfs::Report::from_packet(bytes, status, original.len(), sha)
                            .map(Receipt::Xfs)
                    }
                    LeasedFormat::Ntfs => {
                        ntfs::Report::from_packet(bytes, status, original.len(), sha)
                            .map(Receipt::Ntfs)
                    }
                    LeasedFormat::Btrfs => {
                        btrfs::Report::from_packet(bytes, status, original.len(), sha)
                            .map(Receipt::Btrfs)
                    }
                }
            }
        }
    }
}
pub(crate) struct Running {
    child: Child,
    read: File,
    expected: Expected,
}
impl Running {
    fn waitable(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        loop {
            match self.child.try_wait() {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                r => return r,
            }
        }
    }
    fn read_result(&mut self, exit: std::process::ExitStatus) -> io::Result<Receipt> {
        let cap = self.expected.size() + 1;
        let mut bytes = Vec::with_capacity(cap);
        (&mut self.read).take(cap as u64).read_to_end(&mut bytes)?;
        self.expected.decode(&bytes, exit.code())
    }
    pub(crate) fn poll(&mut self) -> io::Result<Option<Receipt>> {
        let Some(exit) = self.waitable()? else {
            return Ok(None);
        };
        self.read_result(exit).map(Some)
    }
    fn stop_and_wait(&mut self) -> io::Result<std::process::ExitStatus> {
        if let Some(exit) = self.waitable()? {
            return Ok(exit);
        }
        // The single-threaded panel is the sole parent/reaper. This still
        // unreaped child's identity cannot be reused before the wait below.
        let pid = i32::try_from(self.child.id())
            .ok()
            .and_then(Pid::from_raw)
            .ok_or_else(|| invalid("invalid checker child identity"))?;
        match kill_process(pid, Signal::TERM) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => {}
            Err(e) => return Err(e.into()),
        }
        loop {
            match self.child.wait() {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                r => return r,
            }
        }
    }
    pub(crate) fn finish(&mut self) -> io::Result<()> {
        let exit = self.stop_and_wait()?;
        // Merely observing the worker exit (e.g. SIGKILL or code 2 without its receipt) does not
        // prove it observed its sandbox's complete termination and reap.
        self.read_result(exit).map(|_| ())
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.stop_and_wait();
    } // Last-resort cleanup; never used as a positive receipt.
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_code_partial_extra_and_wrong_geometry_never_confirm_a_check() {
        let r = Report {
            state: Outcome::ReportedMessages,
            files: 4,
            used: 8,
            total: 65530,
        };
        let p = packet(&r);
        assert_eq!(result(&p, Some(1), 65530).unwrap().files, 4);
        assert!(result(&p[..16], Some(1), 65530).is_err());
        assert!(result(&[p.as_slice(), &[0]].concat(), Some(1), 65530).is_err());
        assert!(result(&p, None, 65530).is_err());
        assert!(result(&p, Some(0), 65530).is_err());
        assert!(result(&p, Some(1), 65529).is_err());
        let mut bad = p;
        bad[4] = 3;
        assert!(result(&bad, Some(1), 65530).is_err());
        let mut bad = p;
        bad[9..13].copy_from_slice(&65531u32.to_be_bytes());
        assert!(result(&bad, Some(1), 65530).is_err());
    }
}
