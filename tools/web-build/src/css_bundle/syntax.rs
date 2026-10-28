//! Small lexical cursor: comments and strings are opaque to import discovery.
use anyhow::{Context, Result, bail};
use std::ops::Range;

pub(super) struct Url {
    pub value: String,
    pub argument: Range<usize>,
}

#[derive(Clone)]
pub(super) struct Cursor<'a> {
    text: &'a str,
    position: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(text: &'a str) -> Self {
        Self { text, position: 0 }
    }

    pub fn position(&self) -> usize {
        self.position
    }
    pub fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.position).copied()
    }
    pub fn done(&self) -> bool {
        self.position == self.text.len()
    }
    pub fn advance(&mut self) {
        self.position += 1;
    }

    pub fn trivia(&mut self) -> Result<Option<&'a str>> {
        let start = self.position;
        if self.text[start..].starts_with("/*") {
            let end = self.text[start + 2..]
                .find("*/")
                .context("unclosed CSS comment")?;
            self.position = start + 2 + end + 2;
        } else {
            self.whitespace();
        }
        Ok((self.position != start).then(|| &self.text[start..self.position]))
    }

    fn whitespace(&mut self) {
        while self.peek().is_some_and(is_whitespace) {
            self.advance();
        }
    }

    pub fn starts_name(&self) -> bool {
        self.peek().is_some_and(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'\\') || byte >= 128
        })
    }

    pub fn name(&mut self) -> Result<String> {
        let mut name = String::new();
        while self.starts_name() {
            if self.peek() == Some(b'\\') {
                name.push(
                    self.escape()?
                        .context("newline escape is not a CSS identifier")?,
                );
            } else {
                name.push(self.character());
            }
        }
        Ok(name)
    }

    pub fn followed_by_colon(&self) -> Result<bool> {
        let mut next = self.clone();
        while next.trivia()?.is_some() {}
        Ok(next.peek() == Some(b':'))
    }

    fn character(&mut self) -> char {
        let character = self.text[self.position..]
            .chars()
            .next()
            .expect("invariant: cursor is not at EOF");
        self.position += character.len_utf8();
        character
    }

    fn escape(&mut self) -> Result<Option<char>> {
        self.advance();
        let first = self.peek().context("trailing CSS escape")?;
        if matches!(first, b'\n' | b'\r' | 12) {
            self.advance();
            if first == b'\r' && self.peek() == Some(b'\n') {
                self.advance();
            }
            return Ok(None);
        }
        if first.is_ascii_hexdigit() {
            let mut value = 0;
            for _ in 0..6 {
                let Some(digit) = self.peek().and_then(|byte| (byte as char).to_digit(16)) else {
                    break;
                };
                value = value * 16 + digit;
                self.advance();
            }
            if self.peek().is_some_and(is_whitespace) {
                let space = self.peek();
                self.advance();
                if space == Some(b'\r') && self.peek() == Some(b'\n') {
                    self.advance();
                }
            }
            // Invalid code points are deliberately rejected rather than silently
            // changing a file name through CSS's replacement-character recovery.
            if value == 0 {
                bail!("NUL CSS escape is not supported");
            }
            return Ok(Some(
                char::from_u32(value).context("invalid CSS escape code point")?,
            ));
        }
        Ok(Some(self.character()))
    }

    pub fn string(&mut self) -> Result<String> {
        let quote = self.peek().context("missing CSS string")?;
        if !matches!(quote, b'\'' | b'"') {
            bail!("expected CSS string");
        }
        self.advance();
        let mut value = String::new();
        loop {
            match self.peek().context("unclosed CSS string")? {
                byte if byte == quote => {
                    self.advance();
                    return Ok(value);
                }
                b'\\' => {
                    if let Some(character) = self.escape()? {
                        value.push(character);
                    }
                }
                b'\n' | b'\r' | 12 | 0 => bail!("invalid character in CSS string"),
                _ => value.push(self.character()),
            }
        }
    }

    /// Called after a decoded `url` identifier, with the cursor at `(`.
    pub fn url(&mut self) -> Result<Url> {
        self.advance();
        self.whitespace();
        let start = self.position;
        let value = if matches!(self.peek(), Some(b'\'' | b'"')) {
            self.string()?
        } else {
            let mut value = String::new();
            while let Some(byte) = self.peek() {
                if is_whitespace(byte) || byte == b')' {
                    break;
                }
                match byte {
                    b'\\' => value.push(
                        self.escape()?
                            .context("newline escape in unquoted CSS URL")?,
                    ),
                    b'\'' | b'"' | b'(' | 0..=8 | 11 | 14..=31 | 127 => {
                        bail!("invalid unquoted CSS URL")
                    }
                    _ => value.push(self.character()),
                }
            }
            value
        };
        let end = self.position;
        self.whitespace();
        if self.peek() != Some(b')') {
            bail!("unclosed or unsupported CSS url()");
        }
        self.advance();
        Ok(Url {
            value,
            argument: start..end,
        })
    }
}

fn is_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 12)
}
