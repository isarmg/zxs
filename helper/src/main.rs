#![forbid(unsafe_code)]

#[cfg(not(all(
    target_os = "linux",
    target_arch = "x86_64",
    target_pointer_width = "64"
)))]
compile_error!("helper currently supports Linux x86_64 (LP64) only");

#[cfg(any(test, feature = "embedded-zxs"))]
mod asset_digest;
#[cfg(test)]
#[path = "../tests/build/dependency_document.rs"]
mod dependency_document;
#[cfg(any(test, feature = "embedded-zxs"))]
mod embedded_image;
#[cfg(any(test, feature = "embedded-zxs"))]
mod foreground;
mod fresh_password;
mod fresh_random;
mod headless_setup;
mod headless_setup_command;
mod local_worker;
mod maintenance;
mod path_rules;
mod pinned;
pub(crate) mod product_layout;
#[cfg(test)]
#[path = "../tests/common/mod.rs"]
mod test_area;

use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;

const USAGE: &str = "helper: explicit offline ZXS setup, maintenance, and import (Linux x86_64)\n\
usage:\n  helper setup-headless check SYSTEM RECYCLE DATA [DATA ...]
  helper setup-headless preview-unit IP:PORT SYSTEM RECYCLE DATA [DATA ...]
  helper setup-headless new SYSTEM RECYCLE DATA [DATA ...] LOGIN
  helper setup-headless reset SYSTEM RECYCLE DATA [DATA ...]
  helper setup-headless add-data NEW_DATA SYSTEM RECYCLE DATA [DATA ...]
  helper setup-headless init-data TARGET SYSTEM RECYCLE DATA [DATA ...]
  helper setup-headless retire-data TARGET SYSTEM RECYCLE DATA [DATA ...]
  helper preflight-import [--show-names | --check-target] SOURCE SYSTEM RECYCLE DATA [DATA ...]
  helper import-one prepare SOURCE RELATIVE NAME TARGET-DATA SYSTEM RECYCLE DATA [DATA ...]
  helper import-one apply SYSTEM RECYCLE DATA [DATA ...]
  helper import-batch prepare SOURCE TARGET-DATA SYSTEM RECYCLE DATA [DATA ...] (-- RELATIVE NAME [RELATIVE NAME ...] | --map0)
  helper import-batch prepare-tree SOURCE TARGET-DATA SYSTEM RECYCLE DATA [DATA ...] [-- RELATIVE NAME ... | --map0]
  helper import-batch show
  helper import-batch apply SYSTEM RECYCLE DATA [DATA ...]
  helper --help | --version\n\n\
Supply already mounted directories in their registered order for maintenance and import.";

fn usage() -> String {
    let mut help = USAGE.to_owned();
    if cfg!(feature = "embedded-zxs") {
        help.push_str("\n\nOptional embedded ZXS installer:\n  helper install-zxs-image [--destination-dir DIR] --zxs-sha256 HEX (default /zxs/bin/zxs)");
    }
    help
}

#[derive(Debug)]
enum Command {
    Help,
    Version,
    HeadlessSetup(headless_setup_command::Setup),
    ImportCensus(headless_setup::Preflight),
    ImportSingle(headless_setup::ImportOne),
    ImportBatch(headless_setup::ImportBatch),
    #[cfg(feature = "embedded-zxs")]
    EmbeddedInstall(embedded_image::EmbeddedInstall),
}

fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, &'static str> {
    let mut args = args.into_iter();
    let op = args.next().ok_or("expected a command")?;
    if op == "--help" || op == "--version" {
        if args.next().is_some() {
            return Err("unexpected argument");
        }
        return Ok(if op == "--help" {
            Command::Help
        } else {
            Command::Version
        });
    }
    if op == "preflight-import" {
        return headless_setup::Preflight::parse(args).map(Command::ImportCensus);
    }
    if op == "import-batch" {
        return headless_setup::ImportBatch::parse(args).map(Command::ImportBatch);
    }
    if op == "import-one" {
        return headless_setup::ImportOne::parse(args).map(Command::ImportSingle);
    }
    if op == "setup-headless" {
        return headless_setup_command::Setup::parse(args).map(Command::HeadlessSetup);
    }
    #[cfg(feature = "embedded-zxs")]
    if op == "install-zxs-image" {
        return embedded_image::EmbeddedInstall::parse(args).map(Command::EmbeddedInstall);
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
        Command::ImportSingle(request) => return request.run(),
        Command::ImportBatch(request) => return request.run(),
        #[cfg(feature = "embedded-zxs")]
        Command::EmbeddedInstall(s) => {
            s.run()?;
            return Ok(ExitCode::SUCCESS);
        }
        Command::Help => line(&usage())?,
        Command::Version => line(concat!("helper ", env!("CARGO_PKG_VERSION")))?,
    }
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    match parse(std::env::args_os().skip(1)) {
        Err(why) => {
            eprintln!("helper: {why}\n{}", usage());
            ExitCode::from(2)
        }
        Ok(cmd) => match run(cmd) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("helper: action not confirmed: {e}");
                ExitCode::from(2)
            }
        },
    }
}

#[cfg(test)]
#[test]
fn user_help_only_offers_current_setup() {
    let help = usage();
    assert!(help.contains("setup-headless check"));
    assert!(help.contains("setup-headless new"));
    for args in [
        &["setup-headless", "check", "/system", "/recycle", "/data"][..],
        &[
            "setup-headless",
            "new",
            "/system",
            "/recycle",
            "/data",
            "operator",
        ][..],
    ] {
        assert!(matches!(
            parse(args.iter().copied().map(OsString::from)),
            Ok(Command::HeadlessSetup(_))
        ));
    }
    assert!(!help.contains("setup-headless web"));
    for name in [
        "install-pool",
        "setup-pair",
        "reset-account",
        "check-deployment",
        "discover",
    ] {
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
mod arguments {
    use super::*;

    #[test]
    fn retired_host_observations_are_neither_advertised_nor_parsed() {
        let help = usage();
        for (name, old_arguments) in [
            ("inspect-dirs", &["/selected"][..]),
            ("compare-files", &["/reference", "/candidate"][..]),
            (
                "storage-status",
                &[
                    "--system",
                    "/system",
                    "--recycle",
                    "/recycle",
                    "--data",
                    "/data",
                ][..],
            ),
            ("disks", &["--device", "259:1", "--rates"][..]),
            ("hardware", &[][..]),
            ("disk-health", &["--device", "259:1"][..]),
        ] {
            assert!(!help.contains(&format!("helper {name}")), "{name}");
            let args = std::iter::once(name).chain(old_arguments.iter().copied());
            assert_eq!(
                parse(args.map(OsString::from)).unwrap_err(),
                "unknown command",
                "{name}"
            );
        }
    }

    #[test]
    fn retired_tooling_has_no_commands_or_private_worker_entrypoints() {
        let reject = |name: &str, args: &[&str]| {
            let input = std::iter::once(name).chain(args.iter().copied());
            assert!(!usage().contains(name), "{name}");
            assert_eq!(
                parse(input.map(OsString::from)).unwrap_err(),
                "unknown command",
                "{name}"
            );
        };
        let digest = "35".repeat(32);
        for name in ["disk-web", "fs-tools", "__copy-repair-browser-v2"] {
            reject(name, &[]);
        }
        for name in [
            "check-fat32-copy",
            "check-exfat-copy",
            "check-ext4-copy",
            "check-xfs-copy",
            "check-ntfs-copy",
            "check-btrfs-copy",
        ] {
            reject(name, &["/saved/volume", "--sha256", &digest]);
        }
        for name in ["repair-fat32-copy", "repair-exfat-copy", "repair-ext4-copy"] {
            reject(
                name,
                &["/saved/volume", "--sha256", &digest, "--output", "/new-dir"],
            );
        }
        for name in [
            "__fat32-sealed-v1",
            "__exfat-sealed-v1",
            "__ext4-leased-v1",
            "__xfs-leased-v1",
            "__ntfs-leased-v1",
            "__btrfs-leased-v1",
        ] {
            reject(name, &[&digest]);
        }
    }
}
