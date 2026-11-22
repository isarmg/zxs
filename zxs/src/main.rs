use std::{env, io, process::ExitCode};

fn run() -> io::Result<()> {
    if env::args_os().nth(1).is_some() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput,
            "ZXS starts without arguments; setup and offline maintenance are separate"));
    }
    storage_server::start_original_zxs()
}

fn main() -> ExitCode {
    // SAFETY: first process action before the foreground entry, threads or
    // an fd3/fd4 owner. A selected role never falls back to normal startup.
    let status = unsafe { storage_server::try_supervised_zxs_role() }.unwrap_or_else(run);
    match status {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => { eprintln!("zxs: {error}"); ExitCode::FAILURE }
    }
}
