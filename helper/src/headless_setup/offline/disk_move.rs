//! Fixed Helper bridge for one offline DATA move. ZXS owns its durable journal and routes.
use super::{FIXED_ROOTS, OfflineSession, SelectedRoles, parse_id, refused, require_no_stage};
use crate::{
    headless_setup::{parse_paths, random},
    local_worker::{
        Worker,
        transport::{self, Channel, Kind, Payload},
    },
    maintenance::diagnostic_worker,
    path_rules,
};
use rustix::{
    net::UCred,
    process::{getegid, geteuid, getgid, getuid},
};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs::File,
    io::{self, Write},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    process::ExitCode,
};

const ROLE_DOMAIN: &[u8] = b"ZXS disk move live roles v1\0";
const OPERAND_DOMAIN: &[u8] = b"ZXS disk move operand v1\0";
const ORDER_MAGIC: &[u8; 8] = b"ZXDMV1\0\0";
const ORDER_BYTES: usize = 168;
type Hash = [u8; 32];
type Id = [u8; 16];

#[derive(Debug)]
pub(crate) struct DiskMove {
    request: Request,
    roots: Vec<PathBuf>,
}

impl DiskMove {
    pub(crate) fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Self, &'static str> {
        let mut id = || {
            args.next()
                .and_then(|raw| raw.to_str().and_then(parse_id))
                .ok_or("disk move requires nonzero 32-digit lowercase hexadecimal CID and OP_ID")
        };
        let container = id()?;
        let operation = id()?;
        let mut path = || {
            let raw = args
                .next()
                .ok_or("disk move requires SOURCE_DATA and TARGET_DATA")?;
            operand(raw.as_bytes())
        };
        let source = path()?;
        let target = path()?;
        if source == target {
            return Err("disk move requires different source and target operands");
        }
        let roots = parse_paths(args)?;
        Ok(Self {
            request: Request {
                container,
                operation,
                source,
                target,
            },
            roots,
        })
    }

    pub(crate) fn run(self) -> io::Result<ExitCode> {
        rustix::thread::set_no_new_privs(true)?;
        let (uid, gid) = (getuid(), getgid());
        if uid != geteuid() || gid != getegid() {
            return Err(refused());
        }
        diagnostic_worker(uid.as_raw(), gid.as_raw())?;

        let guard = self.request.guard_name();
        // Only this exact active name can reach Core's own native guard proof.
        // Other descriptor stages still block before any mutation or shared sync.
        let session = SelectedRoles::select(&self.roots)?.open_offline(Some(&guard))?;
        let completion = session.with_rechecked_installed(|executable, require_installed| {
            complete(&session, executable, require_installed, self.request)
        })?;
        let mut output = io::stdout().lock();
        completion.write(&mut output, self.request)?;
        output.flush()?;
        Ok(ExitCode::SUCCESS)
    }
}

/// Lexical input only: do not open, normalize or resolve either path here.
fn operand(raw: &[u8]) -> Result<Hash, &'static str> {
    if raw.len() > 4095 {
        return Err("disk move operand exceeds 4095 bytes");
    }
    path_rules::Kind::StorageRoot.check_path(raw)?;
    let len = u16::try_from(raw.len()).map_err(|_| "invalid disk move operand extent")?;
    let mut hash = Sha256::new();
    hash.update(OPERAND_DOMAIN);
    hash.update(len.to_be_bytes());
    hash.update(raw);
    Ok(hash.finalize().into())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Request {
    container: Id,
    operation: Id,
    source: Hash,
    target: Hash,
}

impl Request {
    fn encode(self, binding: &Hash, scope: &Hash) -> io::Result<[u8; ORDER_BYTES]> {
        if *scope == [0; 32]
            || self.container == [0; 16]
            || self.operation == [0; 16]
            || self.source == [0; 32]
            || self.target == [0; 32]
            || self.source == self.target
        {
            return Err(refused());
        }
        let mut bytes = [0; ORDER_BYTES];
        bytes[..8].copy_from_slice(ORDER_MAGIC);
        bytes[8..40].copy_from_slice(binding);
        bytes[40..72].copy_from_slice(scope);
        bytes[72..88].copy_from_slice(&self.container);
        bytes[88..104].copy_from_slice(&self.operation);
        bytes[104..136].copy_from_slice(&self.source);
        bytes[136..168].copy_from_slice(&self.target);
        Ok(bytes)
    }

    fn guard_name(self) -> String {
        format!(
            ".server-stage-move-{}-{}",
            hex(&self.container),
            hex(&self.operation)
        )
    }

    /// Compare only already-held DATA names. A historical DONE may have neither.
    fn current_pair<'a>(self, names: impl Iterator<Item = &'a Path>) -> io::Result<bool> {
        let (mut source, mut target) = (false, false);
        for path in names {
            let hash = operand(path.as_os_str().as_bytes()).map_err(|_| refused())?;
            let seen = if hash == self.source {
                &mut source
            } else if hash == self.target {
                &mut target
            } else {
                continue;
            };
            if *seen {
                return Err(refused());
            }
            *seen = true;
        }
        Ok(source && target)
    }
}

fn complete(
    session: &OfflineSession,
    executable: &File,
    require_installed: &dyn Fn() -> io::Result<()>,
    request: Request,
) -> io::Result<Completion> {
    let binding = session.role_binding(ROLE_DOMAIN)?;
    let pair_enrolled = request.current_pair(session.paths().skip(FIXED_ROOTS))?;
    let scope = random::<32>()?;
    let raw = request.encode(&binding, &scope)?;
    let digest = Sha256::digest(raw).into();
    let order = transport::seal_import_ticket(&raw)?;
    require_installed()?;
    let (local, peer) = transport::pair()?;
    let mut worker = Worker::spawn_data_move(executable, &peer, &order, &session.locked)?;
    drop(peer);
    drop(order);
    let channel = Channel::from_owned_child_session(
        local,
        UCred {
            uid: getuid(),
            gid: getgid(),
            pid: worker.pid,
        },
        scope,
    )?;
    let body = channel.wait_offline(Kind::DataMove, &digest, &worker)?;
    let completion = Completion::decode(body, request, pair_enrolled)?;
    worker.finish_data_move(&channel)?;
    require_installed()?;
    if session.role_binding(ROLE_DOMAIN)? != binding {
        return Err(refused());
    }
    require_no_stage(&session.locked, None)?;
    require_installed()?;
    Ok(completion)
}

#[derive(Debug, Eq, PartialEq)]
enum Completion {
    Completed(Hash),
    Historical(Hash),
}

impl Completion {
    fn decode(packet: Payload, request: Request, pair_enrolled: bool) -> io::Result<Self> {
        if packet[1..17] != request.container || packet[17..33] != request.operation {
            return Err(refused());
        }
        let done = packet[33..65].try_into().map_err(|_| refused())?;
        match packet[0] {
            1 if pair_enrolled => Ok(Self::Completed(done)),
            2 => Ok(Self::Historical(done)),
            _ => Err(refused()),
        }
    }

    fn write(&self, output: &mut impl Write, request: Request) -> io::Result<()> {
        let container = hex(&request.container);
        let operation = hex(&request.operation);
        match self {
            Self::Completed(done) => writeln!(
                output,
                "moved container {container}: operation={operation} done={}",
                hex(done)
            ),
            Self::Historical(done) => writeln!(
                output,
                "historical move completion for {container}: operation={operation} done={}; current location not checked",
                hex(done)
            ),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(text, "{byte:02x}").expect("String writes are infallible");
    }
    text
}

#[cfg(test)]
#[path = "../../../tests/maintenance/disk_move_evidence.rs"]
mod tests;
