//! UR1 is a private upload checkpoint, never a DATA route or operation receipt.
use crate::{
    container_store::{Body, Id, MAIN, Mutation, NodeId, Target, hex, invalid, parse_container_id},
    display,
    hash::Sha256,
};
use std::io;

pub(crate) const CHUNK: u64 = 8 * 1024 * 1024;
pub(crate) const MAX_BODY: u64 = 1 << 40;
pub(super) const MAX_RECORD: usize = 512;
const MAGIC: [u8; 4] = *b"UR1\0";
const ID_DOMAIN: &[u8] = b"ZXS upload intent v1\0";
const EMPTY_SHA: [u8; 32] = [
    0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f, 0xb9, 0x24,
    0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b, 0x78, 0x52, 0xb8, 0x55,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Scope {
    pub(crate) pool: Id,
    pub(crate) epoch: Id,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Intent {
    pub(crate) scope: Scope,
    pub(crate) mutation: Mutation,
    pub(crate) node: NodeId,
    pub(crate) body: Body,
    pub(crate) target: Target,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Birth {
    pub(super) inode: u64,
    pub(super) seconds: i64,
    pub(super) nanos: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Phase {
    Preparing,
    Ready {
        directory: Birth,
        payload: Birth,
        offset: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Record {
    pub(super) data_token: [u8; 32],
    pub(super) intent: Intent,
    pub(super) phase: Phase,
}

impl Intent {
    fn encode(&self, out: &mut Vec<u8>) -> io::Result<()> {
        for id in [
            self.scope.pool,
            self.scope.epoch,
            self.mutation.container,
            self.mutation.request,
            self.node,
        ] {
            put_id(out, id)?;
        }
        if self.scope.pool == self.scope.epoch
            || self.body.length > MAX_BODY
            || self.body.length == 0 && self.body.sha256 != EMPTY_SHA
        {
            return Err(invalid("invalid upload scope or full body"));
        }
        out.extend_from_slice(&self.body.length.to_be_bytes());
        out.extend_from_slice(&self.body.sha256);
        put_target(out, &self.target)
    }

    fn decode(input: &mut Decoder<'_>) -> io::Result<Self> {
        Ok(Self {
            scope: Scope {
                pool: input.id()?,
                epoch: input.id()?,
            },
            mutation: Mutation {
                container: input.id()?,
                request: input.id()?,
            },
            node: input.id()?,
            body: Body {
                length: u64::from_be_bytes(input.part()?),
                sha256: input.part()?,
            },
            target: input.target()?,
        })
    }

    pub(super) fn upload_id(&self) -> io::Result<[u8; 32]> {
        let mut bytes = ID_DOMAIN.to_vec();
        self.encode(&mut bytes)?;
        let id = digest(&bytes)?;
        if id == [0; 32] {
            return Err(invalid("zero upload identity"));
        }
        Ok(id)
    }
}

fn put_id(out: &mut Vec<u8>, id: Id) -> io::Result<()> {
    if id == [0; 16] {
        return Err(invalid("zero ID in upload intent"));
    }
    out.extend_from_slice(&id);
    Ok(())
}

fn put_target(out: &mut Vec<u8>, target: &Target) -> io::Result<()> {
    match target {
        Target::FirstMain { name } => {
            let label = display::component(name.as_bytes(), display::MAX)?;
            let length = u8::try_from(label.len()).map_err(|_| invalid("upload name too long"))?;
            out.extend_from_slice(&[0, length]);
            out.extend_from_slice(label.as_bytes());
            Ok(())
        }
        Target::Branch {
            name,
            expected_head,
        } => {
            out.push(1);
            if name == MAIN {
                out.push(0);
            } else {
                out.push(1);
                put_id(out, parse_container_id(name.as_bytes())?)?;
            }
            put_id(out, *expected_head)
        }
        Target::HistoricalMain {
            new_branch,
            original,
        } => {
            out.push(2);
            put_id(out, parse_container_id(new_branch.as_bytes())?)?;
            put_id(out, *original)
        }
    }
}

impl Birth {
    const ZERO: Self = Self {
        inode: 0,
        seconds: 0,
        nanos: 0,
    };

    pub(super) fn valid(self) -> bool {
        self.inode != 0 && self.nanos < 1_000_000_000 && (self.seconds != 0 || self.nanos != 0)
    }

    fn encode(self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.inode.to_be_bytes());
        out.extend_from_slice(&self.seconds.to_be_bytes());
        out.extend_from_slice(&self.nanos.to_be_bytes());
    }

    fn decode(input: &mut Decoder<'_>) -> io::Result<Self> {
        Ok(Self {
            inode: u64::from_be_bytes(input.part()?),
            seconds: i64::from_be_bytes(input.part()?),
            nanos: u32::from_be_bytes(input.part()?),
        })
    }
}

impl Record {
    pub(super) fn encode(&self) -> io::Result<Vec<u8>> {
        if self.data_token == [0; 32] {
            return Err(invalid("zero DATA token in upload record"));
        }
        let mut raw = MAGIC.to_vec();
        let (phase, directory, payload, offset) = match self.phase {
            Phase::Preparing => (0, Birth::ZERO, Birth::ZERO, 0),
            Phase::Ready {
                directory,
                payload,
                offset,
            } if directory.valid()
                && payload.valid()
                && valid_offset(self.intent.body.length, offset) =>
            {
                (1, directory, payload, offset)
            }
            Phase::Ready { .. } => return Err(invalid("invalid ready upload checkpoint")),
        };
        raw.push(phase);
        raw.extend_from_slice(&self.data_token);
        directory.encode(&mut raw);
        payload.encode(&mut raw);
        raw.extend_from_slice(&offset.to_be_bytes());
        self.intent.encode(&mut raw)?;
        let checksum = digest(&raw)?;
        raw.extend_from_slice(&checksum);
        if raw.len() > MAX_RECORD {
            return Err(invalid("upload record exceeds its byte bound"));
        }
        Ok(raw)
    }

    pub(super) fn decode(raw: &[u8]) -> io::Result<Self> {
        let digest_at = raw
            .len()
            .checked_sub(32)
            .filter(|_| raw.len() <= MAX_RECORD)
            .ok_or_else(|| invalid("invalid upload record length"))?;
        let (framed, checksum) = raw.split_at(digest_at);
        if digest(framed)? != checksum {
            return Err(invalid("invalid upload record checksum"));
        }
        let mut input = Decoder(framed);
        if input.part::<4>()? != MAGIC {
            return Err(invalid("unknown upload record"));
        }
        let phase = input.byte()?;
        let data_token = input.part()?;
        let directory = Birth::decode(&mut input)?;
        let payload = Birth::decode(&mut input)?;
        let offset = u64::from_be_bytes(input.part()?);
        let phase = match phase {
            0 if directory == Birth::ZERO && payload == Birth::ZERO && offset == 0 => {
                Phase::Preparing
            }
            1 => Phase::Ready {
                directory,
                payload,
                offset,
            },
            _ => return Err(invalid("invalid upload record phase")),
        };
        let out = Self {
            data_token,
            intent: Intent::decode(&mut input)?,
            phase,
        };
        if !input.0.is_empty() || out.encode()? != raw {
            return Err(invalid("noncanonical upload record"));
        }
        Ok(out)
    }
}

struct Decoder<'a>(&'a [u8]);

impl Decoder<'_> {
    fn take(&mut self, length: usize) -> io::Result<&[u8]> {
        let (prefix, rest) = self
            .0
            .split_at_checked(length)
            .ok_or_else(|| invalid("truncated upload record"))?;
        self.0 = rest;
        Ok(prefix)
    }

    fn part<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        self.take(N)?
            .try_into()
            .map_err(|_| invalid("truncated upload field"))
    }

    fn byte(&mut self) -> io::Result<u8> {
        Ok(self.part::<1>()?[0])
    }

    fn id(&mut self) -> io::Result<Id> {
        let id = self.part()?;
        if id == [0; 16] {
            return Err(invalid("zero ID in upload intent"));
        }
        Ok(id)
    }

    fn target(&mut self) -> io::Result<Target> {
        match self.byte()? {
            0 => {
                let length = usize::from(self.byte()?);
                let name = display::component(self.take(length)?, display::MAX)?.to_owned();
                Ok(Target::FirstMain { name })
            }
            1 => {
                let name = match self.byte()? {
                    0 => MAIN.to_owned(),
                    1 => hex(self.id()?),
                    _ => return Err(invalid("invalid upload branch")),
                };
                Ok(Target::Branch {
                    name,
                    expected_head: self.id()?,
                })
            }
            2 => Ok(Target::HistoricalMain {
                new_branch: hex(self.id()?),
                original: self.id()?,
            }),
            _ => Err(invalid("invalid upload target")),
        }
    }
}

fn digest(data: &[u8]) -> io::Result<[u8; 32]> {
    let mut digest = Sha256::new();
    digest.update(data)?;
    Ok(digest.finish())
}

fn valid_offset(total: u64, offset: u64) -> bool {
    total <= MAX_BODY && offset <= total && (offset == total || offset.is_multiple_of(CHUNK))
}

pub(super) fn valid_raw_extent(total: u64, durable: u64, raw: u64) -> bool {
    if !valid_offset(total, durable) {
        return false;
    }
    let last = durable + CHUNK.min(total - durable);
    (durable..=last).contains(&raw)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ChunkUse {
    AppendTo(u64),
    VerifyPrior,
}

pub(super) fn chunk_use(total: u64, durable: u64, at: u64, length: u64) -> Option<ChunkUse> {
    if !valid_offset(total, durable)
        || at >= total
        || !at.is_multiple_of(CHUNK)
        || length != CHUNK.min(total - at)
    {
        return None;
    }
    let end = at + length;
    if at == durable {
        Some(ChunkUse::AppendTo(end))
    } else if end <= durable {
        Some(ChunkUse::VerifyPrior)
    } else {
        None
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resumable_record.rs"]
mod tests;
