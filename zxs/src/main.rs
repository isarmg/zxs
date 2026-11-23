use std::{env, io, process::ExitCode};

fn run() -> io::Result<()> {
    if env::args_os().nth(1).is_some() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput,
            "zxs takes no arguments; start it through the qualified offline deployment"));
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return storage_server::start_headless();
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    Err(io::Error::new(io::ErrorKind::Unsupported, "the native headless entry requires x86-64 Linux"))
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => { eprintln!("zxs: {error}"); ExitCode::FAILURE }
    }
}
