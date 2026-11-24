//! Private, non-I/O codec for the immutable DATA move journal.
//! A checked record proves bytes and lineage only, never a live mount, a lease or sync.
#[path = "proof.rs"]
pub(in crate::container_store) mod proof;

use super::{Id, encode as route_record, invalid};
use crate::hash::Sha256;
use std::io::{self, Cursor, Write};

pub(in crate::container_store) type Digest = [u8; 32];
type Route = [u8; 52];
pub(in crate::container_store) const MAX_ROWS: u64 = 65_536;
const INTENT: [u8; 8] = *b"ZXDMI1\0\0";
const GUARD: [u8; 8] = *b"ZXDMG1\0\0";
const READY: [u8; 8] = *b"ZXDMR1\0\0";
const SWITCHED: [u8; 8] = *b"ZXDMS1\0\0";
const DONE: [u8; 8] = *b"ZXDMD1\0\0";
pub(in crate::container_store) const INTENT_BYTES: usize = 460;
pub(in crate::container_store) const GUARD_BYTES: usize = 140;
pub(in crate::container_store) const READY_BYTES: usize = 236;
pub(in crate::container_store) const CUT_BYTES: usize = 72;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::container_store) struct Scope {
    pub(in crate::container_store) pool: Id,
    pub(in crate::container_store) container: Id,
    pub(in crate::container_store) operation: Id,
}

impl Scope {
    fn valid(self) -> bool {
        self.pool != [0; 16] && self.container != [0; 16] && self.operation != [0; 16]
    }

    fn write(self, out: &mut impl Write) -> io::Result<()> {
        out.write_all(&self.pool)?;
        out.write_all(&self.container)?;
        out.write_all(&self.operation)
    }

    fn read(input: &mut &[u8]) -> io::Result<Self> {
        let s = Self {
            pool: take(input)?,
            container: take(input)?,
            operation: take(input)?,
        };
        if !s.valid() {
            return Err(invalid("invalid DATA move scope"));
        }
        Ok(s)
    }
}

/// Native identity on retained SYSTEM or state, relative to a separately proved mount.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::container_store) struct Birth {
    pub(in crate::container_store) inode: u64,
    pub(in crate::container_store) seconds: i64,
    pub(in crate::container_store) nanos: u32,
}

impl Birth {
    fn valid(self) -> bool {
        self.inode != 0 && self.nanos < 1_000_000_000 && (self.seconds != 0 || self.nanos != 0)
    }

    fn write(self, out: &mut impl Write) -> io::Result<()> {
        if !self.valid() {
            return Err(invalid("invalid DATA move native birth"));
        }
        out.write_all(&self.inode.to_be_bytes())?;
        out.write_all(&self.seconds.to_be_bytes())?;
        out.write_all(&self.nanos.to_be_bytes())
    }

    fn read(input: &mut &[u8]) -> io::Result<Self> {
        let b = Self {
            inode: word(input)?,
            seconds: i64::from_be_bytes(take(input)?),
            nanos: u32::from_be_bytes(take(input)?),
        };
        if !b.valid() {
            return Err(invalid("invalid DATA move native birth"));
        }
        Ok(b)
    }
}

/// Constructed only after the given exact record's checksum and fields are checked.
#[derive(Debug)]
pub(in crate::container_store) struct Checked<T> {
    value: T,
    id: Digest,
}

impl<T> Checked<T> {
    pub(in crate::container_store) fn value(&self) -> &T {
        &self.value
    }

    pub(in crate::container_store) fn id(&self) -> Digest {
        self.id
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::container_store) struct Intent {
    pub(in crate::container_store) scope: Scope,
    pub(in crate::container_store) system: Digest,
    pub(in crate::container_store) source: Digest,
    pub(in crate::container_store) target: Digest,
    pub(in crate::container_store) source_operand: Digest,
    pub(in crate::container_store) target_operand: Digest,
    pub(in crate::container_store) state_parent: Birth,
    pub(in crate::container_store) routes_parent: Birth,
    pub(in crate::container_store) journal: Birth,
    pub(in crate::container_store) old_route: Route,
    pub(in crate::container_store) old_birth: Birth,
    pub(in crate::container_store) source_inode: u64,
    pub(in crate::container_store) rows: u64,
    pub(in crate::container_store) source_proof: Digest,
    pub(in crate::container_store) ledger: Digest,
}

impl Intent {
    fn require(&self) -> io::Result<()> {
        if !self.scope.valid()
            || !self.state_parent.valid()
            || !self.routes_parent.valid()
            || !self.journal.valid()
            || !self.old_birth.valid()
            || self.system == [0; 32]
            || self.source == [0; 32]
            || self.target == [0; 32]
            || self.source == self.target
            || self.system == self.source
            || self.system == self.target
            || self.source_operand == [0; 32]
            || self.target_operand == [0; 32]
            || self.source_operand == self.target_operand
            || self.source_inode == 0
            || !(1..=MAX_ROWS).contains(&self.rows)
            || self.old_route != route_record(self.scope.container, self.source)?
        {
            return Err(invalid("invalid DATA move intent binding"));
        }
        Ok(())
    }

    pub(in crate::container_store) fn encode(&self) -> io::Result<[u8; INTENT_BYTES]> {
        self.require()?;
        seal(INTENT, |out| {
            self.scope.write(out)?;
            for field in [
                self.system,
                self.source,
                self.target,
                self.source_operand,
                self.target_operand,
            ] {
                out.write_all(&field)?;
            }
            self.state_parent.write(out)?;
            self.routes_parent.write(out)?;
            self.journal.write(out)?;
            out.write_all(&self.old_route)?;
            self.old_birth.write(out)?;
            out.write_all(&self.source_inode.to_be_bytes())?;
            out.write_all(&self.rows.to_be_bytes())?;
            out.write_all(&self.source_proof)?;
            out.write_all(&self.ledger)
        })
    }

    pub(in crate::container_store) fn decode(raw: &[u8]) -> io::Result<Checked<Self>> {
        let (mut data, id) = unseal::<INTENT_BYTES>(INTENT, raw)?;
        let value = Self {
            scope: Scope::read(&mut data)?,
            system: take(&mut data)?,
            source: take(&mut data)?,
            target: take(&mut data)?,
            source_operand: take(&mut data)?,
            target_operand: take(&mut data)?,
            state_parent: Birth::read(&mut data)?,
            routes_parent: Birth::read(&mut data)?,
            journal: Birth::read(&mut data)?,
            old_route: take(&mut data)?,
            old_birth: Birth::read(&mut data)?,
            source_inode: word(&mut data)?,
            rows: word(&mut data)?,
            source_proof: take(&mut data)?,
            ledger: take(&mut data)?,
        };
        eof(data)?;
        value.require()?;
        Ok(Checked { value, id })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::container_store) struct Guard {
    pub(in crate::container_store) birth: Birth,
}

impl Guard {
    pub(in crate::container_store) fn encode(
        self,
        intent: &Checked<Intent>,
    ) -> io::Result<[u8; GUARD_BYTES]> {
        seal(GUARD, |out| {
            intent.value.scope.write(out)?;
            out.write_all(&intent.id)?;
            self.birth.write(out)
        })
    }

    pub(in crate::container_store) fn decode(
        raw: &[u8],
        intent: &Checked<Intent>,
    ) -> io::Result<Checked<Self>> {
        let (mut data, id) = unseal::<GUARD_BYTES>(GUARD, raw)?;
        let scope = Scope::read(&mut data)?;
        let original: Digest = take(&mut data)?;
        let value = Self {
            birth: Birth::read(&mut data)?,
        };
        eof(data)?;
        if scope != intent.value.scope || original != intent.id {
            return Err(invalid("DATA move guard belongs to a different intent"));
        }
        Ok(Checked { value, id })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::container_store) struct Ready {
    pub(in crate::container_store) guard: Birth,
    pub(in crate::container_store) target_inode: u64,
    pub(in crate::container_store) new_route: Route,
    pub(in crate::container_store) new_birth: Birth,
    pub(in crate::container_store) target_proof: Digest,
}

impl Ready {
    fn require(&self, intent: &Checked<Intent>) -> io::Result<()> {
        if self.target_inode == 0
            || !self.new_birth.valid()
            || self.new_birth.inode == intent.value.old_birth.inode
            || self.new_route != route_record(intent.value.scope.container, intent.value.target)?
        {
            return Err(invalid("invalid DATA move ready binding"));
        }
        Ok(())
    }

    fn guard_id(&self, intent: &Checked<Intent>) -> io::Result<Digest> {
        let raw = Guard { birth: self.guard }.encode(intent)?;
        raw[GUARD_BYTES - 32..]
            .try_into()
            .map_err(|_| invalid("invalid encoded DATA guard"))
    }

    pub(in crate::container_store) fn encode(
        &self,
        intent: &Checked<Intent>,
    ) -> io::Result<[u8; READY_BYTES]> {
        self.require(intent)?;
        seal(READY, |out| {
            out.write_all(&intent.id)?;
            self.guard.write(out)?;
            out.write_all(&self.guard_id(intent)?)?;
            out.write_all(&self.target_inode.to_be_bytes())?;
            out.write_all(&self.new_route)?;
            self.new_birth.write(out)?;
            out.write_all(&self.target_proof)
        })
    }

    pub(in crate::container_store) fn decode(
        raw: &[u8],
        intent: &Checked<Intent>,
    ) -> io::Result<Checked<Self>> {
        let (mut data, id) = unseal::<READY_BYTES>(READY, raw)?;
        let parent: Digest = take(&mut data)?;
        let guard = Birth::read(&mut data)?;
        let guard_id: Digest = take(&mut data)?;
        let value = Self {
            guard,
            target_inode: word(&mut data)?,
            new_route: take(&mut data)?,
            new_birth: Birth::read(&mut data)?,
            target_proof: take(&mut data)?,
        };
        eof(data)?;
        value.require(intent)?;
        if parent != intent.id || guard_id != value.guard_id(intent)? {
            return Err(invalid("DATA move ready has a different original lineage"));
        }
        Ok(Checked { value, id })
    }
}

#[derive(Debug)]
pub(in crate::container_store) struct Switched(Digest);

impl Switched {
    pub(in crate::container_store) fn encode(
        ready: &Checked<Ready>,
    ) -> io::Result<[u8; CUT_BYTES]> {
        seal(SWITCHED, |out| out.write_all(&ready.id))
    }

    fn decode(raw: &[u8], ready: &Checked<Ready>) -> io::Result<Self> {
        Ok(Self(linked(SWITCHED, raw, ready.id)?))
    }
}

pub(in crate::container_store) fn encode_done(switched: &Switched) -> io::Result<[u8; CUT_BYTES]> {
    seal(DONE, |out| out.write_all(&switched.0))
}

#[derive(Debug)]
pub(in crate::container_store) enum Phase {
    Intent,
    Guarded,
    Ready,
    Switched(Switched),
    /// Historical only. Exposing this ID does not assert an original G has gone.
    Done(Digest),
}

/// Every None supplied here must be proved native absence by the caller, not I/O failure.
#[derive(Clone, Copy)]
pub(in crate::container_store) struct JournalBytes<'a> {
    pub(in crate::container_store) intent: &'a [u8],
    pub(in crate::container_store) guard: Option<&'a [u8]>,
    pub(in crate::container_store) ready: Option<&'a [u8]>,
    pub(in crate::container_store) switched: Option<&'a [u8]>,
    pub(in crate::container_store) done: Option<&'a [u8]>,
}

#[derive(Debug)]
pub(in crate::container_store) struct Journal {
    intent: Checked<Intent>,
    guard: Option<Checked<Guard>>,
    ready: Option<Checked<Ready>>,
    phase: Phase,
}

impl Journal {
    pub(in crate::container_store) fn intent(&self) -> &Checked<Intent> {
        &self.intent
    }

    pub(in crate::container_store) fn guard(&self) -> Option<&Checked<Guard>> {
        self.guard.as_ref()
    }

    pub(in crate::container_store) fn ready(&self) -> Option<&Checked<Ready>> {
        self.ready.as_ref()
    }

    pub(in crate::container_store) fn phase(&self) -> &Phase {
        &self.phase
    }

    pub(in crate::container_store) fn decode(raw: JournalBytes<'_>) -> io::Result<Self> {
        let intent = Intent::decode(raw.intent)?;
        let guard = raw.guard.map(|g| Guard::decode(g, &intent)).transpose()?;
        let (ready, phase) = match (raw.ready, raw.switched, raw.done) {
            (None, None, None) if guard.is_some() => (None, Phase::Guarded),
            (None, None, None) => (None, Phase::Intent),
            (Some(r), switched, d) => {
                let ready = Ready::decode(r, &intent)?;
                if guard
                    .as_ref()
                    .is_some_and(|g| g.value.birth != ready.value.guard)
                    || (guard.is_none() && d.is_none())
                {
                    return Err(invalid("DATA move ready lost its exact original guard"));
                }
                let phase = match (switched, d) {
                    (None, None) => Phase::Ready,
                    (Some(s), None) => Phase::Switched(Switched::decode(s, &ready)?),
                    (Some(s), Some(d)) => {
                        let switched = Switched::decode(s, &ready)?;
                        Phase::Done(linked(DONE, d, switched.0)?)
                    }
                    (None, Some(_)) => return Err(invalid("DATA move phase gap")),
                };
                (Some(ready), phase)
            }
            (None, _, _) => return Err(invalid("DATA move phase gap")),
        };
        Ok(Self {
            intent,
            guard,
            ready,
            phase,
        })
    }
}

fn linked(magic: [u8; 8], raw: &[u8], expected: Digest) -> io::Result<Digest> {
    let (mut data, id) = unseal::<CUT_BYTES>(magic, raw)?;
    if take::<32>(&mut data)? != expected {
        return Err(invalid(
            "DATA move phase belongs to a different predecessor",
        ));
    }
    eof(data)?;
    Ok(id)
}

fn seal<const N: usize>(
    magic: [u8; 8],
    write: impl FnOnce(&mut Cursor<&mut [u8]>) -> io::Result<()>,
) -> io::Result<[u8; N]> {
    let mut raw = [0; N];
    let (data, id) = raw.split_at_mut(N - 32);
    let (tag, fields) = data.split_at_mut(8);
    tag.copy_from_slice(&magic);
    let mut output = Cursor::new(fields);
    write(&mut output)?;
    if output.position() != (N - 40) as u64 {
        return Err(invalid("short internal DATA move record"));
    }
    id.copy_from_slice(&digest(data)?);
    Ok(raw)
}

fn unseal<const N: usize>(magic: [u8; 8], raw: &[u8]) -> io::Result<(&[u8], Digest)> {
    if raw.len() != N || !raw.starts_with(&magic) {
        return Err(invalid("wrong DATA move record scope or length"));
    }
    let (data, id) = raw.split_at(N - 32);
    let id = id
        .try_into()
        .map_err(|_| invalid("invalid DATA move record checksum length"))?;
    if digest(data)? != id {
        return Err(invalid("invalid DATA move record checksum"));
    }
    Ok((&data[8..], id))
}

fn take<const N: usize>(input: &mut &[u8]) -> io::Result<[u8; N]> {
    let (head, rest) = input
        .split_first_chunk::<N>()
        .ok_or_else(|| invalid("short DATA move evidence"))?;
    *input = rest;
    Ok(*head)
}

fn word(input: &mut &[u8]) -> io::Result<u64> {
    Ok(u64::from_be_bytes(take(input)?))
}

fn eof(input: &[u8]) -> io::Result<()> {
    if !input.is_empty() {
        return Err(invalid("trailing DATA move evidence"));
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> io::Result<Digest> {
    let mut hash = Sha256::new();
    hash.update(bytes)?;
    Ok(hash.finish())
}

#[cfg(test)]
#[path = "../../../tests/unit/data_move_codec.rs"]
mod tests;
