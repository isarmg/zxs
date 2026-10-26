//! Bounded request-local event pages, not a persistent history cache.
use data_serialization::Serialize;

#[derive(Debug)]
pub struct EventPage {
    pub events: Vec<storage_contract::Event>,
    pub next_cursor: u64,
}
#[derive(Serialize)]
pub(crate) struct EventPageResponse {
    events: Vec<EventResponse>,
    #[serialization(with = "storage_contract::u64_string")]
    next_cursor: u64,
}
#[derive(Serialize)]
struct EventResponse {
    #[serialization(with = "storage_contract::u64_string")]
    sequence: u64,
    operation_id: storage_contract::OperationId,
    kind: String,
    object_id: String,
}
impl From<EventPage> for EventPageResponse {
    fn from(page: EventPage) -> Self {
        let EventPage {
            events,
            next_cursor,
        } = page;
        Self {
            events: events
                .into_iter()
                .map(|event| {
                    let storage_contract::Event {
                        sequence,
                        operation_id,
                        kind,
                        object_id,
                    } = event;
                    EventResponse {
                        sequence,
                        operation_id,
                        kind,
                        object_id,
                    }
                })
                .collect(),
            next_cursor,
        }
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/backend_events.rs"]
mod tests;
