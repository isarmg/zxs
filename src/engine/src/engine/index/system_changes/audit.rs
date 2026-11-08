//! Non-secret audit rows, preserving native map keys and array positions.
use super::{Kind as RowKind, *};
use data_serialization::Deserialize;
use storage_contract::SequenceChanges;

/// Original idempotency/audit owner, not an account or authorization grant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SystemOperationPrincipal {
    pub operation_id: OperationId,
    pub principal: String,
}

/// One event at its exact position in the captured State event array. Identical
/// values and multiple events for one operation keep their distinct positions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SystemAuditEvent {
    #[serialization(with = "storage_contract::u64_string")]
    pub ordinal: u64,
    pub event: Event,
}

/// Scalars from the same admitted State as the projected maps and event array.
/// These are observations, never a publication, lease or recovery capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SystemStateMetadata {
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub business_generation: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub topology_generation: u64,
}

#[derive(Serialize)]
struct PrincipalRef<'a> {
    operation_id: OperationId,
    principal: &'a str,
}

#[derive(Serialize)]
struct EventRef<'a> {
    #[serialization(with = "storage_contract::u64_string")]
    ordinal: u64,
    event: &'a Event,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Principal,
    Event,
    State,
}

impl Kind {
    pub(super) fn sql(self) -> i64 {
        match self {
            Self::Principal => 36,
            Self::Event => 37,
            Self::State => 38,
        }
    }

    pub(super) fn payload(self, state: &State, id: &str) -> Result<Vec<u8>> {
        match self {
            Self::Principal => {
                let operation_id = id
                    .parse::<OperationId>()
                    .expect("invariant: trusted audit order uses canonical operation IDs");
                encode(&PrincipalRef {
                    operation_id,
                    principal: state
                        .operation_principals
                        .get(&operation_id)
                        .expect("invariant: audit principal key belongs to admitted State"),
                })
            }
            Self::Event => {
                let ordinal = id
                    .parse::<usize>()
                    .expect("invariant: trusted event order uses array indices");
                encode(&EventRef {
                    ordinal: ordinal as u64,
                    event: state
                        .events
                        .get(ordinal)
                        .expect("invariant: audit event position belongs to admitted State"),
                })
            }
            Self::State => encode(&metadata(state)),
        }
    }
}

fn metadata(state: &State) -> SystemStateMetadata {
    SystemStateMetadata {
        sequence: state.sequence,
        business_generation: state.business_generation,
        topology_generation: state.topology_generation,
    }
}

fn ordinal_key(index: usize) -> String {
    // Current Engine targets have at most 64-bit usize. Twenty decimal digits
    // retain numeric order across the full u64 domain without SQLite INTEGER.
    format!("{index:020}")
}

#[derive(Default)]
pub(super) struct Order {
    principals: BTreeSet<(u64, String)>,
    events: BTreeSet<(u64, String)>,
    state: BTreeSet<(u64, String)>,
}

impl Order {
    pub(super) fn rows(&self, kind: Kind) -> &BTreeSet<(u64, String)> {
        match kind {
            Kind::Principal => &self.principals,
            Kind::Event => &self.events,
            Kind::State => &self.state,
        }
    }
}

enum EventDelta {
    Append { start: usize },
    Replace,
}

pub(super) struct Delta {
    principals: Vec<OperationId>,
    events: EventDelta,
}

impl Delta {
    pub(super) fn new(before: Option<&State>, after: &State, principals: Vec<OperationId>) -> Self {
        let events = match before.map(|old| after.events.changes_since(&old.events)) {
            Some(SequenceChanges::Append { start }) => EventDelta::Append { start },
            Some(SequenceChanges::All) | None => EventDelta::Replace,
        };
        Self { principals, events }
    }

    fn first_event(&self) -> usize {
        match self.events {
            EventDelta::Append { start } => start,
            EventDelta::Replace => 0,
        }
    }

    pub(super) fn update_order(&self, order: &mut Order, after: &State) {
        for id in &self.principals {
            let key = (0, id.to_string());
            if after.operation_principals.contains_key(id) {
                order.principals.insert(key);
            } else {
                order.principals.remove(&key);
            }
        }
        if matches!(self.events, EventDelta::Replace) {
            order.events.clear();
        }
        for index in self.first_event()..after.events.len() {
            order.events.insert((0, ordinal_key(index)));
        }
        order.state.insert((0, "state".into()));
    }

    pub(super) async fn project(
        &self,
        connection: &mut sqlite_access::SqliteConnection,
        after: &State,
    ) -> std::result::Result<(), ProjectionFailure> {
        for id in &self.principals {
            let row = after
                .operation_principals
                .get(id)
                .map(|principal| PrincipalRef {
                    operation_id: *id,
                    principal,
                });
            store_row(
                connection,
                RowKind::Audit(Kind::Principal),
                &id.to_string(),
                row.as_ref().map(|row| (0, row)),
            )
            .await?;
        }
        if matches!(self.events, EventDelta::Replace) {
            // Unknown array lineage can replace, reorder or truncate its
            // suffix. Never infer append-only semantics from event values.
            sqlite_access::query("DELETE FROM system_changes WHERE kind=?")
                .bind(Kind::Event.sql())
                .execute(&mut *connection)
                .await?;
        }
        for index in self.first_event()..after.events.len() {
            let row = EventRef {
                ordinal: index as u64,
                event: &after.events[index],
            };
            store_row(
                connection,
                RowKind::Audit(Kind::Event),
                &ordinal_key(index),
                Some((0, &row)),
            )
            .await?;
        }
        store_row(
            connection,
            RowKind::Audit(Kind::State),
            "state",
            Some((0, &metadata(after))),
        )
        .await
    }
}

impl Engine {
    /// Read complete operation-owner mappings from the admitted global State.
    /// This does not read passwords, authenticate an actor or grant a replay.
    /// The caller must authorize global metadata access. Uses the same bounded,
    /// State/projection-bound pages and unavailable behavior as other system rows.
    pub fn system_operation_principals(
        &self,
        cursor: Option<&SystemChangeCursor>,
        limit: usize,
    ) -> std::result::Result<SystemChangePage<SystemOperationPrincipal>, SystemChangeReadError>
    {
        self.system_change_page(RowKind::Audit(Kind::Principal), cursor, limit)
    }

    /// Read the exact retained event array in ordinal order. Event sequence and
    /// operation IDs are values, not identities used to collapse duplicate rows.
    /// The caller must authorize global metadata access; this is not a live feed.
    pub fn system_audit_events(
        &self,
        cursor: Option<&SystemChangeCursor>,
        limit: usize,
    ) -> std::result::Result<SystemChangePage<SystemAuditEvent>, SystemChangeReadError> {
        self.system_change_page(RowKind::Audit(Kind::Event), cursor, limit)
    }

    /// Read the three State scalars from the same transactional system database.
    /// The caller must authorize global metadata access. No value is an authority
    /// selection, current lease, physical-health result or independent restore proof.
    pub fn system_state_metadata(
        &self,
    ) -> std::result::Result<SystemStateMetadata, SystemChangeReadError> {
        let page = self.system_change_page(RowKind::Audit(Kind::State), None, 1)?;
        Ok(page
            .records
            .into_iter()
            .next()
            .expect("invariant: exact State metadata page contains its singleton"))
    }
}
