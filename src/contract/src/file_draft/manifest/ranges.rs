//! Flat logical overlay; independent limits for live runs and referenced packs.
//! Operations never read or duplicate a historical FULL to express a partial write.
use super::{ManifestError, packed::{PACK_BYTES, PackId, WorkingPack}};
use data_serialization::{Deserialize, Deserializer, Serialize};
use std::{collections::{BTreeMap, BTreeSet}, fmt, marker::PhantomData};

pub const MAX_LIVE_RUNS: usize = 4096;
pub const MAX_LIVE_PACKS: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunSource {
    Zero {},
    Data { pack: PackId, #[serialization(with = "crate::u64_string")] offset: u64 },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct LogicalRun {
    #[serialization(with = "crate::u64_string")]
    start: u64,
    #[serialization(with = "crate::u64_string")]
    bytes: u64,
    source: RunSource,
}
impl LogicalRun {
    fn checked(start: u64, bytes: u64, source: RunSource) -> Result<Self, ManifestError> {
        if bytes == 0 || start.checked_add(bytes).is_none() { return Err(ManifestError::Coverage); }
        if let RunSource::Data { offset, .. } = source
            && offset.checked_add(bytes).is_none_or(|end| end > PACK_BYTES)
        { return Err(ManifestError::Coverage); }
        Ok(Self { start, bytes, source })
    }
    pub fn start(self) -> u64 { self.start }
    pub fn bytes(self) -> u64 { self.bytes }
    pub fn source(self) -> RunSource { self.source }
    // Used only after admission or checked construction.
    fn end(self) -> u64 { self.start + self.bytes }
    fn slice(self, start: u64, end: u64) -> Self {
        let source = match self.source {
            RunSource::Zero {} => RunSource::Zero {},
            RunSource::Data { pack, offset } => RunSource::Data { pack, offset: offset + (start - self.start) },
        };
        Self { start, bytes: end - start, source }
    }
    fn joins(self, other: Self) -> bool {
        self.end() == other.start && match (self.source, other.source) {
            (RunSource::Zero {}, RunSource::Zero {}) => true,
            (RunSource::Data { pack, offset }, RunSource::Data { pack: right, offset: next }) => {
                pack == right && offset + self.bytes == next
            }
            _ => false,
        }
    }
}

/// A gap is not explicit zero: its caller reads the selected base up to the
/// retained prefix and supplies sparse zero beyond it. EOF is a caller bound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadSpan { Gap, Zero, Data { pack: PackId, offset: u64 } }

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serialization(transparent)]
pub struct RunTable(Vec<LogicalRun>);
impl RunTable {
    pub fn runs(&self) -> &[LogicalRun] { &self.0 }
    pub fn at(&self, start: u64, end: u64) -> Result<(ReadSpan, u64), ManifestError> {
        if end <= start { return Err(ManifestError::Coverage); }
        let after = self.0.partition_point(|run| run.end() <= start);
        match self.0.get(after).copied() {
            None => Ok((ReadSpan::Gap, end - start)),
            Some(run) if run.start > start => Ok((ReadSpan::Gap, end.min(run.start) - start)),
            Some(run) => Ok((match run.source {
                RunSource::Zero {} => ReadSpan::Zero,
                RunSource::Data { pack, offset } => ReadSpan::Data { pack, offset: offset + (start - run.start) },
            }, end.min(run.end()) - start)),
        }
    }
    /// Whether appending physical bytes here extends this same private run.
    /// Even a zero-filled transport frame can be represented by stored bytes;
    /// alternating zero/nonzero frames must not fragment a sequential upload.
    pub(super) fn continues_data(&self, at: u64, pack: PackId, offset: u64) -> bool {
        let count = self.0.partition_point(|run| run.end() <= at);
        count.checked_sub(1).and_then(|i| self.0.get(i)).is_some_and(|r| {
            r.end() == at && matches!(r.source, RunSource::Data { pack: id, offset: start }
                if id == pack && start + r.bytes == offset)
        })
    }
    /// Ordered last-write-wins. Splice only intersected runs plus at most two
    /// neighbors. Check resulting live cardinality before modifying the table.
    pub fn replace(&mut self, start: u64, bytes: u64, source: RunSource) -> Result<(), ManifestError> {
        let insertion = LogicalRun::checked(start, bytes, source)?;
        let end = insertion.end();
        let first = self.0.partition_point(|r| r.end() <= start);
        let last = self.0.partition_point(|r| r.start < end);
        let from = first.saturating_sub(1);
        let to = (last + 1).min(self.0.len());
        let mut parts: Vec<LogicalRun> = Vec::with_capacity(5);
        let mut push = |run: LogicalRun| {
            if let Some(previous) = parts.last_mut() && previous.joins(run) {
                previous.bytes += run.bytes;
            } else { parts.push(run); }
        };
        if from < first { push(self.0[from]); }
        if first < last && self.0[first].start < start { push(self.0[first].slice(self.0[first].start, start)); }
        push(insertion);
        if first < last && self.0[last - 1].end() > end { push(self.0[last - 1].slice(end, self.0[last - 1].end())); }
        if last < to { push(self.0[last]); }
        let result_count = self.0.len() - (to - from) + parts.len();
        if result_count > MAX_LIVE_RUNS { return Err(ManifestError::Budget); }
        if result_count > MAX_LIVE_PACKS && matches!(source, RunSource::Data { .. }) {
            let mut distinct = BTreeSet::new();
            for r in self.0[..from].iter().chain(&parts).chain(&self.0[to..]) {
                if let RunSource::Data { pack, .. } = r.source { distinct.insert(pack); }
                if distinct.len() > MAX_LIVE_PACKS { return Err(ManifestError::Budget); }
            }
        }
        self.0.splice(from..to, parts);
        Ok(())
    }
    pub fn truncate(&mut self, length: u64) {
        let keep = self.0.partition_point(|r| r.start < length);
        self.0.truncate(keep);
        if let Some(last) = self.0.last_mut() && last.end() > length { last.bytes = length - last.start; }
    }
    pub fn references(&self) -> BTreeSet<PackId> {
        self.0.iter().filter_map(|r| match r.source {
            RunSource::Zero {} => None, RunSource::Data { pack, .. } => Some(pack),
        }).collect()
    }
    pub(super) fn from_wire(runs: Vec<LogicalRun>) -> Self { Self(runs) }
    pub(super) fn validate(&self, packs: &[WorkingPack], length: u64) -> Result<(), ManifestError> {
        let runs = &self.0;
        if runs.len() > MAX_LIVE_RUNS || packs.len() > MAX_LIVE_PACKS { return Err(ManifestError::Budget); }
        let mut geometry = BTreeMap::new();
        let mut entities = BTreeSet::new();
        let mut prior_id = None;
        for &pack in packs {
            if prior_id.is_some_and(|prior| prior >= pack.id()) || !entities.insert(pack.entity()) {
                return Err(ManifestError::Entity);
            }
            geometry.insert(pack.id(), pack.data_bytes());
            prior_id = Some(pack.id());
        }
        let mut physical = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut prior: Option<LogicalRun> = None;
        for &run in runs {
            LogicalRun::checked(run.start, run.bytes, run.source)?;
            if run.end() > length || prior.is_some_and(|p| p.end() > run.start || p.joins(run)) {
                return Err(ManifestError::Coverage);
            }
            if let RunSource::Data { pack, offset } = run.source {
                if geometry.get(&pack).is_none_or(|&n| offset + run.bytes > n)
                    || !physical.insert((pack, offset, offset + run.bytes))
                { return Err(ManifestError::Coverage); }
                seen.insert(pack);
            }
            prior = Some(run);
        }
        if seen.len() != packs.len() { return Err(ManifestError::Coverage); }
        let mut last = None;
        for (pack, offset, end) in physical {
            if last.is_some_and(|(id, high)| id == pack && offset < high) { return Err(ManifestError::Coverage); }
            last = Some((pack, end));
        }
        Ok(())
    }
    /// Any run reusing old physical data must be the same logical mapping in
    /// the preceding view. A discarded or truncated slice cannot reappear.
    pub(super) fn retains(&self, previous: &Self, old: &BTreeSet<PackId>, cut: Option<u64>) -> Result<(), ManifestError> {
        for &run in &self.0 {
            let RunSource::Data { pack, offset } = run.source else { continue; };
            if !old.contains(&pack) { continue; }
            if cut.is_some_and(|eof| run.end() > eof) { return Err(ManifestError::Predecessor); }
            let (span, count) = previous.at(run.start, run.end())?;
            if span != (ReadSpan::Data { pack, offset }) || count != run.bytes { return Err(ManifestError::Predecessor); }
        }
        // A previously patched interval before the new cut may be superseded,
        // but cannot become an implicit gap exposing older base bytes again.
        for &run in &previous.0 {
            let end = cut.map_or(run.end(), |n| n.min(run.end()));
            let mut pos = run.start;
            while pos < end {
                let (span, bytes) = self.at(pos, end)?;
                if matches!(span, ReadSpan::Gap) { return Err(ManifestError::Predecessor); }
                pos += bytes;
            }
        }
        Ok(())
    }
}

pub(super) fn bounded<'de, D: Deserializer<'de>, T: Deserialize<'de>, const N: usize>(d: D) -> Result<Vec<T>, D::Error> {
    struct Visitor<T, const N: usize>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const N: usize> data_serialization::de::Visitor<'de> for Visitor<T, N> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "at most {N} working material entries") }
        fn visit_seq<A: data_serialization::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
            let mut result = Vec::new();
            while let Some(value) = seq.next_element()? {
                if result.len() == N { return Err(data_serialization::de::Error::custom(ManifestError::Budget)); }
                result.push(value);
            }
            Ok(result)
        }
    }
    d.deserialize_seq(Visitor::<T, N>(PhantomData))
}

#[cfg(test)]
#[path = "../../../../../test/contract/unit/packed_draft_ranges.rs"]
mod tests;
