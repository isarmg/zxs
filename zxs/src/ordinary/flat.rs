//! A FILE's complete logical address space. All data terminals refer to an
//! immutable issued material, never to a parent node or mutable pathname.
//! The index describes bytes; it cannot authenticate or open its materials.
use crate::{FileId, OperationId, fs::invalid};
use std::{collections::BTreeMap, io};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Material {
    file: FileId,
    issuer: OperationId,
    slot: u32,
    size: u64,
}
impl Material {
    pub(super) fn declared(file: FileId, issuer: OperationId, slot: u32, size: u64) -> io::Result<Self> {
        if file.0 == [0; 16] || issuer.0 == [0; 16] || size > i64::MAX as u64 {
            return Err(invalid("invalid FILE material identifier or length"));
        }
        Ok(Self { file, issuer, slot, size })
    }
    pub(super) fn file(self) -> FileId { self.file }
    pub(super) fn size(self) -> u64 { self.size }
    #[cfg(test)]
    pub(super) fn slot(self) -> u32 { self.slot }
    pub(in crate::ordinary) fn key(self) -> (OperationId, u32) { (self.issuer, self.slot) }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Leaf { Zero, Data(Material) }
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Row { end: u64, terminal: Option<u32>, offset: u64 }

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Index {
    file: FileId,
    rows: Vec<Row>, // exclusive ends strictly increase; first start is zero
    materials: Vec<Material>, // unique in first-use order
}

// This is a bound on actual lookup-table entries, not on the number of
// versions. Independent of architecture, and above the existing journal's
// 3 * MAX_DECISIONS + 1 possible logical cuts. Not an on-disk ABI.
const MAX_ROWS: usize = 131_072;

pub(super) struct Build {
    index: Index,
    numbers: BTreeMap<(OperationId, u32), u32>,
}
impl Build {
    pub(super) fn new(file: FileId) -> io::Result<Self> {
        if file.0 == [0; 16] { return Err(invalid("missing FILE for direct index")); }
        Ok(Self { index: Index { file, rows: Vec::new(), materials: Vec::new() }, numbers: BTreeMap::new() })
    }
    pub(super) fn add(&mut self, leaf: Leaf, offset: u64, bytes: u64) -> io::Result<()> {
        if bytes == 0 { return Err(invalid("empty direct-index segment")); }
        let start = self.index.len();
        let end = start.checked_add(bytes).filter(|v| *v <= i64::MAX as u64)
            .ok_or_else(|| invalid("direct index exceeds native offsets"))?;
        let rows = &mut self.index.rows;
        let count = rows.len();
        let previous_start = if count > 1 { rows[count - 2].end } else { 0 };
        let terminal = match leaf {
            Leaf::Zero if offset != 0 => return Err(invalid("zero segment has a material offset")),
            Leaf::Zero => None,
            Leaf::Data(m) => {
                if m.file != self.index.file || offset.checked_add(bytes).is_none_or(|n| n > m.size) {
                    return Err(invalid("segment exceeds its original FILE material"));
                }
                if let Some(&i) = self.numbers.get(&m.key()) {
                    if self.index.materials[i as usize] != m {
                        return Err(invalid("one original material cannot have two lengths"));
                    }
                    Some(i)
                } else {
                    // A previously unseen material always needs a new row.
                    if count >= MAX_ROWS { return Err(invalid("too many direct index rows")); }
                    let i = self.index.materials.len() as u32; // <= count <= MAX_ROWS
                    rows.try_reserve(1).map_err(|_| invalid("cannot reserve a direct index row"))?;
                    self.index.materials.try_reserve(1).map_err(|_| invalid("cannot reserve material descriptor"))?;
                    self.index.materials.push(m);
                    self.numbers.insert(m.key(), i);
                    Some(i)
                }
            }
        };
        if let Some(last) = rows.last_mut() {
            if last.terminal == terminal && (terminal.is_none() || last.offset.checked_add(start - previous_start) == Some(offset)) {
                last.end = end;
                return Ok(());
            }
        }
        if count >= MAX_ROWS { return Err(invalid("too many direct index rows")); }
        rows.try_reserve(1).map_err(|_| invalid("cannot reserve a direct index row"))?;
        rows.push(Row { end, terminal, offset });
        Ok(())
    }
    pub(super) fn inherit(&mut self, base: &Index, at: u64, bytes: u64) -> io::Result<()> {
        if base.file != self.index.file { return Err(invalid("cannot inherit another FILE's address map")); }
        base.visit(at, bytes, |leaf, pos, n| self.add(leaf, pos, n))
    }
    pub(super) fn finish(self, expected: u64) -> io::Result<Index> {
        if self.index.len() != expected { return Err(invalid("direct index does not cover its exact logical length")); }
        Ok(self.index)
    }
}

impl Index {
    pub(super) fn full(material: Material) -> io::Result<Self> {
        let mut b = Build::new(material.file)?;
        if material.size != 0 { b.add(Leaf::Data(material), 0, material.size)?; }
        b.finish(material.size)
    }
    pub(super) fn file(&self) -> FileId { self.file }
    pub(super) fn len(&self) -> u64 { self.rows.last().map_or(0, |r| r.end) }

    /// Overlay an ordinary write with its stated final length. A resize
    /// takes no replacement. New space past the selected EOF is zero.
    pub(super) fn edit(&self, at: u64, replacement: Option<(Material, u64, u64)>, final_len: u64) -> io::Result<Self> {
        let n = replacement.map_or(0, |(_, _, n)| n);
        let stop = at.checked_add(n).filter(|v| *v <= final_len)
            .ok_or_else(|| invalid("write lies outside the new logical length"))?;
        if final_len > i64::MAX as u64 || (replacement.is_none() && at != 0) {
            return Err(invalid("invalid direct-index edit or resize"));
        }
        if let Some((m, offset, len)) = replacement {
            if len == 0 || m.file != self.file || offset.checked_add(len).is_none_or(|n| n > m.size)
                || self.materials.iter().any(|old| old.key() == m.key() && *old != m) {
                return Err(invalid("invalid replacement material range or original extent"));
            }
        }
        let mut b = Build::new(self.file)?;
        let keep = |out: &mut Build, lo: u64, hi: u64| -> io::Result<()> {
            let end = hi.min(self.len());
            if lo < end { out.inherit(self, lo, end - lo)?; }
            let zero = lo.max(self.len());
            if zero < hi { out.add(Leaf::Zero, 0, hi - zero)?; }
            Ok(())
        };
        keep(&mut b, 0, at)?;
        if let Some((leaf, physical, bytes)) = replacement { b.add(Leaf::Data(leaf), physical, bytes)?; }
        keep(&mut b, stop, final_len)?;
        b.finish(final_len)
    }

    /// One binary search then only intersecting rows; constant-time lookup
    /// of each material. No access to a parent version or a file descriptor.
    pub(super) fn visit(&self, at: u64, bytes: u64, mut emit: impl FnMut(Leaf, u64, u64) -> io::Result<()>) -> io::Result<()> {
        let end = at.checked_add(bytes).filter(|v| *v <= self.len())
            .ok_or_else(|| invalid("read outside the selected direct index"))?;
        if at == end { return Ok(()); }
        let first = self.rows.partition_point(|row| row.end <= at);
        let mut start = if first == 0 { 0 } else { self.rows[first - 1].end };
        for row in &self.rows[first..] {
            let lo = at.max(start);
            let hi = end.min(row.end);
            match row.terminal {
                None => emit(Leaf::Zero, 0, hi - lo)?,
                Some(i) => emit(Leaf::Data(self.materials[i as usize]), row.offset + (lo - start), hi - lo)?,
            }
            if hi == end { return Ok(()); }
            start = row.end;
        }
        Err(invalid("incomplete direct index"))
    }
}

#[cfg(test)]
#[path="../../tests/unit/flat_index.rs"]
mod tests;
pub(in crate::ordinary) mod stored;
mod blocks;
pub(in crate::ordinary) mod logical;
pub(in crate::ordinary) use blocks::{Ref as MaterialRef,write_candidate as write_candidate_checksums};
pub(in crate::ordinary) mod body;
pub(in crate::ordinary) mod local;
pub(crate) mod work;
