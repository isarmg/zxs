//! One same-executable child, with real/effective service UID/GID, no inherited
//! environment, no shell, no external program and no secrets in argv.
use super::{MAX_RECORD_BYTES, WorkerOutput, WorkerReply, WorkerRequest};
use crate::Result;
use std::{
    io::{Read, Write},
    os::unix::process::CommandExt,
    process::{Child, Command, Stdio},
};

// Always join an admitted child, even after a pipe/protocol failure. A shutdown
// or disconnected HTTP request must not abandon initialization in the background.
struct Joined {
    child: Child,
    reaped: bool,
}
impl Joined {
    fn wait(&mut self) -> Result<std::process::ExitStatus> {
        let status = self.child.wait()?;
        self.reaped = true;
        Ok(status)
    }
}
impl Drop for Joined {
    fn drop(&mut self) {
        if !self.reaped {
            if let Err(error) = self.child.wait() {
                tracing::error!(%error, "first-storage worker could not be reaped; inspect retained initialization before retrying");
            }
        }
    }
}
pub(super) fn run(request: WorkerRequest) -> Result<WorkerOutput> {
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("storage coordinator requires root".into());
    }
    let bytes = json_codec::to_vec(&request)?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err("first-storage worker request exceeds its bound".into());
    }
    let account = &request.context.account;
    // Rust's Unix uid() clears supplementary groups before setuid; the child
    // independently rejects any remaining groups or incorrect real/effective IDs.
    let child = Command::new("/proc/self/exe")
        .arg("internal-storage-worker")
        .env_clear()
        .current_dir("/")
        .process_group(0)
        .gid(account.gid)
        .uid(account.uid)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut child = Joined {
        child,
        reaped: false,
    };
    let mut input = child
        .child
        .stdin
        .take()
        .expect("invariant: worker stdin was requested as a pipe");
    let written = input.write_all(&bytes);
    drop(input); // EOF, including on a failed/partial write.
    let mut output = child
        .child
        .stdout
        .take()
        .expect("invariant: worker stdout was requested as a pipe");
    let mut collected = Vec::new();
    let read = (&mut output)
        .take(MAX_RECORD_BYTES as u64 + 1)
        .read_to_end(&mut collected);
    // Drain an overlong controlled child without unbounded allocations. Never
    // let a blocked stdout keep the child alive after its owner returns.
    let drained = std::io::copy(&mut output, &mut std::io::sink());
    drop(output);
    let status = child.wait()?;
    written?;
    read?;
    drained?;
    if !status.success() || collected.len() > MAX_RECORD_BYTES {
        return Err("storage worker failed or exceeded its bounded reply; preserve original intent, no rollback is claimed".into());
    }
    match json_codec::from_slice::<WorkerReply>(&collected)
        .map_err(|_| "invalid storage worker reply; inspect original operation before retrying")?
    {
        WorkerReply::Success { result } => Ok(result),
        WorkerReply::Failed { message } => Err(message.into()),
    }
}
pub(crate) fn entry() -> Result<()> {
    if rustix::process::getuid().as_raw() == 0 || rustix::process::geteuid().as_raw() == 0 {
        return Err(
            "internal storage worker must already run as the selected non-root service account"
                .into(),
        );
    }
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(MAX_RECORD_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err("storage worker request exceeds its bound".into());
    }
    let request: WorkerRequest = json_codec::from_slice(&bytes)?;
    let account = &request.context.account;
    if (
        rustix::process::getuid().as_raw(),
        rustix::process::geteuid().as_raw(),
        rustix::process::getgid().as_raw(),
        rustix::process::getegid().as_raw(),
    ) != (account.uid, account.uid, account.gid, account.gid)
        || !rustix::process::getgroups()?.is_empty()
    {
        return Err("worker real/effective identities or supplementary groups differ; no storage was opened".into());
    }
    let reply = match super::worker::execute(request) {
        Ok(result) => WorkerReply::Success { result },
        Err(error) => WorkerReply::Failed {
            message: error.to_string(),
        },
    };
    let bytes = json_codec::to_vec(&reply)?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err("storage worker reply exceeds its bound".into());
    }
    let mut output = std::io::stdout().lock();
    output.write_all(&bytes)?;
    output.flush()?;
    Ok(())
}
