//! Self-delimiting operator-retained file. The hash detects incomplete or
//! changed input, not who supplied it. Tickets keep the Store's existing format.
use super::{Batch, TICKET_BYTES, Ticket, message, no_memory};
use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};

const MAGIC: &[u8; 8] = b"ZXBATCH1";

fn invalid() -> io::Error {
    message("expected one complete, unchanged binary batch plan")
}

fn fixed<const N: usize>(input: &mut impl Read) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}

impl Batch {
    pub(super) fn write(&self, output: &mut impl Write) -> io::Result<()> {
        let count = u64::try_from(self.0.len()).map_err(|_| invalid())?;
        let mut hash = Sha256::new();
        let mut part = |raw: &[u8]| -> io::Result<()> {
            hash.update(raw);
            output.write_all(raw)
        };
        part(MAGIC)?;
        part(&count.to_be_bytes())?;
        for ticket in &self.0 {
            let raw = ticket.encode()?;
            part(&u32::try_from(raw.len()).map_err(|_| invalid())?.to_be_bytes())?;
            part(&raw)?;
        }
        output.write_all(&hash.finalize())?;
        output.flush()
    }

    pub(super) fn read(input: &mut impl Read) -> io::Result<Self> {
        if fixed::<8>(input)? != *MAGIC {
            return Err(invalid());
        }
        let count = fixed::<8>(input)?;
        let mut hash = Sha256::new();
        hash.update(MAGIC);
        hash.update(count);
        let mut tickets = Vec::new();
        // Never allocate from the untrusted declared row count. Bytes per
        // row are bounded by the original single-file ticket format.
        for _ in 0..u64::from_be_bytes(count) {
            let size = fixed::<4>(input)?;
            let width = usize::try_from(u32::from_be_bytes(size)).map_err(|_| invalid())?;
            if width == 0 || width > TICKET_BYTES {
                return Err(invalid());
            }
            let mut raw = vec![0; width];
            input.read_exact(&mut raw)?;
            let ticket = Ticket::decode(&raw)?;
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
