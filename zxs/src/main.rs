use std::{env, ffi::OsStr, io, net::SocketAddr, process::ExitCode};

fn listen_address() -> io::Result<SocketAddr> {
    let usage = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: zxs --listen IP:PORT (numeric IP, explicit nonzero port)",
        )
    };
    let mut args = env::args_os().skip(1);
    if args.next().as_deref() != Some(OsStr::new("--listen")) {
        return Err(usage());
    }
    let address = args
        .next()
        .and_then(|raw| raw.to_str()?.parse::<SocketAddr>().ok())
        .filter(|address| address.port() != 0)
        .ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }
    Ok(address)
}

fn run() -> io::Result<()> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        let mut args = env::args_os().skip(1);
        match args.next().as_deref().and_then(OsStr::to_str) {
            Some(operation @ ("__offline-live-names-v1" | "__offline-import-one-v1")) => {
                if args.next().is_some() {
                    return Err(io::ErrorKind::InvalidInput.into());
                }
                return if operation == "__offline-live-names-v1" {
                    storage_server::offline_live_names_v1()
                } else {
                    storage_server::offline_import_one_v1()
                };
            }
            _ => {}
        }
    }
    let listen = listen_address()?;
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return storage_server::start_headless(listen);
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    {
        let _ = listen;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the native storage service requires x86-64 Linux",
        ))
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("zxs: {error}");
            ExitCode::FAILURE
        }
    }
}
