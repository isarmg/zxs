//! Read-only selected-container verification through the exact installed engine.
use super::{OfflineSession, SelectedRoles, refused, require_held};
use crate::{
    headless_setup::{parse_paths, random},
    local_worker::{
        Worker,
        transport::{self, Channel, Kind, Payload},
    },
    maintenance::diagnostic_worker,
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
    path::PathBuf,
    process::ExitCode,
};

const ORDER_MAGIC: &[u8; 8] = b"ZXOVF1\0\0";
const ORDER_BYTES: usize = 88;
const ROLE_DOMAIN: &[u8] = b"ZXS verify live roles v1\0";
const KIND: Kind = Kind::VerifyContainer;
type Id = [u8; 16];

#[derive(Debug)]
pub(crate) struct OfflineVerify {
    container: Id,
    roots: Vec<PathBuf>,
}

impl OfflineVerify {
    pub(crate) fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Self, &'static str> {
        let container = args
            .next()
            .and_then(|raw| raw.to_str().and_then(parse_id))
            .ok_or("verify requires one nonzero, 32-digit lowercase hexadecimal container ID")?;
        let roots = parse_paths(args)?;
        Ok(Self { container, roots })
    }

    pub(crate) fn run(self) -> io::Result<ExitCode> {
        rustix::thread::set_no_new_privs(true)?;
        if getuid() != geteuid() || getgid() != getegid() {
            return Err(refused());
        }
        diagnostic_worker(getuid().as_raw(), getgid().as_raw())?;
        // open_durable may sync a previous descriptor publication. Verification
        // uses the original offline observation only, with no such recovery.
        let session = SelectedRoles::select(&self.roots)?.open_offline(None)?;
        let verification = session.with_rechecked_installed(|executable, require_installed| {
            capture(&session, executable, require_installed, self.container)
        })?;
        let mut output = io::stdout().lock();
        verification.write(&mut output, &self.container)?;
        output.flush()?;
        Ok(ExitCode::SUCCESS)
    }
}

fn parse_id(text: &str) -> Option<Id> {
    fn nibble(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            _ => None,
        }
    }
    let raw: &[u8; 32] = text.as_bytes().try_into().ok()?;
    let mut id = [0; 16];
    let (pairs, _) = raw.as_chunks::<2>();
    for (value, two) in id.iter_mut().zip(pairs) {
        *value = (nibble(two[0])? << 4) | nibble(two[1])?;
    }
    (id != [0; 16]).then_some(id)
}

fn live_role_binding(descriptor: &[u8; 32], roles: &[[u64; 3]]) -> io::Result<[u8; 32]> {
    if !(3..=64).contains(&roles.len()) {
        return Err(refused());
    }
    let count = u8::try_from(roles.len()).map_err(|_| refused())?;
    let mut hash = Sha256::new();
    hash.update(ROLE_DOMAIN);
    hash.update(descriptor);
    hash.update([count]);
    for &[device, inode, mount] in roles {
        if inode == 0 || mount == 0 {
            return Err(refused());
        }
        hash.update(device.to_be_bytes());
        hash.update(inode.to_be_bytes());
        hash.update(mount.to_be_bytes());
    }
    Ok(hash.finalize().into())
}

fn original_binding(session: &OfflineSession) -> io::Result<[u8; 32]> {
    // The original roster is already checked against the protected descriptor.
    // Hash the exact first held roots, not a replacement reopened by name.
    let roles = session
        .selected
        .roots
        .iter()
        .map(|root| {
            let (entity, mount) = require_held(&root.input(), session.selected.uid)?;
            Ok([entity.dev, entity.ino, mount.id])
        })
        .collect::<io::Result<Vec<_>>>()?;
    let descriptor = Sha256::digest(session.original.full.as_slice()).into();
    live_role_binding(&descriptor, &roles)
}

fn order(binding: &[u8; 32], scope: &[u8; 32], id: &Id) -> io::Result<[u8; ORDER_BYTES]> {
    if *scope == [0; 32] || *id == [0; 16] {
        return Err(refused());
    }
    let mut raw = [0; ORDER_BYTES];
    raw[..8].copy_from_slice(ORDER_MAGIC);
    raw[8..40].copy_from_slice(binding);
    raw[40..72].copy_from_slice(scope);
    raw[72..].copy_from_slice(id);
    Ok(raw)
}

fn capture(
    session: &OfflineSession,
    executable: &File,
    require_installed: &dyn Fn() -> io::Result<()>,
    id: Id,
) -> io::Result<Verification> {
    let binding = original_binding(session)?;
    let scope = random::<32>()?;
    let raw = order(&binding, &scope, &id)?;
    let digest = Sha256::digest(raw).into();
    let order = transport::seal_import_ticket(&raw)?;
    require_installed()?;
    let (local, peer) = transport::pair()?;
    let mut worker = Worker::spawn_verify_container(executable, &peer, &order, &session.locked)?;
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
    let packet = channel.wait_offline(KIND, &digest, &worker)?;
    let verification = Verification::decode(packet, &id)?;
    worker.finish_verify(&channel)?;
    require_installed()?;
    if original_binding(session)? != binding {
        return Err(refused());
    }
    Ok(verification)
}

#[derive(Debug, Eq, PartialEq)]
enum Area {
    Data,
    Recycle,
}
impl Area {
    const fn name(&self) -> &'static str {
        match self {
            Self::Data => "DATA",
            Self::Recycle => "RECYCLE",
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct Verification {
    area: Area,
    receipts: u64,
    bodies: u64,
    bytes: u64,
}
impl Verification {
    fn decode(packet: Payload, id: &Id) -> io::Result<Self> {
        if packet[0] != 1 || &packet[1..17] != id || packet[42..] != [0; 23] {
            return Err(refused());
        }
        let word = |start: usize| {
            <[u8; 8]>::try_from(&packet[start..start + 8])
                .map(u64::from_be_bytes)
                .map_err(|_| refused())
        };
        let area = match packet[41] {
            1 => Area::Data,
            2 => Area::Recycle,
            _ => return Err(refused()),
        };
        let receipts = word(17)?;
        let bodies = word(25)?;
        let bytes = word(33)?;
        if bodies == 0 || bodies > receipts {
            return Err(refused());
        }
        Ok(Self {
            area,
            receipts,
            bodies,
            bytes,
        })
    }

    fn write(&self, output: &mut impl Write, id: &Id) -> io::Result<()> {
        use std::fmt::Write as _;
        let mut name = String::with_capacity(32);
        for byte in id {
            write!(name, "{byte:02x}").expect("String writes are infallible");
        }
        writeln!(
            output,
            "verified container {name}: area={} receipts={} payloads={} raw_bytes={}",
            self.area.name(),
            self.receipts,
            self.bodies,
            self.bytes
        )
    }
}

#[cfg(test)]
#[path = "../../../tests/maintenance/offline_verify.rs"]
mod tests;
