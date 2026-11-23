#![forbid(unsafe_code)]

#[cfg(not(all(target_os = "linux", target_arch = "x86_64", target_pointer_width = "64")))]
compile_error!("helper currently supports Linux x86_64 (LP64) only");

mod block_tools;
mod disk_web;
mod embedded_image;
mod foreground;
mod fs_tools;
mod fresh_password;
mod fresh_random;
mod headless_setup;
mod headless_setup_command;
mod hardware;
mod volume_images;
mod inspect;
mod isolated_group;
mod local_worker;
mod smb_profile;
mod maintenance;
mod path_rules;
mod pinned;
pub(crate) mod product_layout;
mod sqlite_install;
#[cfg(test)]
#[path = "../tests/common/mod.rs"]
mod test_area;
#[cfg(test)]
#[path = "../tests/common/child.rs"]
mod fixture_child;
#[cfg(test)]
#[path = "../tests/build/dependency_document.rs"]
mod dependency_document;

use volume_images::CheckFormat;
use pinned::storage_status::Selection;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "helper: local diagnostics and explicit offline pool installation (Linux x86_64)\n\
usage:\n  helper inspect-dirs DIR [DIR ...]\n  \
helper compare-files REFERENCE CANDIDATE\n  helper storage-status --system DIR --recycle DIR --data DIR [--data DIR ...]\n  helper disks [--device MAJOR:MINOR] [--rates]\n  helper hardware\n  helper disk-web [--fat32-copy|--exfat-copy|--ext4-copy|--xfs-copy|--ntfs-copy|--btrfs-copy IMAGE --sha256 HEX]\n  helper disk-web --storage-roots --system DIR --recycle DIR --data DIR [--data DIR ...]\n  helper fs-tools [FORMAT [--sha256 HEX]]\n  helper check-fat32-copy IMAGE --sha256 HEX\n  helper check-exfat-copy IMAGE --sha256 HEX\n  helper check-ext4-copy IMAGE --sha256 HEX\n  helper repair-ext4-copy [web] IMAGE --sha256 HEX --output NEW_DIR\n  helper repair-exfat-copy [web] IMAGE --sha256 HEX --output NEW_DIR\n  helper repair-fat32-copy [web] IMAGE --sha256 HEX --output NEW_DIR\n  helper check-xfs-copy IMAGE --sha256 HEX\n  helper check-ntfs-copy IMAGE --sha256 HEX\n  helper check-btrfs-copy IMAGE --sha256 HEX\n  helper setup-headless check SYSTEM RECYCLE DATA [DATA ...]
  helper setup-headless preview-unit IP:PORT SYSTEM RECYCLE DATA [DATA ...]
  helper setup-headless new SYSTEM RECYCLE DATA [DATA ...] LOGIN
  helper setup-headless web new [--preview-unit IP:PORT] SYSTEM RECYCLE DATA [DATA ...] LOGIN
  helper setup-headless reset SYSTEM RECYCLE DATA [DATA ...]
  helper setup-headless add-data NEW_DATA SYSTEM RECYCLE DATA [DATA ...]
  helper preflight-import SOURCE SYSTEM RECYCLE DATA [DATA ...]
  helper --help | --version\n\n\
Supply already mounted directories; storage-status reports observations, not an approved layout.";

fn usage() -> String {
    let mut help = USAGE.to_owned();
    if cfg!(feature = "embedded-zxs") {
        help.push_str("\n\nOptional image extraction:\n  helper install-zxs-image [--destination-dir DIR] --zxs-sha256 HEX (default /zxs/bin/zxs)");
    }
    help.push_str("\n\nOptional read-only health:\n  helper disk-health --device MAJOR:MINOR (one direct PCIe NVMe namespace)");
    help
}

#[derive(Debug)]
enum Command {
    Help,
    Version,
    Directories(Vec<PathBuf>),
    Compare(PathBuf, PathBuf),
    HeadlessSetup(headless_setup_command::Setup),
    ImportCensus(headless_setup::Preflight),
    Storage(Selection),
    #[cfg(feature="embedded-zxs")]
    EmbeddedInstall(embedded_image::EmbeddedInstall),
    Disks(block_tools::Query),
    DiskHealth(block_tools::HealthQuery),
    Hardware,
    DiskWeb(Option<volume_images::Request>),
    DiskWebStorage(Selection),
    SealedWorker(volume_images::Format,[u8;32]),
    LeasedPanelWorker(volume_images::LeasedFormat,[u8;32]),
    #[cfg(test)]
    RustAccountWorker,
    #[cfg(test)]
    RustVaultWriter,
    #[cfg(test)]
    LayoutSqlWorker,
    #[cfg(test)]
    FirstBrowserWorker,
    CopyRepairBrowserWorker,
    FsTools(fs_tools::Query),
    CopyCheck(volume_images::Request),
    RepairExt4(volume_images::repair::Request),
    RepairExfat(volume_images::repair::Request),
    RepairFat32(volume_images::repair::Request),
}

fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, &'static str> {
    let mut args = args.into_iter();
    let op = args.next().ok_or("expected a command")?;
    if op == "--help" || op == "--version" {
        if args.next().is_some() { return Err("unexpected argument"); }
        return Ok(if op == "--help" { Command::Help } else { Command::Version });
    }
    if op == "preflight-import" { return headless_setup::Preflight::parse(args).map(Command::ImportCensus); }
    if op == "setup-headless" { return headless_setup_command::Setup::parse(args).map(Command::HeadlessSetup); }
    #[cfg(feature="embedded-zxs")]
    if op=="install-zxs-image"{return embedded_image::EmbeddedInstall::parse(args).map(Command::EmbeddedInstall);}
    if op == "disk-health"{return block_tools::HealthQuery::parse(args).map(Command::DiskHealth);}
    if op == "disk-web" {
        return match args.next() {
            None => Ok(Command::DiskWeb(None)),
            Some(arg) if arg == "--storage-roots" => Selection::parse(args).map(Command::DiskWebStorage),
            Some(arg) => {
                let kind = volume_images::panel_format(&arg).ok_or(
                    "disk-web accepts one terminal-selected --fat32-copy, --exfat-copy, --ext4-copy, --xfs-copy, --ntfs-copy or --btrfs-copy IMAGE --sha256 HEX",
                )?;
                volume_images::Request::parse(kind, args).map(|r| Command::DiskWeb(Some(r)))
            }
        };
    }
    #[cfg(test)]
    if op=="__zxs-shared-account-sql-v2"||op=="__zxs-shared-vault-writer-v2"{
        if args.next().is_some(){return Err("invalid internal worker");}
        return Ok(if op=="__zxs-shared-account-sql-v2"{Command::RustAccountWorker}else{Command::RustVaultWriter});
    }
    #[cfg(test)]
    if op=="__first-install-browser-v1"{
        if args.next().is_some(){return Err("unexpected browser worker argument");}
        return Ok(Command::FirstBrowserWorker);
    }
    if op=="__copy-repair-browser-v2"{
        if args.next().is_some(){return Err("unexpected copy-repair browser worker argument");}
        return Ok(Command::CopyRepairBrowserWorker);
    }
    #[cfg(test)]
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
    if op == "repair-ext4-copy" {return volume_images::repair::Request::parse(args).map(Command::RepairExt4);}
    if op == "repair-exfat-copy" {return volume_images::repair::Request::parse_exfat(args).map(Command::RepairExfat);}
    if op == "repair-fat32-copy" {return volume_images::repair::Request::parse_fat32(args).map(Command::RepairFat32);}
    if let Some(format) = volume_images::command_format(&op) {
        return volume_images::Request::parse(format, args).map(Command::CopyCheck);
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
        Command::HeadlessSetup(setup) => return setup.run(),
        Command::ImportCensus(request) => return request.run(),
        Command::DiskWeb(image) => disk_web::serve(image)?,
        Command::DiskWebStorage(s)=>disk_web::serve_storage(s)?,
        Command::DiskHealth(query)=>{let(code,public)=query.run();line(&public)?;return Ok(ExitCode::from(code));}
        #[cfg(test)]
        Command::RustAccountWorker=>return Ok(ExitCode::from(if sqlite_install::rust_account_worker().is_ok(){0}else{2})),
        #[cfg(test)]
        Command::RustVaultWriter=>return Ok(ExitCode::from(if sqlite_install::rust_vault_worker().is_ok(){0}else{2})),
        #[cfg(test)]
        Command::FirstBrowserWorker=>return Ok(ExitCode::from(if sqlite_install::exec_first_browser().is_ok(){0}else{2})),
        Command::CopyRepairBrowserWorker=>return Ok(ExitCode::from(if volume_images::repair::authorization::browser_worker().is_ok(){0}else{2})),
        #[cfg(test)]
        Command::LayoutSqlWorker=>return Ok(ExitCode::from(if sqlite_install::layout_sql_worker().is_ok(){0}else{2})),
        Command::SealedWorker(fmt,digest) => return Ok(ExitCode::from(volume_images::panel::worker(fmt,digest).unwrap_or(2))),
        // Code 2 can describe a structured incomplete report. Internal
        // failure must never accept an already-written packet.
        Command::LeasedPanelWorker(kind,digest) => return Ok(ExitCode::from(volume_images::panel::leased_worker(kind,digest).unwrap_or(3))),
        #[cfg(feature="embedded-zxs")]
        Command::EmbeddedInstall(s)=>{s.run()?;return Ok(ExitCode::SUCCESS);}
        Command::Help => line(&usage())?,
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
        Err(why) => { eprintln!("helper: {why}\n{}", usage()); ExitCode::from(2) }
        Ok(cmd) => match run(cmd) {
            Ok(code) => code,
            Err(e) => { eprintln!("helper: action not confirmed: {e}"); ExitCode::from(2) }
        },
    }
}

#[cfg(test)]
#[test]
fn user_help_only_offers_current_setup() {
    let help = usage();
    assert!(help.contains("setup-headless new"));
    assert!(help.contains("setup-headless web new [--preview-unit IP:PORT]"));
    for name in ["install-pool", "setup-pair", "reset-account", "check-deployment", "discover"] {
        assert!(!help.contains(&format!("helper {name} ")));
        assert!(parse([name.into()]).is_err());
    }
    assert_eq!(
        help.contains("install-zxs-image"),
        cfg!(feature = "embedded-zxs")
    );
    assert!(!help.contains("helper __"));
}

#[cfg(test)]
mod copy_arguments{
    use super::*;
    use volume_images::{Format,LeasedFormat};

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
