#![forbid(unsafe_code)]

#[cfg(not(all(target_os = "linux", target_arch = "x86_64", target_pointer_width = "64")))]
compile_error!("helper currently supports Linux x86_64 (LP64) only");

mod block_tools;
mod disk_web;
mod fs_tools;
mod hardware;
mod fat32_check;
mod inspect;
mod isolated_group;
mod smb_profile;
mod maintenance;
mod path_rules;
// Historical byte vectors only; the product has no portable legacy reader.
#[cfg(test)]
mod portable_claim;
#[cfg(test)]
mod portable_genesis;
mod pinned;
mod sqlite_install;
#[cfg(test)]
#[path = "../tests/common/mod.rs"]
mod test_area;

use fat32_check::CheckFormat;
use pinned::storage_status::Selection;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "helper: local diagnostics and explicit offline pool installation (Linux x86_64)\n\
usage:\n  helper inspect-dirs DIR [DIR ...]\n  \
helper compare-files REFERENCE CANDIDATE\n  helper discover [--system DIR]\n  helper check-deployment\n  helper storage-status --system DIR --recycle DIR --data DIR [--data DIR ...]\n  helper check-nonempty-source --init-actor ID --storage-uid N --storage-gid N --system DIR --pool ID --init INIT_OP --source DIR\n  helper check-ordinary-retirement --init-actor ID --storage-uid N --storage-gid N --system DIR --pool ID --init INIT_OP --volume VOLUME_ID --data DIR\n  helper disks [--device MAJOR:MINOR] [--rates]\n  helper hardware\n  helper disk-web [--fat32-copy|--exfat-copy|--ext4-copy|--xfs-copy|--ntfs-copy|--btrfs-copy IMAGE --sha256 HEX]\n  helper disk-web --storage-roots --system DIR --recycle DIR --data DIR [--data DIR ...]\n  helper fs-tools [FORMAT [--sha256 HEX]]\n  helper check-fat32-copy IMAGE --sha256 HEX\n  helper check-exfat-copy IMAGE --sha256 HEX\n  helper check-ext4-copy IMAGE --sha256 HEX\n  helper repair-ext4-copy [web] IMAGE --sha256 HEX --output NEW_DIR\n  helper repair-exfat-copy [web] IMAGE --sha256 HEX --output NEW_DIR\n  helper repair-fat32-copy [web] IMAGE --sha256 HEX --output NEW_DIR\n  helper check-xfs-copy IMAGE --sha256 HEX\n  helper check-ntfs-copy IMAGE --sha256 HEX\n  helper check-btrfs-copy IMAGE --sha256 HEX\n  helper install-pool new|resume|receipt --init-actor ID --storage-uid N --storage-gid N --op ID --approval ID [--system DIR] [--recycle DIR] (--data DIR | --data-label LABEL) ...\n  helper install-pool web new|resume --init-actor ID --storage-uid N --storage-gid N --op ID --approval ID [--system DIR] [--recycle DIR] (--data DIR | --data-label LABEL) ...\n  helper --help | --version\n\n\
install-pool defaults to /zxs/disks/system and /zxs/disks/recycle; choose each ordinary root explicitly.\nSupply already mounted directories; storage-status reports observations, not an approved layout.";

#[derive(Debug)]
enum Command {
    Help,
    Version,
    Directories(Vec<PathBuf>),
    Compare(PathBuf, PathBuf),
    Discover(sqlite_install::Discovery),
    CheckDeployment,
    SharedAccountReset(sqlite_install::SharedAccountReset),
    RetirementCheck(sqlite_install::RetirementCheck),
    ImportCheck(sqlite_install::ImportCheck),
    Storage(Selection),
    Install(sqlite_install::Installation),
    Setup(sqlite_install::Setup),
    #[cfg(feature="embedded-zxs")]
    EmbeddedInstall(sqlite_install::EmbeddedInstall),
    Disks(block_tools::Query),
    Hardware,
    DiskWeb(Option<fat32_check::Request>),
    DiskWebStorage(Selection),
    SealedWorker(fat32_check::Format,[u8;32]),
    LeasedPanelWorker(fat32_check::LeasedFormat,[u8;32]),
    RustAccountWorker,
    RustVaultWriter,
    LayoutSqlWorker,
    FirstBrowserWorker,
    CopyRepairBrowserWorker,
    FsTools(fs_tools::Query),
    CopyCheck(fat32_check::Request),
    RepairExt4(fat32_check::repair::Request),
    RepairExfat(fat32_check::repair::Request),
    RepairFat32(fat32_check::repair::Request),
}

fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, &'static str> {
    let mut args = args.into_iter();
    let op = args.next().ok_or("expected a command")?;
    if op == "--help" || op == "--version" {
        if args.next().is_some() { return Err("unexpected argument"); }
        return Ok(if op == "--help" { Command::Help } else { Command::Version });
    }
    if op=="reset-account"{return sqlite_install::SharedAccountReset::parse(args).map(Command::SharedAccountReset);}
    #[cfg(feature="embedded-zxs")]
    if op=="install-zxs-image"{return sqlite_install::EmbeddedInstall::parse(args).map(Command::EmbeddedInstall);}
    if op == "setup-pair"{return sqlite_install::Setup::parse(args).map(Command::Setup);}
    if op == "install-pool"{return sqlite_install::Installation::parse(args).map(Command::Install);}
    if op == "check-nonempty-source"{return sqlite_install::ImportCheck::parse(args).map(Command::ImportCheck);}
    if op == "check-ordinary-retirement"{return sqlite_install::RetirementCheck::parse(args).map(Command::RetirementCheck);}
    if op == "check-deployment" {
        if args.next().is_some(){return Err("the read-only deployment check takes no paths or options");}
        return Ok(Command::CheckDeployment);
    }
    if op == "discover"{return sqlite_install::Discovery::parse(args).map(Command::Discover);}
    if op == "disk-web" {
        return match args.next() {
            None => Ok(Command::DiskWeb(None)),
            Some(arg) if arg == "--storage-roots" => Selection::parse(args).map(Command::DiskWebStorage),
            Some(arg) => {
                let kind = fat32_check::panel_format(&arg).ok_or(
                    "disk-web accepts one terminal-selected --fat32-copy, --exfat-copy, --ext4-copy, --xfs-copy, --ntfs-copy or --btrfs-copy IMAGE --sha256 HEX",
                )?;
                fat32_check::Request::parse(kind, args).map(|r| Command::DiskWeb(Some(r)))
            }
        };
    }
    if op=="__zxs-shared-account-sql-v2"||op=="__zxs-shared-vault-writer-v2"{
        if args.next().is_some(){return Err("invalid internal worker");}
        return Ok(if op=="__zxs-shared-account-sql-v2"{Command::RustAccountWorker}else{Command::RustVaultWriter});
    }
    if op=="__first-install-browser-v1"{
        if args.next().is_some(){return Err("unexpected browser worker argument");}
        return Ok(Command::FirstBrowserWorker);
    }
    if op=="__copy-repair-browser-v2"{
        if args.next().is_some(){return Err("unexpected copy-repair browser worker argument");}
        return Ok(Command::CopyRepairBrowserWorker);
    }
    if op=="__first-layout-sql-v9"{
        if args.next().is_some(){return Err("invalid internal worker");}
        return Ok(Command::LayoutSqlWorker);
    }
    if let Some(format) = CheckFormat::from_worker_argument(&op) {
        let error = match format {
            CheckFormat::Sealed(_) => "invalid internal worker",
            CheckFormat::Leased(_) => "invalid internal leased worker",
        };
        let expected = args.next().and_then(|s| fs_tools::hex(s.to_str()?)).ok_or(error)?;
        if args.next().is_some() { return Err(error); }
        return Ok(match format {
            CheckFormat::Sealed(kind) => Command::SealedWorker(kind, expected),
            CheckFormat::Leased(kind) => Command::LeasedPanelWorker(kind, expected),
        });
    }
    if op == "inspect-dirs" {
        let dirs: Vec<_> = args.take(inspect::MAX_DIRS + 1).map(PathBuf::from).collect();
        if dirs.is_empty() || dirs.len() > inspect::MAX_DIRS { return Err("expected 1–16 directories"); }
        return Ok(Command::Directories(dirs));
    }
    if op == "repair-ext4-copy" {return fat32_check::repair::Request::parse(args).map(Command::RepairExt4);}
    if op == "repair-exfat-copy" {return fat32_check::repair::Request::parse_exfat(args).map(Command::RepairExfat);}
    if op == "repair-fat32-copy" {return fat32_check::repair::Request::parse_fat32(args).map(Command::RepairFat32);}
    if let Some(format) = fat32_check::command_format(&op) {
        return fat32_check::Request::parse(format, args).map(Command::CopyCheck);
    }
    if op == "fs-tools" { return fs_tools::Query::parse(args).map(Command::FsTools); }
    if op == "hardware" {
        if args.next().is_some() { return Err("hardware takes no paths or options"); }
        return Ok(Command::Hardware);
    }
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
        Command::DiskWebStorage(s)=>disk_web::serve_storage(s)?,
        Command::ImportCheck(check)=>{let(code,public)=check.run();line(&public)?;return Ok(ExitCode::from(code));}
        Command::RetirementCheck(check)=>{let(code,public)=check.run();line(&public)?;return Ok(ExitCode::from(code));}
        Command::SharedAccountReset(i)=>return Ok(ExitCode::from(i.run()?)),
        Command::CheckDeployment=>{let(code,public)=sqlite_install::check_deployment();line(&public)?;return Ok(ExitCode::from(code));}
        Command::Discover(query)=>{let(code,public)=query.run();line(&public)?;return Ok(ExitCode::from(code));}
        Command::RustAccountWorker=>return Ok(ExitCode::from(if sqlite_install::rust_account_worker().is_ok(){0}else{2})),
        Command::RustVaultWriter=>return Ok(ExitCode::from(if sqlite_install::rust_vault_worker().is_ok(){0}else{2})),
        Command::FirstBrowserWorker=>return Ok(ExitCode::from(if sqlite_install::exec_first_browser().is_ok(){0}else{2})),
        Command::CopyRepairBrowserWorker=>return Ok(ExitCode::from(if sqlite_install::exec_copy_repair_browser().is_ok(){0}else{2})),
        Command::LayoutSqlWorker=>return Ok(ExitCode::from(if sqlite_install::layout_sql_worker().is_ok(){0}else{2})),
        Command::SealedWorker(fmt,digest) => return Ok(ExitCode::from(fat32_check::panel::worker(fmt,digest).unwrap_or(2))),
        // Code 2 can describe a structured incomplete report. Internal
        // failure must never accept an already-written packet.
        Command::LeasedPanelWorker(kind,digest) => return Ok(ExitCode::from(fat32_check::panel::leased_worker(kind,digest).unwrap_or(3))),
        Command::Install(i)=>{let(code,public)=i.run()?;line(&public)?;return Ok(ExitCode::from(code));}
        #[cfg(feature="embedded-zxs")]
        Command::EmbeddedInstall(s)=>{s.run()?;return Ok(ExitCode::SUCCESS);}
        Command::Setup(s)=>{let(code,public)=s.run()?;line(&public)?;return Ok(ExitCode::from(code));}
        Command::Help=>{
            line(USAGE)?;
            #[cfg(feature="embedded-zxs")]
            line("Optional exact image only: helper install-zxs-image [--destination-dir DIR] --zxs-sha256 HEX (default /zxs/bin/zxs)")?;
        }
        Command::Version => line(concat!("helper ", env!("CARGO_PKG_VERSION")))?,
        Command::Directories(dirs) => line(&inspect::directories(&dirs)?)?,
        Command::RepairExt4(job) | Command::RepairExfat(job) | Command::RepairFat32(job) => line(&job.run()?)?,
        Command::CopyCheck(job) => {
            let (code, report) = job.run()?;
            line(&report)?;
            return Ok(ExitCode::from(code));
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
        Command::Hardware => {
            let report = hardware::observe();
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

#[cfg(test)]
mod copy_arguments{
    use super::*;
    use fat32_check::{Format,LeasedFormat};

    #[test]
    fn hardware_argv_only_selects_a_fixed_passive_observation() {
        assert!(matches!(parse(["hardware"].map(OsString::from)), Ok(Command::Hardware)));
        for extra in ["--loop", "--write", "/proc/self/status", "/dev/sda"] {
            assert!(parse(["hardware", extra].map(OsString::from)).is_err());
        }
    }

    #[test]
    fn the_repair_worker_gets_no_path_format_or_tool_arguments(){
        let cmd=|a:&[&str]|parse(a.iter().map(|&arg|OsString::from(arg)));
        assert!(matches!(cmd(&["__copy-repair-browser-v2"]),Ok(Command::CopyRepairBrowserWorker)));
        for extra in ["fat32","/saved/volume","--repair"]{
            assert!(cmd(&["__copy-repair-browser-v2",extra]).is_err());
        }
    }
    #[test]
    fn readonly_formats_bind_the_same_choice_in_cli_panel_and_fixed_worker() {
        let digest = "35".repeat(32);
        let cmd = |a: &[&str]| parse(a.iter().copied().map(OsString::from));
        let cases = [
            ("check-fat32-copy", "--fat32-copy", "__fat32-sealed-v1", CheckFormat::Sealed(Format::Fat32), "-a"),
            ("check-exfat-copy", "--exfat-copy", "__exfat-sealed-v1", CheckFormat::Sealed(Format::Exfat), "-p"),
            ("check-ext4-copy", "--ext4-copy", "__ext4-leased-v1", CheckFormat::Leased(LeasedFormat::Ext4), "-y"),
            ("check-xfs-copy", "--xfs-copy", "__xfs-leased-v1", CheckFormat::Leased(LeasedFormat::Xfs), "-L"),
            ("check-ntfs-copy", "--ntfs-copy", "__ntfs-leased-v1", CheckFormat::Leased(LeasedFormat::Ntfs), "-d"),
            ("check-btrfs-copy", "--btrfs-copy", "__btrfs-leased-v1", CheckFormat::Leased(LeasedFormat::Btrfs), "--repair"),
        ];
        for (cli, web, worker, format, forbidden) in cases {
            assert!(matches!(cmd(&[cli, "/saved/volume", "--sha256", &digest]),
                Ok(Command::CopyCheck(r)) if r.format() == format));
            assert!(matches!(cmd(&["disk-web", web, "/saved/volume", "--sha256", &digest]),
                Ok(Command::DiskWeb(Some(r))) if r.format() == format));
            match cmd(&[worker, &digest]).unwrap() {
                Command::SealedWorker(kind, bound) => {
                    assert_eq!(format, CheckFormat::Sealed(kind));
                    assert_eq!(bound, [0x35; 32]);
                }
                Command::LeasedPanelWorker(kind, bound) => {
                    assert_eq!(format, CheckFormat::Leased(kind));
                    assert_eq!(bound, [0x35; 32]);
                }
                other => panic!("not the selected diagnostic worker: {other:?}"),
            }
            assert!(cmd(&[cli, "relative", "--sha256", &digest]).is_err());
            assert!(cmd(&[cli, "/saved/volume", "--sha256", &digest, forbidden]).is_err());
            assert!(cmd(&["disk-web", web, "relative", "--sha256", &digest]).is_err());
            assert!(cmd(&["disk-web", web, "/saved/volume", "--sha256", &digest, forbidden]).is_err());
            assert!(cmd(&["disk-web", web, "/saved/volume", "--sha256", &digest, "--fat32-copy"]).is_err());
            assert!(cmd(&[worker, "/saved/volume", &digest]).is_err());
            assert!(cmd(&[worker, &digest, forbidden]).is_err());
        }
        assert!(cmd(&["check-zfs-copy", "/saved/volume", "--sha256", &digest]).is_err());
        assert!(cmd(&["disk-web", "--zfs-copy", "/saved/volume", "--sha256", &digest]).is_err());
        assert!(cmd(&["disk-web", "--repair-ext4-copy", "/saved/volume", "--sha256", &digest]).is_err());
        assert!(cmd(&["__ext4-sealed-v1", &digest]).is_err());
        assert!(cmd(&["__fat32-leased-v1", &digest]).is_err());
        assert!(cmd(&["__ext4-leased-v1", &digest, "--journal"]).is_err());
        assert!(cmd(&["check-btrfs-copy", "/saved/volume", "--sha256", &digest, "--force"]).is_err());
    }

    #[test]
    fn readonly_argv_keeps_raw_absolute_paths_but_never_accepts_non_text_options_or_digests() {
        use std::os::unix::ffi::OsStringExt;
        let raw_image = OsString::from_vec(b"/saved/\xff".to_vec());
        let digest = OsString::from("35".repeat(32));
        let sha = OsString::from("--sha256");
        assert!(matches!(parse(["check-ext4-copy".into(), raw_image.clone(), sha.clone(), digest.clone()]),
            Ok(Command::CopyCheck(r)) if r.format() == CheckFormat::Leased(LeasedFormat::Ext4)));
        assert!(matches!(parse(["disk-web".into(), "--ext4-copy".into(), raw_image, sha, digest.clone()]),
            Ok(Command::DiskWeb(Some(r))) if r.format() == CheckFormat::Leased(LeasedFormat::Ext4)));
        let bad_option = OsString::from_vec(b"--ext4-copy\xff".to_vec());
        assert!(parse(["disk-web".into(), bad_option, "/saved/volume".into(), "--sha256".into(), digest]).is_err());
        let mut nontext = vec![b'0'; 64];
        nontext[12] = 0xff;
        assert!(parse(["__ext4-leased-v1".into(), OsString::from_vec(nontext)]).is_err());
    }
}
