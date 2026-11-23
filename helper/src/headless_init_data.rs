//! Explicitly initialize exactly one enrolled DATA using the installed Store.
use super::{
    FreshSelection, Reset, import_one::selected_data, offline_catalog::with_installed, random,
    refused, require_held,
};
use crate::local_worker::{
    Worker,
    transport::{self, Channel, Kind, Payload},
};
use rustix::{
    net::UCred,
    process::{getegid, geteuid, getgid, getuid},
};
use sha2::{Digest, Sha256};
use std::{fs::File, io, os::unix::ffi::OsStrExt, path::Path};

impl FreshSelection {
    pub(crate) fn init_data(self, path: &Path) -> io::Result<()> {
        if getuid() != geteuid() || getgid() != getegid() {
            return Err(refused());
        }
        let reset = self.open_offline(None)?;
        let path = path.as_os_str().as_bytes();
        let token = selected_data(&reset, path)?;
        let scope = random::<32>()?;
        if scope == [0; 32] {
            return Err(refused());
        }
        let binding = Sha256::digest(reset.original.full.as_slice()).into();
        let roles = reset
            .selected
            .roots
            .iter()
            .map(|root| {
                let (entity, mount) = require_held(&root.input(), reset.selected.uid)?;
                Ok([entity.dev, entity.ino, mount.id])
            })
            .collect::<io::Result<Vec<_>>>()?;
        let order = encode(binding, scope, reset.selected.pool, token, &roles, path)?;
        with_installed(&reset, |executable| exchange(&reset, executable, scope, &order))
            .map_err(|e| io::Error::new(e.kind(), format!(
                "DATA initialization not confirmed: {e}; preserve the selected root and retry only the same command"
            )))
    }
}

fn encode(
    binding: [u8; 32],
    scope: [u8; 32],
    pool: [u8; 16],
    token: [u8; 32],
    roles: &[[u64; 3]],
    path: &[u8],
) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(b"ZXINITD1");
    out.extend_from_slice(&binding);
    out.extend_from_slice(&scope);
    out.extend_from_slice(&pool);
    out.extend_from_slice(&token);
    if !(3..=64).contains(&roles.len()) {
        return Err(refused());
    }
    out.push(u8::try_from(roles.len()).map_err(|_| refused())?);
    for role in roles {
        for field in role {
            out.extend_from_slice(&field.to_be_bytes());
        }
    }
    out.extend_from_slice(
        &u16::try_from(path.len())
            .map_err(|_| refused())?
            .to_be_bytes(),
    );
    out.extend_from_slice(path);
    Ok(out)
}

fn step(kind: u8, nonce: u64) -> Payload {
    let mut out = [0; 65];
    out[0] = kind;
    out[1..9].copy_from_slice(&nonce.to_be_bytes());
    out
}

fn exchange(reset: &Reset, executable: &File, scope: [u8; 32], raw: &[u8]) -> io::Result<()> {
    let digest: [u8; 32] = Sha256::digest(raw).into();
    let order = transport::seal_import_ticket(raw)?;
    let (local, peer) = transport::pair()?;
    let mut worker = Worker::spawn_data_init(executable, &peer, &order, &reset.locked)?;
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
    let mut nonce = 0_u64;
    loop {
        let packet = channel.wait_offline(Kind::InitData, &digest, || worker.live())?;
        if nonce != 0 && packet == step(3, nonce) {
            break;
        }
        if packet[0] == 4 && packet[1..9] == nonce.to_be_bytes() {
            let length = usize::from(packet[9]);
            if length <= 55
                && packet[10..10 + length]
                    .iter()
                    .all(|byte| (b' '..=b'~').contains(byte))
                && packet[10 + length..].iter().all(|&byte| byte == 0)
            {
                let reason =
                    std::str::from_utf8(&packet[10..10 + length]).map_err(|_| refused())?;
                return Err(io::Error::other(format!("storage refused: {reason}")));
            }
        }
        nonce = nonce.checked_add(1).ok_or_else(refused)?;
        if packet != step(1, nonce) {
            return Err(refused());
        }
        reset.require_original()?;
        channel.send(Kind::InitData, &digest, &step(2, nonce))?;
    }
    worker.finish_import(&channel)?;
    reset.require_original()
}

#[cfg(test)]
#[path = "../tests/maintenance/init_data_order.rs"]
mod tests;
