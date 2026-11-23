use std::{env, io, process::ExitCode};

fn main() -> ExitCode {
    if env::args_os().nth(1).is_some() {
        eprintln!("activate-headless takes no arguments");
        return ExitCode::FAILURE;
    }
    match activate() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("activate-headless: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn activate() -> io::Result<()> {
    storage_server::activate_headless()
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn activate() -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}
