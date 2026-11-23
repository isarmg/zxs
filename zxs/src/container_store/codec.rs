//! Canonical request receipt, kept physically separate from opaque payload.
use super::{
    decode_tags, encode_tags, invalid,
    model::{Body, Change, Id, Mutation, Receipt, Target},
};
use std::io;

pub(super) const MAX_RECEIPT: usize = super::TAG_BYTES + 1024;

fn put_text(out: &mut Vec<u8>, value: &str) -> io::Result<()> {
    let raw = crate::display::component(value.as_bytes(), crate::display::MAX)?.as_bytes();
    out.push(u8::try_from(raw.len()).map_err(|_| invalid("container label too long"))?);
    out.extend_from_slice(raw);
    Ok(())
}
fn put_optional(out: &mut Vec<u8>, id: Option<Id>) {
    out.push(u8::from(id.is_some()));
    if let Some(id) = id {
        out.extend_from_slice(&id);
    }
}

pub(super) fn encode(r: &Receipt) -> io::Result<Vec<u8>> {
    let mut out = b"OC2\0".to_vec();
    out.extend_from_slice(&r.mutation.container);
    out.extend_from_slice(&r.mutation.request);
    out.extend_from_slice(&r.sequence.to_be_bytes());
    match &r.change {
        Change::Save {
            target,
            node,
            body,
            initial_name,
        } => {
            out.push(1);
            match target {
                Target::Branch {
                    name,
                    expected_head,
                } => {
                    out.push(0);
                    put_text(&mut out, name)?;
                    put_optional(&mut out, *expected_head);
                }
                Target::HistoricalMain {
                    new_branch,
                    original,
                } => {
                    out.push(1);
                    put_text(&mut out, new_branch)?;
                    out.extend_from_slice(original);
                }
            }
            out.extend_from_slice(node);
            out.extend_from_slice(&body.length.to_be_bytes());
            out.extend_from_slice(&body.sha256);
            out.push(u8::from(initial_name.is_some()));
            if let Some(name) = initial_name {
                put_text(&mut out, name)?;
            }
        }
        Change::Branch { name, from } => {
            out.push(2);
            put_text(&mut out, name)?;
            out.extend_from_slice(from);
        }
        Change::Rename {
            expected_sequence,
            name,
        } => {
            out.push(3);
            out.extend_from_slice(&expected_sequence.to_be_bytes());
            put_text(&mut out, name)?;
        }
        Change::Tags {
            expected_sequence,
            tags,
        } => {
            out.push(4);
            out.extend_from_slice(&expected_sequence.to_be_bytes());
            out.extend_from_slice(&encode_tags(tags)?);
        }
    }
    Ok(out)
}

struct Decoder<'a>(&'a [u8]);
impl Decoder<'_> {
    fn part(&mut self, n: usize) -> io::Result<&[u8]> {
        let (front, rest) = self
            .0
            .split_at_checked(n)
            .ok_or_else(|| invalid("truncated container receipt"))?;
        self.0 = rest;
        Ok(front)
    }
    fn array<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        self.part(N)?
            .try_into()
            .map_err(|_| invalid("truncated container receipt"))
    }
    fn byte(&mut self) -> io::Result<u8> {
        Ok(self.array::<1>()?[0])
    }
    fn word(&mut self) -> io::Result<u64> {
        self.array().map(u64::from_be_bytes)
    }
    fn text(&mut self) -> io::Result<String> {
        let n = usize::from(self.byte()?);
        crate::display::component(self.part(n)?, crate::display::MAX).map(str::to_owned)
    }
    fn optional<T>(
        &mut self,
        read: impl FnOnce(&mut Self) -> io::Result<T>,
    ) -> io::Result<Option<T>> {
        match self.byte()? {
            0 => Ok(None),
            1 => read(self).map(Some),
            _ => Err(invalid("invalid optional receipt field")),
        }
    }
}
pub(super) fn decode(bytes: &[u8]) -> io::Result<Receipt> {
    let mut input = Decoder(bytes);
    if input.array::<4>()? != *b"OC2\0" {
        return Err(invalid("unknown container receipt format"));
    }
    let mutation = Mutation {
        container: input.array()?,
        request: input.array()?,
    };
    let sequence = input.word()?;
    let change = match input.byte()? {
        1 => {
            let mode = input.byte()?;
            let branch = input.text()?;
            let target = match mode {
                0 => Target::Branch {
                    name: branch,
                    expected_head: input.optional(Decoder::array::<16>)?,
                },
                1 => Target::HistoricalMain {
                    new_branch: branch,
                    original: input.array()?,
                },
                _ => return Err(invalid("unknown save target")),
            };
            let node = input.array()?;
            let body = Body {
                length: input.word()?,
                sha256: input.array()?,
            };
            let initial_name = input.optional(Decoder::text)?;
            Change::Save {
                target,
                node,
                body,
                initial_name,
            }
        }
        2 => Change::Branch {
            name: input.text()?,
            from: input.array()?,
        },
        3 => Change::Rename {
            expected_sequence: input.word()?,
            name: input.text()?,
        },
        4 => {
            let expected_sequence = input.word()?;
            let tags = decode_tags(input.0)?;
            input.0 = &[];
            Change::Tags {
                expected_sequence,
                tags,
            }
        }
        _ => return Err(invalid("unknown container operation")),
    };
    if !input.0.is_empty() {
        return Err(invalid("trailing container receipt bytes"));
    }
    Ok(Receipt {
        mutation,
        sequence,
        change,
    })
}
