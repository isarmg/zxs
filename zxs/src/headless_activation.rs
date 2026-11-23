//! Privileged, one-shot composition from an already qualified host selection.
use super::{
    ApprovedNginx, ApprovedService, LaunchFds, bad, launch_approved, launch_service, live,
    protected_dir, protected_file,
};
use std::{
    io::{self, Read},
    os::fd::AsRawFd,
};

const ENROLLMENT: &str = "/etc/zxs-gateway/activation.v1";
const LIMIT: u64 = 256;

fn sha(value: &str) -> io::Result<[u8; 32]> {
    fn digit(byte: u8) -> io::Result<u8> {
        match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            _ => Err(bad()),
        }
    }
    if value.len() != 64 {
        return Err(bad());
    }
    let mut answer = [0; 32];
    for (out, pair) in answer.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        *out = digit(pair[0])? << 4 | digit(pair[1])?;
    }
    if answer == [0; 32] {
        return Err(bad());
    }
    Ok(answer)
}

fn identity(value: &str) -> io::Result<u32> {
    if value.is_empty() || value.starts_with('0') || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    let value = value.parse::<u32>().map_err(|_| bad())?;
    if matches!(value, 0 | 65534 | u32::MAX) {
        return Err(bad());
    }
    Ok(value)
}

fn enrollment() -> io::Result<(ApprovedNginx, ApprovedService)> {
    use std::os::unix::fs::MetadataExt;
    for path in ["/etc", "/etc/zxs-gateway"] {
        protected_dir(path)?;
    }
    let file = protected_file(ENROLLMENT, LIMIT)?;
    let first = file.metadata()?;
    if first.mode() & 0o7777 != 0o400 {
        return Err(bad());
    }
    let mut bytes = Vec::new();
    (&file).take(LIMIT + 1).read_to_end(&mut bytes)?;
    let named = protected_file(ENROLLMENT, LIMIT)?.metadata()?;
    let stamp = |m: &std::fs::Metadata| {
        (
            m.dev(),
            m.ino(),
            m.len(),
            m.mode(),
            m.ctime(),
            m.ctime_nsec(),
            m.mtime(),
            m.mtime_nsec(),
        )
    };
    if bytes.len() as u64 != first.len()
        || stamp(&first) != stamp(&file.metadata()?)
        || stamp(&first) != stamp(&named)
    {
        return Err(bad());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| bad())?;
    let mut fields = text.strip_suffix('\n').ok_or_else(bad)?.split('\n');
    if fields.next() != Some("ZXS-HOST-ACTIVATION-1") {
        return Err(bad());
    }
    let mut next = || fields.next().ok_or_else(bad);
    let nginx = ApprovedNginx {
        executable_sha: sha(next()?)?,
        configuration_sha: sha(next()?)?,
        worker_uid: identity(next()?)?,
    };
    let service = ApprovedService {
        image_sha: sha(next()?)?,
        uid: identity(next()?)?,
        ipc_gid: identity(next()?)?,
    };
    if fields.next().is_some() || service.uid == nginx.worker_uid {
        return Err(bad());
    }
    Ok((nginx, service))
}

fn ask_gateway_to_stop(gateway: &LaunchFds) -> io::Result<()> {
    // SAFETY: PIDFD names the directly owned original process. The kernel
    // receives a null siginfo and no privileges are inferred from a PID.
    let status = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            gateway.original_pidfd.as_raw_fd(),
            libc::SIGQUIT,
            std::ptr::null::<libc::siginfo_t>(),
            0u32,
        )
    };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(crate) fn activate() -> io::Result<()> {
    // SAFETY: only observes effective identity, does not change it.
    if unsafe { libc::geteuid() } != 0 {
        return Err(bad());
    }
    let (nginx, service) = enrollment()?;
    let (_gateway_child, fds) = launch_approved(&nginx)?;
    match launch_service(&service, &fds) {
        Ok(_service_child) if live(&fds.original_pidfd).is_ok() => Ok(()),
        Ok(mut service_child) => {
            let _ = service_child.kill();
            let _ = service_child.wait();
            Err(bad())
        }
        Err(original) => {
            if let Err(stop) = ask_gateway_to_stop(&fds) {
                if live(&fds.original_pidfd).is_ok() {
                    return Err(io::Error::other(format!(
                        "the service could not start ({original}); the original gateway could not be stopped ({stop})"
                    )));
                }
            }
            Err(original)
        }
    }
}
