//! A bounded fixed order on the same original offline lease for every row.
use super::{
    Channel, ImportIntent, Payload, Sha256, fixed, inherited_order_with_limit, parse_ticket,
    receipt_step, refused, source_matches, step,
};
use crate::{container_store::import_one::OfflineImport, headless_offline};
use std::{cell::Cell, collections::BTreeSet, io};

const BATCH_KIND: u8 = 47;
const MAX_BATCH_BYTES: usize = 32 * 1024 * 1024;
const MAX_ROWS: usize = 65_536;
const TICKET_BYTES: usize = super::MAX_ENVELOPE - 72;
const MIN_ROW_BYTES: usize = 4 + 184;

struct Row {
    digest: [u8; 32],
    intent: ImportIntent,
}

struct Plan {
    scope: [u8; 32],
    digest: [u8; 32],
    rows: Vec<Row>,
}

pub(crate) fn run() -> io::Result<()> {
    let (socket, parent, raw) = inherited_order_with_limit(MAX_BATCH_BYTES)?;
    let (fence, selection) = headless_offline::retain()?;
    let Plan {
        scope,
        digest,
        rows,
    } = parse_plan(&raw, fence.binding)?;
    drop(raw);
    // No writer is bound until the *entire* plan has passed independent
    // canonical checks, including its trailing digest and every ticket.
    let (writer, state) = selection.bind_import(|state| fence.require(state))?;
    let require = || fence.require(&state);
    let count = u64::try_from(rows.len()).map_err(|_| refused())?;
    let mut channel = Channel {
        socket,
        parent,
        scope,
        digest,
        kind: BATCH_KIND,
    };
    for (i, row) in rows.iter().enumerate() {
        let ordinal = u64::try_from(i + 1).map_err(|_| refused())?;
        channel.digest = row.digest;
        import_row(&writer, &channel, &row.intent, ordinal, &require)?;
    }
    require()?;
    channel.digest = digest;
    channel.send(step(8, count))
}

fn import_row(
    writer: &OfflineImport,
    channel: &Channel,
    intent: &ImportIntent,
    ordinal: u64,
    require: &impl Fn() -> io::Result<()>,
) -> io::Result<()> {
    let (_, node) = intent.identity()?;
    if let Some(receipt) = writer.receipt(intent, require).map_err(|_| refused())? {
        let confirmed = numbered(receipt_step(5, &receipt, node), ordinal);
        channel.send(confirmed)?;
        receive_ack(channel, confirmed)?;
        return require();
    }

    channel.send(numbered(step(1, 0), ordinal))?;
    let (answer, source) = channel.receive()?;
    let mount = u64::from_be_bytes(answer[1..9].try_into().map_err(|_| refused())?);
    if mount == 0 || answer != numbered(step(2, mount), ordinal) {
        return Err(refused());
    }
    let source = source.ok_or_else(refused)?;
    let nonce = Cell::new(0_u64);
    let authorize = || {
        require()?;
        source_matches(&source, intent, mount)?;
        let now = nonce.get().checked_add(1).ok_or_else(refused)?;
        nonce.set(now);
        channel.send(numbered(step(3, now), ordinal))?;
        let (answer, unexpected) = channel.receive()?;
        if unexpected.is_some() || answer != numbered(step(4, now), ordinal) {
            return Err(refused());
        }
        require()?;
        source_matches(&source, intent, mount)
    };
    let receipt = writer
        .save(intent, &source, authorize)
        .map_err(|_| refused())?;
    require()?;
    source_matches(&source, intent, mount)?;
    let confirmed = numbered(receipt_step(6, &receipt, node), ordinal);
    channel.send(confirmed)?;
    receive_ack(channel, confirmed)?;
    require()?;
    source_matches(&source, intent, mount)
}

fn receive_ack(channel: &Channel, confirmed: Payload) -> io::Result<()> {
    let (answer, fd) = channel.receive()?;
    check_ack(confirmed, answer, fd.is_some())
}

fn check_ack(mut confirmed: Payload, answer: Payload, has_fd: bool) -> io::Result<()> {
    // Only a byte-for-byte mirror of this exact confirmed operation permits
    // the next item. The original Helper sends it after re-pin and flush.
    confirmed[0] = 7;
    if has_fd || answer != confirmed {
        return Err(refused());
    }
    Ok(())
}

fn numbered(mut data: Payload, ordinal: u64) -> Payload {
    data[57..65].copy_from_slice(&ordinal.to_be_bytes());
    data
}

fn digest(bytes: &[u8]) -> io::Result<[u8; 32]> {
    let mut hash = Sha256::new();
    hash.update(bytes)?;
    Ok(hash.finish())
}

fn parse_plan(raw: &[u8], binding: [u8; 32]) -> io::Result<Plan> {
    if raw.len() > MAX_BATCH_BYTES {
        return Err(refused());
    }
    let mut input = raw;
    if fixed::<8>(&mut input)? != *b"ZXIBC1\0\0" || fixed::<32>(&mut input)? != binding {
        return Err(refused());
    }
    let scope = fixed::<32>(&mut input)?;
    if scope == [0; 32] {
        return Err(refused());
    }
    let (frame, checksum) = input.split_last_chunk::<32>().ok_or_else(refused)?;
    if digest(frame)? != *checksum {
        return Err(refused());
    }
    let mut encoded = frame;
    if fixed::<8>(&mut encoded)? != *b"ZXBATCH1" {
        return Err(refused());
    }
    let count = usize::try_from(u64::from_be_bytes(fixed(&mut encoded)?)).map_err(|_| refused())?;
    if !(1..=MAX_ROWS).contains(&count) || count > encoded.len() / MIN_ROW_BYTES {
        return Err(refused());
    }
    let mut rows = Vec::new();
    rows.try_reserve_exact(count)
        .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
    for _ in 0..count {
        let width =
            usize::try_from(u32::from_be_bytes(fixed(&mut encoded)?)).map_err(|_| refused())?;
        if width == 0 || width > TICKET_BYTES {
            return Err(refused());
        }
        let (ticket, tail) = encoded.split_at_checked(width).ok_or_else(refused)?;
        rows.push(Row {
            digest: digest(ticket)?,
            intent: parse_ticket(ticket)?,
        });
        encoded = tail;
    }
    if !encoded.is_empty() {
        return Err(refused());
    }
    check_rows(&rows)?;
    Ok(Plan {
        scope,
        digest: digest(raw)?,
        rows,
    })
}

fn check_rows(rows: &[Row]) -> io::Result<()> {
    let first = &rows.first().ok_or_else(refused)?.intent;
    let mut seeds = BTreeSet::new();
    let mut relatives = BTreeSet::new();
    let mut names = BTreeSet::new();
    for row in rows {
        let ticket = &row.intent;
        if ticket.pool != first.pool
            || ticket.data_token != first.data_token
            || ticket.data_path != first.data_path
            || ticket.source_root != first.source_root
            || !seeds.insert(ticket.seed)
            || !relatives.insert(ticket.source_relative.as_slice())
            || !names.insert(ticket.name.as_str())
        {
            return Err(refused());
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/offline_import_batch.rs"]
mod tests;
