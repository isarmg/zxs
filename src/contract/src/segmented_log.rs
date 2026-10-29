//! Segmented append logs for the State event array and permanent operation
//! receipts. Sealed segments are immutable, content-addressed metadata objects
//! read on demand; only the open tail and the sealed-segment directory stay
//! resident, so cloning a State copies at most one open segment per log.
//!
//! Sealing is decided at append time by format constants, never by budgets.
//! These containers do not read disks; the engine resolves sealed bodies.
use crate::{Error, ErrorCode, Event, OperationId, OperationResult, Result};
use data_serialization::{Deserialize, Deserializer, Serialize, Serializer};
use std::{collections::BTreeMap, sync::Arc};

/// Fixed layout markers, rejected strictly like every other format identifier.
macro_rules! format_marker {
    ($name:ident, $wire:literal) => {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        pub struct $name;
        impl Serialize for $name {
            fn serialize<S: Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                serializer.serialize_str($wire)
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                struct MarkerVisitor;
                impl data_serialization::de::Visitor<'_> for MarkerVisitor {
                    type Value = $name;
                    fn expecting(
                        &self,
                        formatter: &mut std::fmt::Formatter<'_>,
                    ) -> std::fmt::Result {
                        formatter.write_str(concat!("the fixed marker ", $wire))
                    }
                    fn visit_str<E: data_serialization::de::Error>(
                        self,
                        token: &str,
                    ) -> std::result::Result<Self::Value, E> {
                        if token == $wire {
                            Ok($name)
                        } else {
                            Err(E::unknown_variant(token, &[$wire]))
                        }
                    }
                }
                deserializer.deserialize_str(MarkerVisitor)
            }
        }
    };
}

/// Format constant: every sealed event segment holds exactly this many events.
pub const EVENT_SEGMENT_SPAN: usize = 1024;
/// Format constants: an operation segment seals at this record count, or once
/// its accumulated canonical record bytes reach this target, whichever first.
pub const OPERATION_SEGMENT_RECORDS: usize = 256;
pub const OPERATION_SEGMENT_TARGET_BYTES: usize = 1024 * 1024;

/// Object kinds for sealed segment bodies, shared with the engine object store.
pub const EVENT_SEGMENT_KIND: &str = "event-segment";
pub const OPERATION_SEGMENT_KIND: &str = "operation-segment";

/// Append provenance for one segmented log between two State identities.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SegmentChanges {
    /// Only these segment ordinals (sealed directory or tail) may differ.
    Segments(Arc<std::collections::BTreeSet<u64>>),
    All,
}

/// One sealed event segment: its body hash plus the first event sequence,
/// kept resident so sequence cursors can locate a segment without reading it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct EventSegmentRef {
    pub hash: String,
    #[serialization(with = "crate::u64_string")]
    pub first_sequence: u64,
}

/// Persisted body of a sealed or tail event segment.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct EventSegment {
    pub format: EventSegmentFormat,
    pub events: Vec<Event>,
}
format_marker!(EventSegmentFormat, "event-segment");

#[derive(Clone, Debug)]
pub struct SegmentedEvents {
    sealed: Arc<Vec<EventSegmentRef>>,
    tail: Vec<Event>,
    /// The records of the most recently sealed segment, retained only so the
    /// next snapshot encoding can install that one new object. Never read back
    /// as history; decoding a snapshot starts with None.
    fresh_seal: Option<Arc<EventSegment>>,
    identity: Arc<()>,
    base: Option<(Arc<()>, u64, usize)>,
}

impl SegmentedEvents {
    pub fn new() -> Self {
        Self {
            sealed: Arc::new(Vec::new()),
            tail: Vec::new(),
            fresh_seal: None,
            identity: Arc::new(()),
            base: None,
        }
    }
    pub(crate) fn from_parts(sealed: Vec<EventSegmentRef>, tail: Vec<Event>) -> Self {
        Self {
            sealed: Arc::new(sealed),
            tail,
            fresh_seal: None,
            identity: Arc::new(()),
            base: None,
        }
    }
    /// Build a log from plain events in order, sealing at the format span.
    /// Intended for fixtures and imports; publication appends use `push`.
    pub fn from_events(events: Vec<Event>) -> Result<Self> {
        let mut log = Self::new();
        for event in events {
            log.push(event)?;
        }
        Ok(log)
    }
    pub fn begin_tracking(&mut self) {
        // The retained fresh-seal body belongs to the transaction that sealed
        // it; a new transaction starts without one.
        self.fresh_seal = None;
        self.base = Some((
            Arc::clone(&self.identity),
            self.sealed.len() as u64,
            self.tail.len(),
        ));
    }
    /// Total number of events across sealed segments and the open tail.
    pub fn len(&self) -> u64 {
        self.sealed.len() as u64 * EVENT_SEGMENT_SPAN as u64 + self.tail.len() as u64
    }
    pub fn is_empty(&self) -> bool {
        self.sealed.is_empty() && self.tail.is_empty()
    }
    pub fn sealed(&self) -> &[EventSegmentRef] {
        &self.sealed
    }
    pub fn tail(&self) -> &[Event] {
        &self.tail
    }
    /// The ordinal of the open tail segment (one past the sealed directory).
    pub fn tail_ordinal(&self) -> u64 {
        self.sealed.len() as u64
    }
    pub fn fresh_seal(&self) -> Option<(u64, &EventSegment)> {
        self.fresh_seal
            .as_deref()
            .map(|segment| (self.sealed.len() as u64 - 1, segment))
    }
    /// Append one event; seals the tail into an immutable segment at the
    /// format span. Sealing failures are reported, never deferred silently.
    pub fn push(&mut self, event: Event) -> Result<()> {
        self.identity = Arc::new(());
        self.tail.push(event);
        if self.tail.len() == EVENT_SEGMENT_SPAN {
            self.seal_tail()?;
        }
        Ok(())
    }
    /// Replace the most recent event during recovery tail correction. The
    /// caller re-opens a just-sealed segment first when the tail is empty.
    pub fn replace_last(&mut self, event: Event) -> Result<()> {
        self.identity = Arc::new(());
        let Some(last) = self.tail.last_mut() else {
            return Err(Error::new(
                ErrorCode::StateInvalid,
                "event tail is empty; reopen the sealed segment before updating it",
            ));
        };
        *last = event;
        if self.tail.len() == EVENT_SEGMENT_SPAN {
            self.seal_tail()?;
        }
        Ok(())
    }
    fn seal_tail(&mut self) -> Result<()> {
        let segment = EventSegment {
            format: EventSegmentFormat,
            events: std::mem::take(&mut self.tail),
        };
        let hash = crate::object_hash(EVENT_SEGMENT_KIND, &segment)?;
        let first_sequence = segment.events[0].sequence;
        Arc::make_mut(&mut self.sealed).push(EventSegmentRef {
            hash,
            first_sequence,
        });
        self.fresh_seal = Some(Arc::new(segment));
        Ok(())
    }
    /// Re-open the last sealed segment as the tail for a boundary tail update.
    /// `records` must be that exact segment's decoded body.
    pub fn reopen_last_sealed(&mut self, records: Vec<Event>) -> Result<()> {
        let Some(reference) = self.sealed.last() else {
            return Err(Error::new(
                ErrorCode::StateInvalid,
                "no sealed event segment to reopen",
            ));
        };
        if !self.tail.is_empty() || records.len() != EVENT_SEGMENT_SPAN {
            return Err(Error::new(
                ErrorCode::StateInvalid,
                "only a full sealed segment reopens into an empty tail",
            ));
        }
        let segment = EventSegment {
            format: EventSegmentFormat,
            events: records,
        };
        if crate::object_hash(EVENT_SEGMENT_KIND, &segment)? != reference.hash {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "reopened event segment differs from its sealed hash",
            ));
        }
        self.identity = Arc::new(());
        Arc::make_mut(&mut self.sealed).pop();
        self.base = None;
        self.fresh_seal = None;
        self.tail = segment.events;
        Ok(())
    }
    pub fn changes_since(&self, before: &Self) -> SegmentChanges {
        let unchanged = Arc::ptr_eq(&self.identity, &before.identity);
        let appended = self.base.as_ref().is_some_and(|(base, sealed, tail)| {
            Arc::ptr_eq(base, &before.identity)
                && *sealed == before.sealed.len() as u64
                && *tail == before.tail.len()
        });
        if unchanged {
            return SegmentChanges::Segments(Arc::new(Default::default()));
        }
        if !appended || self.sealed.len() < before.sealed.len() {
            return SegmentChanges::All;
        }
        // Appends touch every segment ordinal from the previous tail onward.
        let dirty = (before.sealed.len() as u64..=self.sealed.len() as u64)
            .filter(|ordinal| *ordinal < self.sealed.len() as u64 || !self.tail.is_empty())
            .collect();
        SegmentChanges::Segments(Arc::new(dirty))
    }
}
impl Default for SegmentedEvents {
    fn default() -> Self {
        Self::new()
    }
}
impl PartialEq for SegmentedEvents {
    fn eq(&self, other: &Self) -> bool {
        self.sealed == other.sealed && self.tail == other.tail
    }
}
/// Logical wire shape used only by the single disk-read entry and fixtures.
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct SegmentedEventsWire {
    sealed: Vec<EventSegmentRef>,
    tail: Vec<Event>,
}
impl Serialize for SegmentedEvents {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        SegmentedEventsWire {
            sealed: self.sealed.as_ref().clone(),
            tail: self.tail.clone(),
        }
        .serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for SegmentedEvents {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let wire = SegmentedEventsWire::deserialize(deserializer)?;
        Ok(Self::from_parts(wire.sealed, wire.tail))
    }
}

/// Typed directory leaf value for one event segment (sealed or tail).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct EventDirectoryEntry {
    pub kind: EventSegmentFormat,
    pub hash: String,
    #[serialization(with = "crate::u64_string")]
    pub first_sequence: u64,
}

/// Typed directory leaf value for one operation segment. The counts are
/// recorded placement facts, re-verified against decoded bodies on full
/// admission and against the located receipt on point reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct OperationDirectoryEntry {
    pub kind: OperationSegmentFormat,
    pub hash: String,
    #[serialization(with = "crate::u64_string")]
    pub first_sequence: u64,
    #[serialization(with = "crate::u64_string")]
    pub first_ordinal: u64,
    #[serialization(with = "crate::u64_string")]
    pub records: u64,
}
impl OperationDirectoryEntry {
    pub fn reference(&self) -> OperationSegmentRef {
        OperationSegmentRef {
            hash: self.hash.clone(),
            first_sequence: self.first_sequence,
            first_ordinal: self.first_ordinal,
            records: self.records,
        }
    }
}

/// One permanent operation receipt: the audit/idempotency principal and the
/// frozen publication result, bound together in decision order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct OperationRecordEntry {
    pub principal: String,
    pub result: OperationResult,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct OperationSegmentRef {
    pub hash: String,
    #[serialization(with = "crate::u64_string")]
    pub first_sequence: u64,
    #[serialization(with = "crate::u64_string")]
    pub first_ordinal: u64,
    #[serialization(with = "crate::u64_string")]
    pub records: u64,
}

/// Persisted body of a sealed or tail operation segment.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct OperationSegment {
    pub format: OperationSegmentFormat,
    pub records: Vec<OperationRecordEntry>,
}
format_marker!(OperationSegmentFormat, "operation-segment");

#[derive(Clone, Debug)]
pub struct OperationLog {
    sealed: Arc<Vec<OperationSegmentRef>>,
    tail: Vec<OperationRecordEntry>,
    tail_first_ordinal: u64,
    tail_first_sequence: u64,
    tail_bytes: usize,
    fresh_seal: Option<Arc<OperationSegment>>,
    /// Complete id→ordinal index, present only between a cold decode and the
    /// next published basis. Full snapshot encoding consumes it; incremental
    /// encodings never need it. It is memory, not a second receipt authority.
    cold_index: Option<Arc<BTreeMap<OperationId, u64>>>,
    identity: Arc<()>,
    base: Option<(Arc<()>, u64, usize)>,
}

impl OperationLog {
    pub fn new() -> Self {
        Self {
            sealed: Arc::new(Vec::new()),
            tail: Vec::new(),
            tail_first_ordinal: 0,
            tail_first_sequence: 0,
            tail_bytes: 0,
            fresh_seal: None,
            cold_index: Some(Arc::new(BTreeMap::new())),
            identity: Arc::new(()),
            base: None,
        }
    }
    pub(crate) fn from_parts(
        sealed: Vec<OperationSegmentRef>,
        tail: Vec<OperationRecordEntry>,
        cold_index: Option<BTreeMap<OperationId, u64>>,
    ) -> Result<Self> {
        let tail_first_ordinal = sealed
            .last()
            .map(|segment| segment.first_ordinal + segment.records)
            .unwrap_or(0);
        let tail_first_sequence = tail.first().map(|entry| entry.result.sequence).unwrap_or(0);
        let mut tail_bytes = 0usize;
        for entry in &tail {
            tail_bytes = tail_bytes.saturating_add(crate::canonical_json(entry)?.len());
        }
        // Without sealed segments the complete index is derivable from the
        // resident tail alone; decoded states with sealed history must bring
        // the index the disk-read entry verified against the bodies.
        let cold_index = cold_index.or_else(|| {
            sealed.is_empty().then(|| {
                tail.iter()
                    .enumerate()
                    .map(|(ordinal, entry)| (entry.result.operation_id, ordinal as u64))
                    .collect()
            })
        });
        Ok(Self {
            sealed: Arc::new(sealed),
            tail,
            tail_first_ordinal,
            tail_first_sequence,
            tail_bytes,
            fresh_seal: None,
            cold_index: cold_index.map(Arc::new),
            identity: Arc::new(()),
            base: None,
        })
    }
    /// Build a log from receipts in decision order, sealing at the format
    /// thresholds. Intended for fixtures and imports.
    pub fn from_records(records: Vec<OperationRecordEntry>) -> Result<Self> {
        let mut log = Self::new();
        for record in records {
            log.push(record)?;
        }
        Ok(log)
    }
    pub fn begin_tracking(&mut self) {
        // The retained fresh-seal body belongs to the transaction that sealed
        // it; a new transaction starts without one.
        self.fresh_seal = None;
        self.base = Some((
            Arc::clone(&self.identity),
            self.sealed.len() as u64,
            self.tail.len(),
        ));
    }
    pub fn len(&self) -> u64 {
        self.tail_first_ordinal + self.tail.len() as u64
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn sealed(&self) -> &[OperationSegmentRef] {
        &self.sealed
    }
    pub fn tail(&self) -> &[OperationRecordEntry] {
        &self.tail
    }
    pub fn tail_first_ordinal(&self) -> u64 {
        self.tail_first_ordinal
    }
    pub fn tail_ordinal(&self) -> u64 {
        self.sealed.len() as u64
    }
    pub fn fresh_seal(&self) -> Option<(u64, &OperationSegment)> {
        self.fresh_seal
            .as_deref()
            .map(|segment| (self.sealed.len() as u64 - 1, segment))
    }
    /// The complete cold index, available only until the next published basis.
    pub fn cold_index(&self) -> Option<&BTreeMap<OperationId, u64>> {
        self.cold_index.as_deref()
    }
    /// A published basis now carries this log's encoding lineage; the one-shot
    /// cold index is no longer needed and its memory is released.
    pub fn release_cold_index(&mut self) {
        self.cold_index = None;
    }
    /// Reattach the complete id→ordinal index rebuilt from the published
    /// locator root, so a full encoding stays possible after the one-shot
    /// index was consumed (a local durable preparation forces one). The
    /// entries come from hash-verified pages; this check binds them to the
    /// resident log shape and resident record identities, never making the
    /// index a second receipt authority.
    pub fn attach_cold_index(&mut self, index: BTreeMap<OperationId, u64>) -> Result<()> {
        let differs = || {
            Error::new(
                ErrorCode::ObjectCorrupt,
                "operation locators differ from the receipt log",
            )
        };
        if index.len() as u64 != self.len() {
            return Err(differs());
        }
        let total = usize::try_from(self.len()).map_err(|_| differs())?;
        let mut seen = vec![false; total];
        for ordinal in index.values() {
            let slot = usize::try_from(*ordinal).ok().filter(|slot| *slot < total);
            match slot {
                Some(slot) if !seen[slot] => seen[slot] = true,
                _ => return Err(differs()),
            }
        }
        for (offset, entry) in self.tail.iter().enumerate() {
            if index.get(&entry.result.operation_id)
                != Some(&(self.tail_first_ordinal + offset as u64))
            {
                return Err(differs());
            }
        }
        if let Some(segment) = self.fresh_seal.as_deref() {
            let first = self.tail_first_ordinal - segment.records.len() as u64;
            for (offset, entry) in segment.records.iter().enumerate() {
                if index.get(&entry.result.operation_id) != Some(&(first + offset as u64)) {
                    return Err(differs());
                }
            }
        }
        self.cold_index = Some(Arc::new(index));
        Ok(())
    }
    /// Find a receipt in the resident tail by operation id.
    pub fn tail_record(&self, id: OperationId) -> Option<&OperationRecordEntry> {
        self.tail
            .iter()
            .find(|entry| entry.result.operation_id == id)
    }
    /// Append one receipt in decision order; seals at the format thresholds.
    pub fn push(&mut self, entry: OperationRecordEntry) -> Result<()> {
        self.identity = Arc::new(());
        if self.tail.is_empty() {
            self.tail_first_sequence = entry.result.sequence;
        }
        let bytes = crate::canonical_json(&entry)?.len();
        if let Some(index) = &mut self.cold_index {
            Arc::make_mut(index).insert(
                entry.result.operation_id,
                self.tail_first_ordinal + self.tail.len() as u64,
            );
        }
        self.tail.push(entry);
        self.tail_bytes = self.tail_bytes.saturating_add(bytes);
        if self.tail.len() == OPERATION_SEGMENT_RECORDS
            || self.tail_bytes >= OPERATION_SEGMENT_TARGET_BYTES
        {
            self.seal_tail()?;
        }
        Ok(())
    }
    fn seal_tail(&mut self) -> Result<()> {
        let segment = OperationSegment {
            format: OperationSegmentFormat,
            records: std::mem::take(&mut self.tail),
        };
        let hash = crate::object_hash(OPERATION_SEGMENT_KIND, &segment)?;
        let records = segment.records.len() as u64;
        Arc::make_mut(&mut self.sealed).push(OperationSegmentRef {
            hash,
            first_sequence: self.tail_first_sequence,
            first_ordinal: self.tail_first_ordinal,
            records,
        });
        self.tail_first_ordinal += records;
        self.tail_bytes = 0;
        self.fresh_seal = Some(Arc::new(segment));
        Ok(())
    }
    /// Replace the most recent receipt during recovery tail correction.
    pub fn replace_last(&mut self, entry: OperationRecordEntry) -> Result<()> {
        let Some(last) = self.tail.last_mut() else {
            return Err(Error::new(
                ErrorCode::StateInvalid,
                "operation tail is empty; reopen the sealed segment before updating it",
            ));
        };
        if last.result.operation_id != entry.result.operation_id {
            return Err(Error::new(
                ErrorCode::StateInvalid,
                "tail update targets a different operation",
            ));
        }
        self.identity = Arc::new(());
        let previous = crate::canonical_json(&*last)?.len();
        let bytes = crate::canonical_json(&entry)?.len();
        *last = entry;
        self.tail_bytes = self
            .tail_bytes
            .saturating_sub(previous)
            .saturating_add(bytes);
        // The seal rule is a pure function of the final record list; after a
        // tail update the segment re-seals exactly when it meets it again.
        if self.tail.len() == OPERATION_SEGMENT_RECORDS
            || self.tail_bytes >= OPERATION_SEGMENT_TARGET_BYTES
        {
            self.seal_tail()?;
        }
        Ok(())
    }
    /// Re-open the last sealed segment as the tail for a boundary tail update.
    pub fn reopen_last_sealed(&mut self, records: Vec<OperationRecordEntry>) -> Result<()> {
        let Some(reference) = self.sealed.last().cloned() else {
            return Err(Error::new(
                ErrorCode::StateInvalid,
                "no sealed operation segment to reopen",
            ));
        };
        if !self.tail.is_empty() || records.len() as u64 != reference.records {
            return Err(Error::new(
                ErrorCode::StateInvalid,
                "only the exact sealed segment reopens into an empty tail",
            ));
        }
        let segment = OperationSegment {
            format: OperationSegmentFormat,
            records,
        };
        if crate::object_hash(OPERATION_SEGMENT_KIND, &segment)? != reference.hash {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "reopened operation segment differs from its sealed hash",
            ));
        }
        self.identity = Arc::new(());
        Arc::make_mut(&mut self.sealed).pop();
        self.base = None;
        self.fresh_seal = None;
        self.tail_first_ordinal = reference.first_ordinal;
        self.tail_first_sequence = reference.first_sequence;
        self.tail = segment.records;
        self.tail_bytes = 0;
        for entry in &self.tail {
            self.tail_bytes = self
                .tail_bytes
                .saturating_add(crate::canonical_json(entry)?.len());
        }
        Ok(())
    }
    pub fn changes_since(&self, before: &Self) -> SegmentChanges {
        if Arc::ptr_eq(&self.identity, &before.identity) {
            return SegmentChanges::Segments(Arc::new(Default::default()));
        }
        let appended = self.base.as_ref().is_some_and(|(base, sealed, tail)| {
            Arc::ptr_eq(base, &before.identity)
                && *sealed == before.sealed.len() as u64
                && *tail == before.tail.len()
        });
        if !appended || self.sealed.len() < before.sealed.len() {
            return SegmentChanges::All;
        }
        let dirty = (before.sealed.len() as u64..=self.sealed.len() as u64)
            .filter(|ordinal| *ordinal < self.sealed.len() as u64 || !self.tail.is_empty())
            .collect();
        SegmentChanges::Segments(Arc::new(dirty))
    }
    /// Receipt ids appended onto `before`, for incremental locator encoding.
    pub fn appended_ids_since(&self, before: &Self) -> Option<Vec<(OperationId, u64)>> {
        match self.changes_since(before) {
            SegmentChanges::All => None,
            SegmentChanges::Segments(_) => {
                let start = before.len();
                Some(self.records_from(start))
            }
        }
    }
    /// (id, ordinal) pairs for resident records at and after `start`. Only the
    /// tail and the retained fresh seal are resident; earlier ordinals are not
    /// served here.
    fn records_from(&self, start: u64) -> Vec<(OperationId, u64)> {
        let mut out = Vec::new();
        if let Some((_, segment)) = self.fresh_seal() {
            let first = self.tail_first_ordinal - segment.records.len() as u64;
            for (offset, entry) in segment.records.iter().enumerate() {
                let ordinal = first + offset as u64;
                if ordinal >= start {
                    out.push((entry.result.operation_id, ordinal));
                }
            }
        }
        for (offset, entry) in self.tail.iter().enumerate() {
            let ordinal = self.tail_first_ordinal + offset as u64;
            if ordinal >= start {
                out.push((entry.result.operation_id, ordinal));
            }
        }
        out
    }
}
impl Default for OperationLog {
    fn default() -> Self {
        Self::new()
    }
}
impl PartialEq for OperationLog {
    fn eq(&self, other: &Self) -> bool {
        self.sealed == other.sealed && self.tail == other.tail
    }
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct OperationLogWire {
    sealed: Vec<OperationSegmentRef>,
    tail: Vec<OperationRecordEntry>,
    /// Complete id→ordinal index attached by the disk-read entry alone.
    #[serialization(default, skip_serializing_if = "Option::is_none")]
    index: Option<BTreeMap<String, String>>,
}
impl Serialize for OperationLog {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        OperationLogWire {
            sealed: self.sealed.as_ref().clone(),
            tail: self.tail.clone(),
            index: None,
        }
        .serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for OperationLog {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let wire = OperationLogWire::deserialize(deserializer)?;
        let index = wire
            .index
            .map(|index| {
                index
                    .into_iter()
                    .map(|(id, ordinal)| {
                        Ok::<_, Error>((id.parse()?, crate::parse_u64_decimal(&ordinal)?))
                    })
                    .collect::<Result<BTreeMap<_, _>>>()
            })
            .transpose()
            .map_err(data_serialization::de::Error::custom)?;
        Self::from_parts(wire.sealed, wire.tail, index)
            .map_err(data_serialization::de::Error::custom)
    }
}

#[cfg(test)]
#[path = "../../../test/contract/unit/segmented_log.rs"]
mod tests;
