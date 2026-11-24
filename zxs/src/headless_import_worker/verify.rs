//! Fixed existing-only verification under the original offline parent.
use super::{Channel, Payload, Sha256, fixed, inherited_order_with_limit, refused};
use crate::{
    container_store::{
        Id,
        offline_catalog::{Verification, VerifiedLocation},
    },
    headless_offline,
};
use std::io;

const ORDER_BYTES: usize = 88;
const VERIFY_KIND: u8 = 48;
const ROLE_DOMAIN: &[u8] = b"ZXS verify live roles v1\0";

pub(crate) fn run() -> io::Result<()> {
    let (socket, parent, raw) = inherited_order_with_limit(ORDER_BYTES)?;
    let (fence, selection) = headless_offline::retain()?;
    let live_roles = selection.verify_live_roles(|state| fence.require(state))?;
    let binding = live_binding(fence.binding, &live_roles)?;
    let (scope, id) = order(&raw, binding)?;
    let mut hash = Sha256::new();
    hash.update(&raw)?;
    let channel = Channel {
        socket,
        parent,
        scope,
        digest: hash.finish(),
        kind: VERIFY_KIND,
    };
    let verified = selection.offline_verify(id, |state| fence.require(state))?;
    if selection.verify_live_roles(|state| fence.require(state))? != live_roles {
        return Err(refused());
    }
    channel.send(terminal(id, verified)?)
}

fn live_binding(descriptor: [u8; 32], ordered: &[[u64; 3]]) -> io::Result<[u8; 32]> {
    let count = u8::try_from(ordered.len()).map_err(|_| refused())?;
    if !(3..=64).contains(&count) {
        return Err(refused());
    }
    let mut hash = Sha256::new();
    hash.update(ROLE_DOMAIN)?;
    hash.update(&descriptor)?;
    hash.update(&[count])?;
    for [device, inode, mount] in ordered {
        if *inode == 0 || *mount == 0 {
            return Err(refused());
        }
        for coordinate in [device, inode, mount] {
            hash.update(&coordinate.to_be_bytes())?;
        }
    }
    Ok(hash.finish())
}

fn order(mut raw: &[u8], binding: [u8; 32]) -> io::Result<([u8; 32], Id)> {
    if raw.len() != ORDER_BYTES
        || fixed::<8>(&mut raw)? != *b"ZXOVF1\0\0"
        || fixed::<32>(&mut raw)? != binding
    {
        return Err(refused());
    }
    let scope = fixed(&mut raw)?;
    let id = fixed(&mut raw)?;
    if scope == [0; 32] || id == [0; 16] || !raw.is_empty() {
        return Err(refused());
    }
    Ok((scope, id))
}

fn terminal(id: Id, record: Verification) -> io::Result<Payload> {
    if id == [0; 16] || record.bodies == 0 || record.bodies > record.receipts {
        return Err(refused());
    }
    let mut out = [0; 65];
    out[0] = 1;
    out[1..17].copy_from_slice(&id);
    out[17..25].copy_from_slice(&record.receipts.to_be_bytes());
    out[25..33].copy_from_slice(&record.bodies.to_be_bytes());
    out[33..41].copy_from_slice(&record.bytes.to_be_bytes());
    out[41] = match record.location {
        VerifiedLocation::Data => 1,
        VerifiedLocation::Recycle => 2,
    };
    Ok(out)
}

#[cfg(test)]
pub(crate) fn fixture_binding(descriptor: [u8; 32], roles: &[[u64; 3]]) -> io::Result<[u8; 32]> {
    live_binding(descriptor, roles)
}

#[cfg(test)]
pub(crate) fn fixture_order(raw: &[u8], binding: [u8; 32]) -> io::Result<([u8; 32], Id)> {
    order(raw, binding)
}

#[cfg(test)]
#[path = "../../tests/unit/offline_verify_worker.rs"]
mod tests;
