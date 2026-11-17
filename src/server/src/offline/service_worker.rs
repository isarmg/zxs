//! One same-executable child, with real/effective service UID/GID, no inherited
//! environment, no shell, no external program and no secrets in argv.
mod encoding;
mod failure;
use crate::offline::service_worker::failure::{Failure, RequestPhase, Stage};
use data_serialization::{Deserialize, Serialize, de::DeserializeOwned};
const MAX_RECORD_BYTES: usize = 64 * 1024;
pub(crate) trait Request: Serialize + DeserializeOwned {
    type Output: Serialize + DeserializeOwned;
    const COMMAND: &'static str;
    const MAX_RESPONSE_BYTES: usize = MAX_RECORD_BYTES;
    fn account(&self) -> &crate::offline::account::Snapshot;
    fn execute(self) -> crate::offline::Result<Self::Output>;
}
/// The root's admitted context and a single domain action cross the same strict
/// pipe boundary. Neither decoding nor this envelope opens a storage owner.
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Envelope<A> {
    pub context: super::managed_layout::Context,
    pub action: A,
}
pub(crate) trait Action: Serialize + DeserializeOwned {
    type Output: Serialize + DeserializeOwned;
    const COMMAND: &'static str;
    fn execute(request: Envelope<Self>) -> Result<Self::Output>;
}
impl<A: Action> Request for Envelope<A> {
    type Output = A::Output;
    const COMMAND: &'static str = A::COMMAND;
    fn account(&self) -> &crate::offline::account::Snapshot {
        &self.context.account
    }
    fn execute(self) -> Result<Self::Output> {
        A::execute(self)
    }
}
#[derive(Serialize, Deserialize)]
#[serialization(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum Reply<T> {
    Success { result: T },
    Failed { message: String },
}
use crate::offline::Result;
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
        let status = self
            .child
            .wait()
            .map_err(|source| Failure::io(Stage::Wait, source))?;
        self.reaped = true;
        Ok(status)
    }
}
impl Drop for Joined {
    fn drop(&mut self) {
        if !self.reaped
            && let Err(error) = self.child.wait()
        {
            tracing::error!(%error, "service worker could not be reaped; inspect retained initialization before retrying");
        }
    }
}
/// Measure with the actual compact pipe codec, without allocating an oversized
/// payload. This is a length observation and cannot emit a successful reply.
pub(crate) fn encoded_fits(value: &impl Serialize, maximum: usize) -> Result<bool> {
    struct Count { remaining: usize, exceeded: bool }
    impl Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            match self.remaining.checked_sub(bytes.len()) {
                Some(left) => { self.remaining = left; Ok(bytes.len()) },
                None => { self.exceeded = true; Err(std::io::Error::other("bounded pipe encoding exceeds its limit")) },
            }
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    let mut count = Count { remaining: maximum, exceeded: false };
    match json_codec::to_writer(&mut count, value) {
        Ok(()) => Ok(true),
        Err(_) if count.exceeded => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// A plan that cannot be inspected after a disconnect is not admissible.
/// Serialize both actual domain envelopes with the same bound as the worker;
/// never estimate escaped lengths or assume their action tags are equally long.
pub(super) fn retained_execution<A: Action, P: Clone>(
    context: &super::managed_layout::Context,
    plan: &P,
    execute: impl FnOnce(P) -> A,
    original: impl FnOnce(P) -> A,
) -> Result<Envelope<A>> {
    let request = Envelope { context: context.clone(), action: execute(plan.clone()) };
    if !encoded_fits(&request, MAX_RECORD_BYTES)? {
        return Err(Failure::RequestBudget {
            phase: RequestPhase::Execution, maximum: MAX_RECORD_BYTES,
        }.into());
    }
    let inspection = Envelope { context: context.clone(), action: original(plan.clone()) };
    if !encoded_fits(&inspection, MAX_RECORD_BYTES)? {
        return Err(Failure::RequestBudget {
            phase: RequestPhase::OriginalInspection, maximum: MAX_RECORD_BYTES,
        }.into());
    }
    Ok(request)
}
pub(crate) fn run<T: Request>(request: T) -> Result<T::Output> {
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("storage coordinator requires root".into());
    }
    let bytes = encoding::frame(&request, MAX_RECORD_BYTES, || Failure::DispatchBudget {
        maximum: MAX_RECORD_BYTES,
    })?;
    let account = request.account();
    // Rust's Unix uid() clears supplementary groups before setuid; the child
    // independently rejects any remaining groups or incorrect real/effective IDs.
    let mut command = Command::new("/proc/self/exe");
    command
        .arg(T::COMMAND)
        .env_clear()
        .current_dir("/")
        .process_group(0)
        .gid(account.gid)
        .uid(account.uid)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let fd = platform_io::service_lease::ServiceLease::host()?.inherit_to(&mut command)?;
    command.arg("--service-lease-fd").arg(fd.to_string());
    let child = command
        .spawn()
        .map_err(|source| Failure::io(Stage::Spawn, source))?;
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
        .take(T::MAX_RESPONSE_BYTES as u64 + 1)
        .read_to_end(&mut collected);
    // Drain an overlong controlled child without unbounded allocations. Never
    // let a blocked stdout keep the child alive after its owner returns.
    let drained = std::io::copy(&mut output, &mut std::io::sink());
    drop(output);
    let status = child.wait()?;
    written.map_err(|source| Failure::io(Stage::WriteRequest, source))?;
    read.map_err(|source| Failure::io(Stage::ReadReply, source))?;
    drained.map_err(|source| Failure::io(Stage::DrainReply, source))?;
    failure::admit_reply(status, collected.len(), T::MAX_RESPONSE_BYTES)?;
    match json_codec::from_slice::<Reply<T::Output>>(&collected)
        .map_err(|_| "invalid storage worker reply; inspect original operation before retrying")?
    {
        Reply::Success { result } => Ok(result),
        Reply::Failed { message } => Err(message.into()),
    }
}
pub(crate) fn entry<T: Request>() -> Result<()> {
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
    let request: T = json_codec::from_slice(&bytes)?;
    let account = request.account();
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
    let reply = match request.execute() {
        Ok(result) => Reply::Success { result },
        Err(error) => Reply::Failed {
            // Storage failures can traverse protected credential backup
            // material. Never expand the native source chain into this wire.
            message: encoding::diagnostic(error.as_ref()),
        },
    };
    let bytes = encoding::frame(&reply, T::MAX_RESPONSE_BYTES, || Failure::ReplyBudget {
        maximum: T::MAX_RESPONSE_BYTES,
    })?;
    let mut output = std::io::stdout().lock();
    output.write_all(&bytes)?;
    output.flush()?;
    Ok(())
}
