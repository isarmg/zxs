//! Bounds and encodings for the independently published embedded ELF.
use sha2::{Digest, Sha256};
use std::io::{self, Read};

pub(crate) fn hex(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let mut out = [0; 32];
    for (i, v) in out.iter_mut().enumerate() {
        *v = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Fixed lower-case ASCII, two digits for each byte, including leading zeroes.
pub(crate) fn lower_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::new();
    for &byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    text
}

#[cfg(test)]
#[test]
fn byte_hex_keeps_both_digits_and_lowercase() {
    assert_eq!(lower_hex(&[]), "");
    assert_eq!(lower_hex(&[0, 0x0f, 0x10, 0xba, 0xff]), "000f10baff");
}

pub(crate) fn hash_upto(file: &mut impl Read, size: u64, max: u64) -> io::Result<[u8; 32]> {
    if size == 0 || size > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "file outside hash budget",
        ));
    }
    let mut hash = Sha256::new();
    let mut left = size;
    let mut buf = vec![0; 32 * 1024];
    while left != 0 {
        let n = usize::try_from(left).unwrap_or(usize::MAX).min(buf.len());
        file.read_exact(&mut buf[..n])?;
        hash.update(&buf[..n]);
        left -= n as u64;
    }
    loop {
        match file.read(&mut buf[..1]) {
            Ok(0) => return Ok(hash.finalize().into()),
            Ok(_) => return Err(io::Error::new(io::ErrorKind::InvalidData, "size changed")),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}
