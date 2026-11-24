//! Bounded, self-delimiting operator-retained plan. The digest is not a signature.
use super::super::{PATH_BYTES, TICKET_BYTES};
use super::{Batch, Ticket, message, no_memory};
use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};

const MAGIC: &[u8; 8] = b"ZXBATCH1";
pub(super) const ORDER_PREFIX_BYTES: usize = 72;
pub(super) const MAX_ORDER_BYTES: usize = 32 * 1024 * 1024;
pub(super) const MAX_PLAN_BYTES: usize = MAX_ORDER_BYTES - ORDER_PREFIX_BYTES;
pub(super) const MAX_ROWS: usize = 65_536;
const FRAME_BYTES: usize = 8 + 8 + 32;
const FIXED_TICKET_BYTES: usize = TICKET_BYTES - 3 * PATH_BYTES - 255;

fn invalid() -> io::Error {
    message("expected one complete, unchanged binary batch plan")
}

pub(super) fn limit() -> io::Error {
    message(
        "import plan exceeds its row or byte budget; keep the original or use smaller selections",
    )
}

pub(super) fn require_count(count: usize) -> io::Result<()> {
    if count == 0 {
        return Err(message("an import batch cannot be empty"));
    }
    if count > MAX_ROWS {
        return Err(limit());
    }
    Ok(())
}

pub(super) fn width(target: &[u8], root: &[u8], relative: &[u8], name: &[u8]) -> usize {
    FIXED_TICKET_BYTES + target.len() + root.len() + relative.len() + name.len()
}

fn charge(remaining: &mut usize, width: usize) -> io::Result<()> {
    if !(FIXED_TICKET_BYTES..=TICKET_BYTES).contains(&width) {
        return Err(invalid());
    }
    *remaining = remaining.checked_sub(4 + width).ok_or_else(limit)?;
    Ok(())
}

pub(super) fn measure(widths: impl Iterator<Item = usize>) -> io::Result<()> {
    let mut remaining = MAX_PLAN_BYTES - FRAME_BYTES;
    for width in widths {
        charge(&mut remaining, width)?;
    }
    Ok(())
}

fn fixed<const N: usize>(input: &mut impl Read) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}

impl Batch {
    pub(super) fn write(&self, output: &mut impl Write) -> io::Result<()> {
        require_count(self.0.len())?;
        measure(
            self.0
                .iter()
                .map(|t| width(&t.target, &t.source_root, &t.relative, &t.name)),
        )?;
        let count = u64::try_from(self.0.len()).map_err(|_| limit())?;
        let mut hash = Sha256::new();
        let mut part = |raw: &[u8]| -> io::Result<()> {
            hash.update(raw);
            output.write_all(raw)
        };
        part(MAGIC)?;
        part(&count.to_be_bytes())?;
        for ticket in &self.0 {
            let raw = ticket.encode()?;
            part(
                &u32::try_from(raw.len())
                    .map_err(|_| invalid())?
                    .to_be_bytes(),
            )?;
            part(&raw)?;
        }
        output.write_all(&hash.finalize())?;
        output.flush()
    }

    pub(super) fn read(input: &mut impl Read) -> io::Result<Self> {
        Self::read_bounded(input, MAX_PLAN_BYTES)
    }

    pub(super) fn read_bounded(input: &mut impl Read, max_bytes: usize) -> io::Result<Self> {
        let mut remaining = max_bytes
            .min(MAX_PLAN_BYTES)
            .checked_sub(FRAME_BYTES)
            .ok_or_else(limit)?;
        if fixed::<8>(input)? != *MAGIC {
            return Err(invalid());
        }
        let count = fixed::<8>(input)?;
        let rows = usize::try_from(u64::from_be_bytes(count)).map_err(|_| limit())?;
        require_count(rows)?;
        if rows > remaining / (4 + FIXED_TICKET_BYTES) {
            return Err(limit());
        }
        let mut hash = Sha256::new();
        hash.update(MAGIC);
        hash.update(count);
        let mut tickets = Vec::new();
        // Charge each row before allocating or reading its raw bytes. Never
        // allocate from the untrusted declared row count.
        for _ in 0..rows {
            let size = fixed::<4>(input)?;
            let width = usize::try_from(u32::from_be_bytes(size)).map_err(|_| invalid())?;
            charge(&mut remaining, width)?;
            let mut raw = vec![0; width];
            input.read_exact(&mut raw)?;
            let ticket = Ticket::decode(&raw)?;
            if ticket.encode()? != raw {
                return Err(invalid());
            }
            hash.update(size);
            hash.update(&raw);
            tickets.try_reserve(1).map_err(no_memory)?;
            tickets.push(ticket);
        }
        let expected: [u8; 32] = hash.finalize().into();
        if fixed::<32>(input)? != expected {
            return Err(invalid());
        }
        match fixed::<1>(input) {
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Self::checked(tickets),
            Err(e) => Err(e),
            Ok(_) => Err(invalid()),
        }
    }
}
