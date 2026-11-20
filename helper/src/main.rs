//! Explicit local diagnostics and offline installation. The loopback-only disk panel starts only by explicit command.
#![forbid(unsafe_code)]

#[cfg(not(all(target_os = "linux", target_arch = "x86_64", target_pointer_width = "64")))]
compile_error!("helper currently supports Linux x86_64 (LP64) only");

mod block_tools;
mod disk_web;
mod fs_tools;
mod fat32_check;
mod inspect;
mod isolated_group;
mod smb_host;
mod maintenance;
mod path_rules;
mod portable_claim;
mod portable_genesis;
mod pinned;
mod sqlite_install;
#[cfg(test)]
#[path = "../tests/common/mod.rs"]
mod test_area;

use pinned::storage_status::Selection;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "helper: local diagnostics and explicit offline pool installation (Linux x86_64)\n\
usage:\n  helper inspect-dirs DIR [DIR ...]\n  \
helper compare-files REFERENCE CANDIDATE\n  helper storage-status --system DIR --recycle DIR --data DIR [--data DIR ...]\n  helper disks [--device MAJOR:MINOR] [--rates]\n  helper disk-web [--fat32-copy|--exfat-copy IMAGE --sha256 HEX]\n  helper fs-tools [FORMAT [--sha256 HEX]]\n  helper check-fat32-copy IMAGE --sha256 HEX\n  helper check-exfat-copy IMAGE --sha256 HEX\n  helper install-pool new|resume --op ID --approval ID --system DIR --recycle DIR --data DIR [--data DIR ...]\n  helper setup-pair provision --create-layout --instance ID --approval ID\n  helper setup-pair provision --system DIR --pool ID --init ID --instance ID --op DOMAIN_OP --approval ID\n  helper setup-pair enroll --system DIR --pool ID --init ID --instance ID --domain-op DOMAIN_OP --op SETUP_OP --approval ID --web-login NAME --smb-login NAME [--receipt-only|--resume]\n  helper setup-pair reset-smb --system DIR --pool ID --init ID --instance ID --domain-op ID --setup-op ID --op ID --approval ID --smb-account ID --smb-login NAME --previous-op ID --previous-generation N [--receipt-only|--resume]\n  helper setup-pair reset-web --system DIR --pool ID --init ID --setup-op ID --op ID --approval ID --web-account ID --previous-op ID --previous-generation N [--receipt-only]\n  helper --help | --version\n\n\
Supply explicit absolute paths; storage-status reports observations, not an approved layout.";

#[derive(Debug)]
enum Command {
    Help,
    Version,
    Directories(Vec<PathBuf>),
    Compare(PathBuf, PathBuf),
    Storage(Selection),
    Install(sqlite_install::Installation),
    Setup(sqlite_install::Setup),
    Disks(block_tools::Query),
    DiskWeb(Option<(fat32_check::Format,fat32_check::Request)>),
    SealedWorker(fat32_check::Format,[u8;32]),
    SamWorker,
    NativeSqlWorker,
    FsTools(fs_tools::Query),
    Fat32(fat32_check::Request),
    Exfat(fat32_check::Request),
}

fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, &'static str> {
    let mut args = args.into_iter();
    let op = args.next().ok_or("expected a command")?;
    if op == "--help" || op == "--version" {
        if args.next().is_some() { return Err("unexpected argument"); }
        return Ok(if op == "--help" { Command::Help } else { Command::Version });
    }
    if op == "setup-pair"{return sqlite_install::Setup::parse(args).map(Command::Setup);}
    if op == "install-pool"{return sqlite_install::Installation::parse(args).map(Command::Install);}
    if op == "disk-web" {
        return match args.next(){
            None=>Ok(Command::DiskWeb(None)),
            Some(arg) if arg=="--fat32-copy"=>fat32_check::Request::parse(args).map(|r|Command::DiskWeb(Some((fat32_check::Format::Fat32,r)))),
            Some(arg) if arg=="--exfat-copy"=>fat32_check::Request::parse_exfat(args).map(|r|Command::DiskWeb(Some((fat32_check::Format::Exfat,r)))),
            _=>Err("disk-web accepts one terminal-selected --fat32-copy or --exfat-copy IMAGE --sha256 HEX"),
        };
    }
    if op=="__native-account-sql-v7"{
        if args.next().is_some(){return Err("invalid internal worker");}
        return Ok(Command::NativeSqlWorker);
    }
    if op=="__sam-fresh-v1"{
        if args.next().is_some(){return Err("invalid internal worker");}
        return Ok(Command::SamWorker);
    }
    if op=="__fat32-sealed-v1"||op=="__exfat-sealed-v1"{
        let format=if op=="__exfat-sealed-v1"{fat32_check::Format::Exfat}else{fat32_check::Format::Fat32};
        let expected=args.next().and_then(|s|fs_tools::hex(s.to_str()?)).ok_or("invalid internal worker")?;
        if args.next().is_some(){return Err("invalid internal worker");}
        return Ok(Command::SealedWorker(format,expected));
    }
    if op == "inspect-dirs" {
        let dirs: Vec<_> = args.take(inspect::MAX_DIRS + 1).map(PathBuf::from).collect();
        if dirs.is_empty() || dirs.len() > inspect::MAX_DIRS { return Err("expected 1–16 directories"); }
        return Ok(Command::Directories(dirs));
    }
    if op == "check-exfat-copy" {return fat32_check::Request::parse_exfat(args).map(Command::Exfat);}
    if op == "check-fat32-copy" { return fat32_check::Request::parse(args).map(Command::Fat32); }
    if op == "fs-tools" { return fs_tools::Query::parse(args).map(Command::FsTools); }
    if op == "disks" { return block_tools::Query::parse(args).map(Command::Disks); }
    if op == "storage-status" { return Selection::parse(args).map(Command::Storage); }
    if op == "compare-files" {
        let a = args.next().ok_or("expected a reference and candidate")?;
        let b = args.next().ok_or("expected a reference and candidate")?;
        if args.next().is_some() { return Err("unexpected argument"); }
        return Ok(Command::Compare(a.into(), b.into()));
    }
    Err("unknown command")
}

fn line(s: &str) -> io::Result<()> {
    let mut out = io::stdout().lock();
    writeln!(out, "{s}")?;
    out.flush()
}

fn run(command: Command) -> io::Result<ExitCode> {
    match command {
        Command::DiskWeb(image) => disk_web::serve(image)?,
        Command::NativeSqlWorker=>return Ok(ExitCode::from(if sqlite_install::native_sql_worker().is_ok(){0}else{2})),
        Command::SamWorker=>return Ok(ExitCode::from(if sqlite_install::sam_worker().is_ok(){0}else{2})),
        Command::SealedWorker(fmt,digest) => return Ok(ExitCode::from(fat32_check::panel::worker(fmt,digest).unwrap_or(2))),
        Command::Install(i)=>line(&i.run()?)?,
        Command::Setup(s)=>{let(code,public)=s.run()?;line(&public)?;return Ok(ExitCode::from(code));}
        Command::Help => line(USAGE)?,
        Command::Version => line(concat!("helper ", env!("CARGO_PKG_VERSION")))?,
        Command::Directories(dirs) => line(&inspect::directories(&dirs)?)?,
        Command::Exfat(job) => {
            let report=job.run_exfat()?;line(&report.to_string())?;return Ok(ExitCode::from(report.exit()));
        }
        Command::Fat32(job) => {
            let outcome=job.run()?;line(&outcome.to_string())?;return Ok(ExitCode::from(outcome.exit()));
        }
        Command::FsTools(query) => {
            let report = query.observe();
            line(&report.to_string())?;
            return Ok(ExitCode::from(report.exit()));
        }
        Command::Disks(query) => {
            let report = query.observe()?;
            line(&report.to_string())?;
            return Ok(if report.complete() { ExitCode::SUCCESS } else { ExitCode::from(2) });
        }
        Command::Storage(selection) => {
            let report = selection.observe();
            line(&report.to_string())?;
            return Ok(if report.all_sampled() { ExitCode::SUCCESS } else { ExitCode::from(2) });
        }
        Command::Compare(a, b) => {
            let observed = inspect::compare(&a, &b)?;
            if observed.equal {
                line(&format!("equal: {} bytes in each distinct regular file", observed.left))?;
            } else {
                line(&format!("different: reference={} bytes candidate={} bytes", observed.left, observed.right))?;
                return Ok(ExitCode::from(1));
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    match parse(std::env::args_os().skip(1)) {
        Err(why) => { eprintln!("helper: {why}\n{USAGE}"); ExitCode::from(2) }
        Ok(cmd) => match run(cmd) {
            Ok(code) => code,
            Err(e) => { eprintln!("helper: action not confirmed: {e}"); ExitCode::from(2) }
        },
    }
}
