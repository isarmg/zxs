//! Exact bounded JSON control requests for the private Workspace hop.
use super::super::{Id, JsonMutation, Request, dispatch::Decoded};
use std::io::Read;

const MAX_JSON: u64 = 64 * 1024;

pub(super) fn decode(request: &Request, input: &mut impl Read, length: u64) -> Option<Decoded> {
    if length > MAX_JSON { return None; }
    let mut body = Vec::new();
    input.take(length + 1).read_to_end(&mut body).ok()?;
    if body.len() as u64 != length { return None; }
    let mut p = Parser(std::str::from_utf8(&body).ok()?);
    p.need('{')?;
    let key = p.string()?;
    p.need(':')?;
    let result = match (request, key.as_str()) {
        (Request::Json { mutation: JsonMutation::Rename, length: expected, .. }, "name") if *expected == length =>
            Decoded::Rename(p.string()?),
        (Request::Json { mutation: JsonMutation::ReplaceTags, length: expected, .. }, "tags") if *expected == length =>
            Decoded::ReplaceTags(p.strings()?),
        (Request::CreateBranch { length: expected, .. }, "from_node") if *expected == length =>
            Decoded::CreateBranchFrom(Id::parse(&p.string()?).ok()?),
        _ => return None,
    };
    p.need('}')?;
    if p.space().is_empty() { Some(result) } else { None }
}

struct Parser<'a>(&'a str);

impl<'a> Parser<'a> {
    fn space(&mut self) -> &'a str {
        self.0 = self.0.trim_start_matches([' ', '\t', '\n', '\r']);
        self.0
    }

    fn take(&mut self, token: char) -> bool {
        if let Some(rest) = self.space().strip_prefix(token) { self.0 = rest; true } else { false }
    }

    fn need(&mut self, token: char) -> Option<()> {
        self.take(token).then_some(())
    }

    fn char(&mut self) -> Option<char> {
        let ch = self.0.chars().next()?;
        self.0 = &self.0[ch.len_utf8()..];
        Some(ch)
    }

    fn code_unit(&mut self) -> Option<u16> {
        let hex = self.0.get(..4)?;
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) { return None; }
        self.0 = &self.0[4..];
        u16::from_str_radix(hex, 16).ok()
    }

    fn scalar(&mut self) -> Option<char> {
        let hi = self.code_unit()?;
        if (0xd800..=0xdbff).contains(&hi) {
            self.0 = self.0.strip_prefix("\\u")?;
            let lo = self.code_unit()?;
            if !(0xdc00..=0xdfff).contains(&lo) { return None; }
            return char::from_u32(0x10000 + (u32::from(hi - 0xd800) << 10) + u32::from(lo - 0xdc00));
        }
        char::from_u32(u32::from(hi))
    }

    fn string(&mut self) -> Option<String> {
        self.need('"')?;
        let mut value = String::new();
        loop {
            let ch = match self.char()? {
                '"' => return Some(value),
                '\\' => match self.char()? {
                    '"' => '"', '\\' => '\\', '/' => '/',
                    'b' => '\x08', 'f' => '\x0c', 'n' => '\n', 'r' => '\r', 't' => '\t',
                    'u' => self.scalar()?,
                    _ => return None,
                },
                ch if ch < '\u{20}' => return None,
                ch => ch,
            };
            value.push(ch);
        }
    }

    fn strings(&mut self) -> Option<Vec<String>> {
        self.need('[')?;
        let mut values = Vec::new();
        if self.take(']') { return Some(values); }
        loop {
            values.push(self.string()?);
            if self.take(']') { return Some(values); }
            self.need(',')?;
        }
    }
}
