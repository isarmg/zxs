//! Qualification of complete standalone executable bytes. No storage setup.
use std::io;

#[cfg(any(test, feature = "embedded-zxs"))]
use sha2::{Digest, Sha256};

#[cfg(feature = "embedded-zxs")]
pub(crate) use crate::sqlite_install::EmbeddedInstall;

pub(crate) const MAX_IMAGE: u64 = 64 << 20;

pub(crate) fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "not the independently approved protected static ZXS ELF",
    )
}

#[cfg(any(test, feature = "embedded-zxs"))]
pub(crate) fn verify(
    bytes: &[u8],
    compiler_expected: [u8; 32],
    approved: [u8; 32],
) -> io::Result<[u8; 32]> {
    if compiler_expected == [0; 32]
        || approved != compiler_expected
        || bytes.is_empty()
        || bytes.len() as u64 > MAX_IMAGE
        || <[u8; 32]>::from(Sha256::digest(bytes)) != approved
    {
        return Err(denied());
    }
    standalone(bytes.len() as u64, |offset, out| {
        let start = usize::try_from(offset).map_err(|_| denied())?;
        let end = start.checked_add(out.len()).ok_or_else(denied)?;
        out.copy_from_slice(bytes.get(start..end).ok_or_else(denied)?);
        Ok(())
    })?;
    Ok(approved)
}

// Same bounded ELF structure as the ZXS consumer's private image gate.
const MAX_PH: u16 = 128;
const MAX_DYN: u64 = 128 * 1024;
const PH_LEN: usize = 56;
const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_GNU_STACK: u32 = 0x6474_e551;
const PF_X: u32 = 1;

fn get<const N: usize>(b: &[u8], offset: usize) -> [u8; N] {
    b[offset..offset + N].try_into().expect("fixed ELF field")
}
fn short(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(get(b, o))
}
fn word(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(get(b, o))
}
fn long(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(get(b, o))
}
fn range(o: u64, n: u64, len: u64) -> bool {
    o.checked_add(n).is_some_and(|end| end <= len)
}
pub(crate) fn standalone(
    len: u64,
    mut read: impl FnMut(u64, &mut [u8]) -> io::Result<()>,
) -> io::Result<()> {
    let mut head = [0; 64];
    read(0, &mut head)?;
    let ty = short(&head, 16);
    let machine = short(&head, 18);
    let n = short(&head, 56);
    let entry = long(&head, 24);
    let ph = long(&head, 32);
    let row_size = PH_LEN as u64;
    if !head.starts_with(b"\x7fELF\x02\x01\x01")
        || machine != 62
        || word(&head, 20) != 1
        || !matches!(ty, ET_EXEC | ET_DYN)
        || entry == 0
        || short(&head, 52) != 64
        || usize::from(short(&head, 54)) != PH_LEN
        || !(1..=MAX_PH).contains(&n)
        || ph < 64
        || !range(ph, u64::from(n) * row_size, len)
    {
        return Err(denied());
    }
    let mut executable_entry = false;
    let mut dyn_rows = None;
    let mut loads = Vec::with_capacity(usize::from(n));
    for i in 0..u64::from(n) {
        let mut p = [0; PH_LEN];
        read(ph + i * row_size, &mut p)?;
        let k = word(&p, 0);
        let flags = word(&p, 4);
        let (o, address, bytes, mem) = (long(&p, 8), long(&p, 16), long(&p, 32), long(&p, 40));
        if k == PT_INTERP || k == PT_GNU_STACK && flags & PF_X != 0 {
            return Err(denied());
        }
        if k == PT_LOAD {
            if bytes > mem || !range(o, bytes, len) || address.checked_add(mem).is_none() {
                return Err(denied());
            }
            if flags & PF_X != 0 && entry >= address && entry - address < bytes {
                executable_entry = true;
            }
            loads.push((o, address, bytes));
        }
        if k == PT_DYNAMIC
            && (dyn_rows.replace((o, address, bytes)).is_some()
                || !(16..=MAX_DYN).contains(&bytes)
                || bytes > mem
                || !bytes.is_multiple_of(16)
                || !range(o, bytes, len))
        {
            return Err(denied());
        }
    }
    if !executable_entry {
        return Err(denied());
    }
    let mut pie = false;
    if let Some((o, address, bytes)) = dyn_rows {
        if !loads.into_iter().any(|(off, v, len)| {
            address
                .checked_sub(v)
                .is_some_and(|d| off.checked_add(d) == Some(o) && range(d, bytes, len))
        }) {
            return Err(denied());
        }
        let mut end = false;
        for i in (0..bytes).step_by(16) {
            let mut row = [0; 16];
            read(o + i, &mut row)?;
            let tag = long(&row, 0);
            let value = long(&row, 8);
            if end && tag != 0 {
                return Err(denied());
            }
            match tag {
                0 => end = true, // DT_NULL
                1 | 15 | 29 | 0x7fff_fffd | 0x7fff_ffff | 0x6fff_fefa | 0x6fff_fefb
                | 0x6fff_fefc => return Err(denied()),
                0x6fff_fffb => pie |= value & 0x0800_0000 != 0, // DT_FLAGS_1 / DF_1_PIE
                _ => {}
            }
        }
        if !end {
            return Err(denied());
        }
    }
    if ty == ET_DYN && !pie {
        return Err(denied());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/build/standalone_elf.rs"]
pub(crate) mod tests;
