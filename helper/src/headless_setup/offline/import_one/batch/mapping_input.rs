//! Optional raw-byte mapping input for more named overrides than fit in argv.
//! RELATIVE NUL NAME NUL repeated, then a separate empty path (NUL) and EOF.
use super::{Mapping, frame, message, no_memory};
use std::{
    ffi::OsString,
    io::{self, BufRead, Read},
    os::unix::ffi::OsStringExt,
};

fn field(input: &mut impl BufRead, limit: usize) -> io::Result<Vec<u8>> {
    let mut raw = Vec::new();
    let width = input
        .by_ref()
        .take((limit + 2) as u64)
        .read_until(0, &mut raw)?;
    if width == 0 || width > limit + 1 || raw.pop() != Some(0) {
        return Err(message(
            "incomplete or overlong NUL-delimited mapping field",
        ));
    }
    Ok(raw)
}

pub(super) fn read(input: &mut impl BufRead) -> io::Result<Vec<Mapping>> {
    let mut rows = Vec::new();
    let mut remaining = frame::MAX_PLAN_BYTES;
    loop {
        let path = field(input, super::super::PATH_BYTES)?;
        if path.is_empty() {
            if !input.fill_buf()?.is_empty() {
                return Err(message("unexpected bytes after the mapping terminator"));
            }
            return Ok(rows);
        }
        if rows.len() == frame::MAX_ROWS {
            return Err(frame::limit());
        }
        let name = field(input, 255)?;
        if name.is_empty() {
            return Err(message(
                "a mapping needs a nonempty destination display name",
            ));
        }
        remaining = remaining
            .checked_sub(path.len() + name.len())
            .ok_or_else(frame::limit)?;
        rows.try_reserve(1).map_err(no_memory)?;
        rows.push((OsString::from_vec(path), OsString::from_vec(name)));
    }
}
