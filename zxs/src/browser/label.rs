//! Exact label text at the management boundary. A label is never a pathname.
pub(crate) const MAX: usize = 255;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Label(String);

fn nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

fn decode_hex(text: &str, bytes: &mut [u8]) -> Option<()> {
    let (pairs, rest) = text.as_bytes().as_chunks::<2>();
    if !rest.is_empty() || pairs.len() != bytes.len() {
        return None;
    }
    for (byte, pair) in bytes.iter_mut().zip(pairs) {
        *byte = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(())
}

pub(crate) fn hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let mut bytes = [0; N];
    decode_hex(text, &mut bytes)?;
    Some(bytes)
}

pub(crate) fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from(HEX[usize::from(*b >> 4)]));
        out.push(char::from(HEX[usize::from(*b & 15)]));
    }
    out
}

impl Label {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        if text.len() > MAX || text.trim().is_empty()
            || text.chars().any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}')) {
            return None;
        }
        Some(Self(text.to_owned()))
    }

    pub(crate) fn from_bytes(raw: &[u8]) -> Option<Self> {
        Self::parse(std::str::from_utf8(raw).ok()?)
    }

    pub(crate) fn from_hex(text: &str) -> Option<Self> {
        if text.is_empty() || text.len() > MAX * 2 || !text.len().is_multiple_of(2) {
            return None;
        }
        let mut raw = [0; MAX];
        let len = text.len() / 2;
        decode_hex(text, &mut raw[..len])?;
        Self::from_bytes(&raw[..len])
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        self.text().as_bytes()
    }

    pub(crate) fn text(&self) -> &str {
        &self.0
    }

    pub(crate) fn encoded(&self) -> String {
        to_hex(self.bytes())
    }
}

#[cfg(test)]
#[path = "../../tests/browser/label.rs"]
mod tests;
