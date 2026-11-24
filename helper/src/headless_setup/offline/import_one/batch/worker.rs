//! One exact installed writer owns the entire batch; Helper retains the sources.
use super::super::super::{import_command::write_receipt, random};
use super::super::{
    ExternalSource,
    ipc::{ApplyOutcome, receipt, result_ids, step},
    refused,
};
use super::{Batch, OfflineSession, Ticket, frame};
use crate::local_worker::{
    Worker,
    transport::{self, Channel, Kind, Payload},
};
use rustix::{
    net::UCred,
    process::{getgid, getuid},
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Write},
};

const ORDER_MAGIC: &[u8; 8] = b"ZXIBC1\0\0";
const KIND: Kind = Kind::ImportBatch;

pub(super) fn run(
    batch: &Batch,
    session: &OfflineSession,
    executable: &File,
    require_installed: &dyn Fn() -> io::Result<()>,
    output: &mut impl Write,
) -> io::Result<()> {
    let binding = Sha256::digest(session.original.full.as_slice()).into();
    let scope = random::<32>()?;
    let (order, digest) = sealed_order(batch, &binding, &scope)?;
    require_installed()?;

    let (local, peer) = transport::pair()?;
    let worker = Worker::spawn_import_batch(executable, &peer, &order, &session.locked)?;
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
    let mut handoff = Handoff {
        session,
        require_installed,
        worker,
        channel,
    };
    let count = batch.0.len();
    batch.visit(|i, ticket| handoff.item(i + 1, count, ticket, output))?;
    handoff.finish(&digest, count).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "batch end not confirmed: {error}; keep the sources and retry only with the same intact batch plan"
            ),
        )
    })
}

pub(super) fn sealed_order(
    batch: &Batch,
    binding: &[u8; 32],
    scope: &[u8; 32],
) -> io::Result<(File, [u8; 32])> {
    if *scope == [0; 32] {
        return Err(refused());
    }
    let mut hash = Sha256::new();
    let order = transport::seal_import_order(|file| {
        let mut output = HashOutput {
            file,
            hash: &mut hash,
        };
        output.write_all(ORDER_MAGIC)?;
        output.write_all(binding)?;
        output.write_all(scope)?;
        batch.write(&mut output)
    })?;
    let bytes = usize::try_from(order.metadata()?.len()).map_err(|_| frame::limit())?;
    if bytes > frame::MAX_ORDER_BYTES {
        return Err(frame::limit());
    }
    Ok((order, hash.finalize().into()))
}

struct HashOutput<'a> {
    file: &'a mut File,
    hash: &'a mut Sha256,
}

impl Write for HashOutput<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let n = self.file.write(bytes)?;
        self.hash.update(&bytes[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

struct Handoff<'a> {
    session: &'a OfflineSession,
    require_installed: &'a dyn Fn() -> io::Result<()>,
    worker: Worker,
    channel: Channel,
}

impl Handoff<'_> {
    fn require(&self, ticket: &Ticket, source: Option<&ExternalSource>) -> io::Result<()> {
        self.worker.live()?;
        (self.require_installed)()?;
        ticket.require_target(self.session)?;
        if let Some(source) = source {
            source.verify()?;
        }
        Ok(())
    }

    fn item(
        &self,
        ordinal: usize,
        count: usize,
        ticket: &Ticket,
        output: &mut impl Write,
    ) -> io::Result<()> {
        let raw = ticket.encode()?;
        let digest = Sha256::digest(&raw).into();
        let (container, node) = result_ids(&raw, ticket.seed)?;
        let mut row = Row {
            ordinal: u64::try_from(ordinal).map_err(|_| refused())?,
            container,
            seed: ticket.seed,
            node,
            requested: false,
            nonce: 0,
        };
        let mut source = None;
        let (mut confirmed, prior) = loop {
            let packet = self.channel.wait_offline(KIND, &digest, &self.worker)?;
            match row.accept(packet)? {
                Next::Source => {
                    self.require(ticket, None)?;
                    let original = ticket.source(self.session)?;
                    let body = original.reader()?;
                    self.require(ticket, Some(&original))?;
                    let answer = numbered(step(2, original.mount.id), row.ordinal);
                    self.channel.send_original(KIND, &digest, &answer, &body)?;
                    source = Some(original);
                }
                Next::Recheck(nonce) => {
                    let original = source.as_ref().ok_or_else(refused)?;
                    self.require(ticket, Some(original))?;
                    let answer = numbered(step(4, nonce), row.ordinal);
                    self.channel.send(KIND, &digest, &answer)?;
                }
                Next::Confirmed { prior } => break (packet, prior),
            }
        };
        confirmed[0] = 7;
        report_then_ack(
            output,
            |out| {
                write!(out, "{ordinal}/{count} ")?;
                write_receipt(
                    out,
                    &ApplyOutcome {
                        prior,
                        container,
                        node,
                    },
                )
            },
            || self.require(ticket, source.as_ref()),
            || self.channel.send(KIND, &digest, &confirmed),
        )
    }

    fn finish(&mut self, digest: &[u8; 32], count: usize) -> io::Result<()> {
        let count = u64::try_from(count).map_err(|_| refused())?;
        let terminal = self.channel.wait_offline(KIND, digest, &self.worker)?;
        require_end(terminal, count)?;
        self.worker.finish_import(&self.channel)?;
        (self.require_installed)()
    }
}

fn report_then_ack<W: Write>(
    output: &mut W,
    report: impl FnOnce(&mut W) -> io::Result<()>,
    require: impl Fn() -> io::Result<()>,
    ack: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    require()?;
    report(output)?;
    output.flush()?;
    // Output may block. The same originals must still be valid at ACK, not
    // merely before display. No next row can start without this exact ACK.
    require()?;
    ack()
}

fn numbered(mut payload: Payload, ordinal: u64) -> Payload {
    payload[57..65].copy_from_slice(&ordinal.to_be_bytes());
    payload
}

fn require_end(packet: Payload, count: u64) -> io::Result<()> {
    if packet != step(8, count) {
        return Err(refused());
    }
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
enum Next {
    Source,
    Recheck(u64),
    Confirmed { prior: bool },
}

struct Row {
    ordinal: u64,
    container: [u8; 16],
    seed: [u8; 16],
    node: [u8; 16],
    requested: bool,
    nonce: u64,
}

impl Row {
    fn accept(&mut self, packet: Payload) -> io::Result<Next> {
        if !self.requested && packet == numbered(step(1, 0), self.ordinal) {
            self.requested = true;
            return Ok(Next::Source);
        }
        let prior = !self.requested && packet[0] == 5;
        let saved = self.requested && self.nonce != 0 && packet[0] == 6;
        if (prior || saved)
            && packet
                == numbered(
                    receipt(packet[0], self.container, self.seed, self.node),
                    self.ordinal,
                )
        {
            return Ok(Next::Confirmed { prior });
        }
        if self.requested && packet[0] == 3 {
            let nonce = self.nonce.checked_add(1).ok_or_else(refused)?;
            if packet == numbered(step(3, nonce), self.ordinal) {
                self.nonce = nonce;
                return Ok(Next::Recheck(nonce));
            }
        }
        Err(refused())
    }
}

#[cfg(test)]
#[path = "../../../../../tests/maintenance/import_batch_worker.rs"]
mod tests;
