//! Fixed, authenticated local handoff to the installed storage binary.
use super::super::random;
use super::{ExternalSource, OfflineSession, Ticket, refused};
use crate::local_worker::{
    Worker,
    transport::{self, Channel, Kind, Payload},
};
use rustix::{
    net::UCred,
    process::{getegid, geteuid, getgid, getuid},
};
use sha2::{Digest, Sha256};
use std::{fs::File, io};

pub(in super::super) struct ApplyOutcome {
    pub(in super::super) prior: bool,
    pub(in super::super) container: [u8; 16],
    pub(in super::super) node: [u8; 16],
}

pub(in super::super) fn apply(
    session: &OfflineSession,
    ticket: &Ticket,
) -> io::Result<ApplyOutcome> {
    ticket.require_target(session)?;
    if getuid() != geteuid() || getgid() != getegid() {
        return Err(refused());
    }
    session.with_installed(|executable| run(executable, session, ticket))
}

fn run(executable: &File, session: &OfflineSession, ticket: &Ticket) -> io::Result<ApplyOutcome> {
    let raw = ticket.encode()?;
    let digest = Sha256::digest(&raw).into();
    let (container, node) = result_ids(&raw, ticket.seed)?;
    let binding: [u8; 32] = Sha256::digest(session.original.full.as_slice()).into();
    let scope = random::<32>()?;
    if scope == [0; 32] {
        return Err(refused());
    }
    let mut envelope = Vec::with_capacity(72 + raw.len());
    envelope.extend_from_slice(b"ZXIOC1\0\0");
    envelope.extend_from_slice(&binding);
    envelope.extend_from_slice(&scope);
    envelope.extend_from_slice(&raw);
    let order = transport::seal_import_ticket(&envelope)?;
    let (local, peer) = transport::pair()?;
    let mut worker = Worker::spawn_import(executable, &peer, &order, &session.locked)?;
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
    let prior = exchange(
        &mut worker,
        &channel,
        &digest,
        ticket,
        session,
        container,
        node,
    )?;
    Ok(ApplyOutcome {
        prior,
        container,
        node,
    })
}

fn exchange(
    worker: &mut Worker,
    channel: &Channel,
    digest: &[u8; 32],
    ticket: &Ticket,
    session: &OfflineSession,
    container: [u8; 16],
    node: [u8; 16],
) -> io::Result<bool> {
    let mut source: Option<ExternalSource> = None;
    let mut nonce = 0_u64;
    let prior = loop {
        let payload = channel.wait_import(digest, || worker.live())?;
        match payload[0] {
            1 if source.is_none() && payload == step(1, 0) => {
                ticket.require_target(session)?;
                let selected = ticket.source(session)?;
                let body = selected.reader()?;
                channel.send_original(
                    Kind::ImportStep,
                    digest,
                    &step(2, selected.mount.id),
                    &body,
                )?;
                source = Some(selected);
            }
            3 if source.is_some() => {
                nonce = nonce.checked_add(1).ok_or_else(refused)?;
                if payload != step(3, nonce) {
                    return Err(refused());
                }
                ticket.require_target(session)?;
                source.as_ref().ok_or_else(refused)?.verify()?;
                channel.send(Kind::ImportStep, digest, &step(4, nonce))?;
            }
            5 if source.is_none() && payload == receipt(5, container, ticket.seed, node) => {
                break true;
            }
            6 if source.is_some()
                && nonce != 0
                && payload == receipt(6, container, ticket.seed, node) =>
            {
                break false;
            }
            _ => return Err(refused()),
        }
    };
    worker.finish_import(channel)?;
    ticket.require_target(session)?;
    if let Some(source) = source {
        source.verify()?;
    }
    Ok(prior)
}

pub(super) fn step(kind: u8, word: u64) -> Payload {
    let mut out = [0; 65];
    out[0] = kind;
    out[1..9].copy_from_slice(&word.to_be_bytes());
    out
}

pub(super) fn receipt(kind: u8, container: [u8; 16], request: [u8; 16], node: [u8; 16]) -> Payload {
    let mut out = step(kind, 1);
    out[9..25].copy_from_slice(&container);
    out[25..41].copy_from_slice(&request);
    out[41..57].copy_from_slice(&node);
    out
}

pub(super) fn result_ids(raw: &[u8], seed: [u8; 16]) -> io::Result<([u8; 16], [u8; 16])> {
    let mut canonical = Sha256::new();
    canonical.update(b"ZXS-IMPORT-INTENT-V1\0");
    canonical.update(raw.get(24..).ok_or_else(refused)?);
    let intent = canonical.finalize();
    let derive = |domain: &[u8]| -> io::Result<[u8; 16]> {
        let mut d = Sha256::new();
        d.update(domain);
        d.update(seed);
        d.update(intent);
        d.finalize()[..16].try_into().map_err(|_| refused())
    };
    let container = derive(b"ZXS-IMPORT-CONTAINER-V1\0")?;
    let node = derive(b"ZXS-IMPORT-NODE-V1\0")?;
    if container == [0; 16] || node == [0; 16] || container == node {
        return Err(refused());
    }
    Ok((container, node))
}
