//! On-demand, hash-verified reads of sealed event/operation segments and
//! locator pages, with explicit in-process cache limits. Caches hold copies of
//! published immutable objects only: eviction never loses authority, a miss
//! re-reads and re-verifies from the configured metadata members.
use crate::disk::DiskResources;
use crate::publication_evidence::verified_graph_object_at;
use crate::{DiskConfig, Error, Result};
use std::borrow::{Borrow, Cow};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use storage_contract::{
    EVENT_SEGMENT_KIND, EVENT_SEGMENT_SPAN, ErrorCode, EventSegment, EventSegmentRef,
    OPERATION_SEGMENT_KIND, OperationRecordEntry, OperationSegment, OperationSegmentRef,
};

/// Explicit cache limits (see docs/architecture/namespace-roots-and-segmented-logs.md §3.4).
const MAX_CACHED_EVENT_SEGMENTS: usize = 8;
const MAX_CACHED_OPERATION_SEGMENTS: usize = 8;
const MAX_CACHED_LOCATOR_PAGE_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_CACHED_NAMESPACE_BYTES: usize = 8 * 1024 * 1024;

pub(crate) struct BoundedCache<V, K = String> {
    entries: BTreeMap<K, (V, usize)>,
    order: VecDeque<K>,
    bytes: usize,
    max_entries: usize,
    max_bytes: usize,
}
impl<V: Clone, K: Ord + Clone> BoundedCache<V, K> {
    pub(crate) fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            max_entries,
            max_bytes,
        }
    }
    pub(crate) fn get<Q: Ord + ?Sized>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        self.entries.get(key).map(|(value, _)| value.clone())
    }
    pub(crate) fn insert(&mut self, hash: K, value: V, bytes: usize) {
        // Oversized objects can still be served by the caller, but are never
        // retained beyond this cache's declared byte or entry budget.
        if bytes > self.max_bytes || self.max_entries == 0 {
            return;
        }
        if self.entries.contains_key(&hash) {
            return;
        }
        while self.entries.len() >= self.max_entries
            || (self.bytes.saturating_add(bytes) > self.max_bytes && !self.entries.is_empty())
        {
            let Some(evicted) = self.order.pop_front() else {
                break;
            };
            if let Some((_, evicted_bytes)) = self.entries.remove(&evicted) {
                self.bytes = self.bytes.saturating_sub(evicted_bytes);
            }
        }
        self.bytes = self.bytes.saturating_add(bytes);
        self.order.push_back(hash.clone());
        self.entries.insert(hash, (value, bytes));
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum NamespaceCacheKey {
    Page([u8; 64]),
    Index([u8; 64]),
}

impl NamespaceCacheKey {
    pub(crate) fn parse(reference: &storage_contract::NamespaceReference) -> Result<Self> {
        reference.validate()?;
        let hash = reference.hash.as_bytes().try_into()
            .expect("invariant: a validated namespace hash has 64 bytes");
        Ok(match reference.kind {
            storage_contract::NamespaceRefKind::Page => Self::Page(hash),
            storage_contract::NamespaceRefKind::Index => Self::Index(hash),
        })
    }
}

pub(crate) type NamespaceCache = BoundedCache<crate::namespace::NamespaceNode, NamespaceCacheKey>;

pub(crate) struct SegmentCaches {
    events: Mutex<BoundedCache<Arc<Vec<storage_contract::Event>>>>,
    operations: Mutex<BoundedCache<Arc<Vec<OperationRecordEntry>>>>,
    locator_pages: Mutex<BoundedCache<Arc<json_codec::Value>>>,
    namespace: Mutex<NamespaceCache>,
}
impl SegmentCaches {
    pub(crate) fn new() -> Self {
        Self {
            events: Mutex::new(BoundedCache::new(MAX_CACHED_EVENT_SEGMENTS, usize::MAX)),
            operations: Mutex::new(BoundedCache::new(MAX_CACHED_OPERATION_SEGMENTS, usize::MAX)),
            locator_pages: Mutex::new(BoundedCache::new(usize::MAX, MAX_CACHED_LOCATOR_PAGE_BYTES)),
            namespace: Mutex::new(BoundedCache::new(usize::MAX, MAX_CACHED_NAMESPACE_BYTES)),
        }
    }
    pub(crate) fn replace_namespace(&self, cache: NamespaceCache) {
        *Self::lock(&self.namespace) = cache;
    }

    fn lock<V, K>(
        cache: &Mutex<BoundedCache<V, K>>,
    ) -> std::sync::MutexGuard<'_, BoundedCache<V, K>> {
        // Cache contents are non-authoritative copies: a panic while holding
        // the lock cannot corrupt state worse than an empty cache, so a
        // poisoned cache continues with its current entries.
        match cache.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// One verified read scope: the fixed members this engine publishes to.
pub(crate) struct SegmentReader<'a> {
    pub(crate) resources: &'a DiskResources,
    pub(crate) members: &'a [DiskConfig],
    pub(crate) caches: &'a SegmentCaches,
    pub(crate) observe: Option<&'a dyn Fn() -> Result<()>>,
}

/// The unseen event suffix after a cursor, up to `limit` events: the resident
/// segment directory locates the first candidate segment by `first_sequence`,
/// only the needed sealed bodies are read (on demand, bounded cache), and the
/// resident tail serves recent cursors without any read.
pub(crate) fn events_page(
    events: &storage_contract::SegmentedEvents,
    reader: &SegmentReader<'_>,
    after: u64,
    limit: usize,
) -> Result<Vec<storage_contract::Event>> {
    let mut out = Vec::new();
    let sealed = events.sealed();
    let tail = events.tail();
    // A cursor at or past the tail's first sequence cannot miss any sealed
    // event (sequences are nondecreasing across the seal boundary), so the
    // resident tail serves it without reading any body.
    let tail_serves = tail.first().is_some_and(|first| after >= first.sequence);
    if !tail_serves {
        // First segment that could hold events beyond `after`: the one before
        // the first segment whose first sequence exceeds the cursor.
        let index = sealed
            .partition_point(|segment| segment.first_sequence <= after)
            .saturating_sub(1);
        out = read_event_segments(&sealed[index..], reader, after, limit)?;
    }
    if out.len() < limit {
        let rank = tail.partition_point(|event| event.sequence <= after);
        out.extend(tail[rank..].iter().take(limit - out.len()).cloned());
    }
    Ok(out)
}

pub(crate) fn read_event_segments(
    sealed: &[EventSegmentRef],
    reader: &SegmentReader<'_>,
    after: u64,
    limit: usize,
) -> Result<Vec<storage_contract::Event>> {
    let mut out = Vec::new();
    for reference in sealed {
        if out.len() == limit {
            break;
        }
        let segment = reader.event_segment(reference)?;
        let rank = segment.partition_point(|event| event.sequence <= after);
        out.extend(segment[rank..].iter().take(limit - out.len()).cloned());
    }
    Ok(out)
}

impl SegmentReader<'_> {
    pub(crate) fn checkpoint(&self) -> Result<()> {
        if let Some(observe) = self.observe {
            observe()?;
        }
        Ok(())
    }
    /// A sealed event segment: exactly the format span, nondecreasing
    /// sequences, first sequence matching its resident directory entry.
    pub(crate) fn event_segment(
        &self,
        reference: &EventSegmentRef,
    ) -> Result<Arc<Vec<storage_contract::Event>>> {
        self.checkpoint()?;
        if let Some(cached) = SegmentCaches::lock(&self.caches.events).get(&reference.hash) {
            return Ok(cached);
        }
        let object = verified_graph_object_at(
            self.resources,
            self.members,
            EVENT_SEGMENT_KIND,
            &reference.hash,
        )?;
        let bytes = object.canonical_len();
        let segment: EventSegment =
            json_codec::from_value(object.into_value()).map_err(|source| {
                Error::from(source).context(ErrorCode::ObjectCorrupt, "event segment is invalid")
            })?;
        if segment.events.len() != EVENT_SEGMENT_SPAN {
            return Err(sealed_shape("sealed event segment span differs"));
        }
        if segment.events[0].sequence != reference.first_sequence {
            return Err(sealed_shape("event segment first sequence differs"));
        }
        if !segment
            .events
            .iter()
            .map(|event| event.sequence)
            .is_sorted()
        {
            return Err(sealed_shape(
                "event segment sequences are not nondecreasing",
            ));
        }
        let events = Arc::new(segment.events);
        SegmentCaches::lock(&self.caches.events).insert(
            reference.hash.clone(),
            Arc::clone(&events),
            bytes,
        );
        Ok(events)
    }

    /// A sealed operation segment: exactly the recorded length, nondecreasing
    /// decision slots, first ordinal/sequence matching the directory entry.
    pub(crate) fn operation_segment(
        &self,
        reference: &OperationSegmentRef,
    ) -> Result<Arc<Vec<OperationRecordEntry>>> {
        self.checkpoint()?;
        if let Some(cached) = SegmentCaches::lock(&self.caches.operations).get(&reference.hash) {
            return Ok(cached);
        }
        let object = verified_graph_object_at(
            self.resources,
            self.members,
            OPERATION_SEGMENT_KIND,
            &reference.hash,
        )?;
        let bytes = object.canonical_len();
        let segment: OperationSegment =
            json_codec::from_value(object.into_value()).map_err(|source| {
                Error::from(source)
                    .context(ErrorCode::ObjectCorrupt, "operation segment is invalid")
            })?;
        if segment.records.len() as u64 != reference.records || segment.records.is_empty() {
            return Err(sealed_shape("sealed operation segment length differs"));
        }
        if segment.records[0].result.sequence != reference.first_sequence {
            return Err(sealed_shape("operation segment first sequence differs"));
        }
        if !segment
            .records
            .iter()
            .map(|record| record.result.sequence)
            .is_sorted()
        {
            return Err(sealed_shape(
                "operation segment sequences are not nondecreasing",
            ));
        }
        let records = Arc::new(segment.records);
        SegmentCaches::lock(&self.caches.operations).insert(
            reference.hash.clone(),
            Arc::clone(&records),
            bytes,
        );
        Ok(records)
    }

    /// Exact locator point read through the published radix root. `None` means
    /// the id was never bound; path damage stays an explicit error.
    pub(crate) fn locator_ordinal(
        &self,
        root: &crate::metadata_map::Reference,
        id: storage_contract::OperationId,
    ) -> Result<Option<u64>> {
        let value = crate::metadata_map::lookup(root, &id.to_string(), |kind, hash| {
            self.checkpoint()?;
            if let Some(cached) =
                SegmentCaches::lock(&self.caches.locator_pages).get(&format!("{kind}/{hash}"))
            {
                // Cached copies are re-authenticated on every hit; the cache
                // never weakens the hash check, it only avoids member I/O.
                return crate::journal::AuthenticatedMetadataObject::authenticate(
                    kind,
                    hash,
                    cached.as_ref().clone(),
                );
            }
            let object = verified_graph_object_at(self.resources, self.members, kind, hash)?;
            SegmentCaches::lock(&self.caches.locator_pages).insert(
                format!("{kind}/{hash}"),
                Arc::new(object.value().clone()),
                object.canonical_len(),
            );
            Ok(object)
        })?;
        value
            .map(|value| {
                let text: String = json_codec::from_value(value)?;
                storage_contract::parse_u64_decimal(&text)
            })
            .transpose()
            .map_err(|source| {
                Error::from(source)
                    .context(ErrorCode::ObjectCorrupt, "operation locator is invalid")
            })
    }
}

fn sealed_shape(reason: &'static str) -> Error {
    Error::new(ErrorCode::ObjectCorrupt, reason)
}

impl SegmentReader<'_> {
    /// One verified namespace page or index, through the bounded cache.
    pub(crate) fn namespace_node(
        &self,
        reference: &storage_contract::NamespaceReference,
    ) -> Result<crate::namespace::NamespaceNode> {
        self.checkpoint()?;
        let address = NamespaceCacheKey::parse(reference)?;
        if let Some(node) = SegmentCaches::lock(&self.caches.namespace).get(&address) {
            return Ok(node);
        }
        let object = verified_graph_object_at(
            self.resources,
            self.members,
            match reference.kind {
                storage_contract::NamespaceRefKind::Page => storage_contract::NAMESPACE_PAGE_KIND,
                storage_contract::NamespaceRefKind::Index => storage_contract::NAMESPACE_INDEX_KIND,
            },
            &reference.hash,
        )?;
        let canonical_len = object.canonical_len();
        let node =
            crate::namespace::NamespaceNode::decode(reference, object.into_value(), canonical_len)?;
        SegmentCaches::lock(&self.caches.namespace).insert(address, node.clone(), canonical_len);
        Ok(node)
    }
}

/// Shared permanent-receipt lookup over one published State: resident tail
/// and retained fresh seal first, then the one-shot cold index, then an exact
/// locator point read plus one sealed-segment read. `None` means the id was
/// never bound; path or segment damage stays an explicit error.
pub(crate) struct ReceiptLookup<'a, 'state> {
    pub(crate) state: &'state storage_contract::State,
    pub(crate) reader: SegmentReader<'a>,
    pub(crate) locator_root: Option<&'a crate::metadata_map::Reference>,
}
impl<'state> ReceiptLookup<'_, 'state> {
    pub(crate) fn find(
        &self,
        id: storage_contract::OperationId,
    ) -> Result<Option<OperationRecordEntry>> {
        Ok(self.find_view(id)?.map(Cow::into_owned))
    }

    pub(crate) fn find_view(
        &self,
        id: storage_contract::OperationId,
    ) -> Result<Option<Cow<'state, OperationRecordEntry>>> {
        if let Some(entry) = self.state.operations.tail_record(id) {
            return Ok(Some(Cow::Borrowed(entry)));
        }
        if let Some((_, segment)) = self.state.operations.fresh_seal()
            && let Some(entry) = segment
                .records
                .iter()
                .find(|entry| entry.result.operation_id == id)
        {
            return Ok(Some(Cow::Borrowed(entry)));
        }
        let ordinal = if let Some(index) = self.state.operations.cold_index() {
            match index.get(&id) {
                Some(ordinal) => *ordinal,
                None => return Ok(None),
            }
        } else {
            let root = self.locator_root.ok_or_else(|| {
                Error::new(
                    ErrorCode::StateInvalid,
                    "invariant: a published state carries its locator root or cold index",
                )
            })?;
            match self.reader.locator_ordinal(root, id)? {
                Some(ordinal) => ordinal,
                None => return Ok(None),
            }
        };
        let entry = self.by_ordinal(ordinal)?;
        if entry.result.operation_id != id {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "operation locator points at a different receipt",
            ));
        }
        Ok(Some(Cow::Owned(entry)))
    }
    /// The receipt at one log ordinal, from the resident tail or a sealed
    /// segment located through the resident directory.
    pub(crate) fn by_ordinal(&self, ordinal: u64) -> Result<OperationRecordEntry> {
        let log = &self.state.operations;
        let out_of_range = || {
            Error::new(
                ErrorCode::ObjectCorrupt,
                "operation ordinal is beyond the published log",
            )
        };
        if ordinal >= log.tail_first_ordinal() {
            let offset =
                usize::try_from(ordinal - log.tail_first_ordinal()).map_err(|_| out_of_range())?;
            return log.tail().get(offset).cloned().ok_or_else(out_of_range);
        }
        let sealed = log.sealed();
        let index = sealed
            .partition_point(|segment| segment.first_ordinal <= ordinal)
            .checked_sub(1)
            .ok_or_else(out_of_range)?;
        let reference = &sealed[index];
        let segment = self.reader.operation_segment(reference)?;
        let offset =
            usize::try_from(ordinal - reference.first_ordinal).map_err(|_| out_of_range())?;
        segment.get(offset).cloned().ok_or_else(out_of_range)
    }
}

#[cfg(test)]
#[path = "../../../test/engine/unit/segment_store_tests.rs"]
mod tests;
