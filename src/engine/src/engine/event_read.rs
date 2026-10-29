//! Detached reads of one fixed-size public event page.
use super::Engine;
use crate::{
    DiskConfig, Event, Result,
    disk::DiskResources,
    segment_store::{SegmentCaches, SegmentReader},
};
use std::sync::Arc;
use storage_contract::{EventSegmentRef, SegmentedEvents};

const PAGE_SIZE: usize = 1000;
const _: () = assert!(PAGE_SIZE <= storage_contract::EVENT_SEGMENT_SPAN);

struct EventRows {
    sealed: Vec<EventSegmentRef>,
    tail: Vec<Event>,
}
impl EventRows {
    fn capture(events: &SegmentedEvents, after: u64) -> Self {
        let tail = events.tail();
        let tail_serves = tail.first().is_some_and(|event| after >= event.sequence);
        let sealed = if tail_serves {
            Vec::new()
        } else {
            let refs = events.sealed();
            let start = refs
                .partition_point(|reference| reference.first_sequence <= after)
                .saturating_sub(1);
            refs[start..].iter().take(2).cloned().collect::<Vec<_>>()
        };
        // Only the first candidate can contain events at or before the cursor.
        let sealed_fills_page = sealed.len() == 2
            || sealed
                .first()
                .is_some_and(|reference| reference.first_sequence > after);
        let tail = if sealed_fills_page {
            Vec::new()
        } else {
            let start = tail.partition_point(|event| event.sequence <= after);
            tail[start..].iter().take(PAGE_SIZE).cloned().collect()
        };
        Self { sealed, tail }
    }
    fn read(self, reader: &SegmentReader<'_>, after: u64) -> Result<Vec<Event>> {
        let mut events =
            crate::segment_store::read_event_segments(&self.sealed, reader, after, PAGE_SIZE)?;
        let remaining = PAGE_SIZE - events.len();
        events.extend(self.tail.into_iter().take(remaining));
        Ok(events)
    }
}

/// One admitted event prefix, with at most two segment references and 1000
/// possible tail rows. Row count does not bound the strings' heap size.
/// Published immutable metadata is not garbage-collected at runtime.
pub struct EventPageReadPlan {
    resources: Arc<DiskResources>,
    members: Vec<DiskConfig>,
    caches: Arc<SegmentCaches>,
    rows: EventRows,
    after: u64,
}
impl EventPageReadPlan {
    /// Read hash-verified segments without holding the engine lock.
    ///
    /// # Errors
    /// Returns the selected members' I/O or metadata integrity failure.
    pub fn read(self) -> Result<Vec<Event>> {
        self.rows.read(
            &SegmentReader {
                resources: &self.resources,
                members: &self.members,
                caches: &self.caches,
            },
            self.after,
        )
    }
}
impl Engine {
    /// Capture the next public page (at most 1000 events) without disk reads.
    /// The caller separately admits the sequence cursor and current authority.
    pub fn prepare_event_page(&self, after: u64) -> EventPageReadPlan {
        EventPageReadPlan {
            resources: Arc::clone(&self.disk_resources),
            members: self.members(),
            caches: Arc::clone(&self.segment_caches),
            rows: EventRows::capture(&self.state.events, after),
            after,
        }
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/event_read_tests.rs"]
mod tests;
