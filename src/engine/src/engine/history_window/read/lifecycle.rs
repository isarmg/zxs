//! Current-authorized deletion evidence and local permanent path facts.
use crate::engine::history_window::HistoryCancellationCheckpoint;
use crate::engine::history_window::read::HistoryWindowRead;
use crate::{Engine, Result, State};
use crate::publication_evidence::SnapshotLifecycleBinding;
use crate::state_projection::lifecycles::{
    AdmittedLifecycle, LifecycleMembershipIndex, LifecycleSource,
};
use std::sync::{Arc, Mutex, Weak};
use storage_contract::file_lifecycle::{
    Cursor, Detail, Event, IntervalBody, LifecycleError, LinkRole, NodeLink, Page, References, Request, Timestamp,
};
use storage_contract::history_window::{
    HistoryFileId, HistoryTreeId, HistoryWindowCancellation,
};

mod navigation;
mod copy_merges;

struct EventBudget {
    count: usize,
    bytes: usize,
}

impl EventBudget {
    fn new() -> Self {
        Self { count: 0, bytes: 2 }
    }

    fn admit(&mut self, event: &Event) -> Result<()> {
        let bytes = json_codec::to_vec(event)?.len() + usize::from(self.count != 0);
        if self.count == storage_contract::file_lifecycle::MAX_AUDIT_EVENTS
            || bytes > storage_contract::file_lifecycle::MAX_AUDIT_BYTES - self.bytes
        {
            return Err(LifecycleError::Bytes.into_error().into());
        }
        self.count += 1;
        self.bytes += bytes;
        Ok(())
    }
}

/// One complete budget-checked audit plus one bounded membership index per
/// Engine. Keys own no authoritative State and exclude cursor/page size; those
/// remain request-specific. Membership reuse never replaces ledger admission.
#[derive(Default)]
pub(in crate::engine) struct LifecycleCache {
    slot: Mutex<Option<CachedAudit>>,
    memberships: Mutex<Option<CachedMemberships>>,
    snapshot: Mutex<Option<SnapshotLifecycleBinding>>,
}

struct AuditKey {
    state: Weak<State>,
    file: HistoryFileId,
    history: HistoryTreeId,
}

struct CachedAudit {
    key: AuditKey,
    events: Arc<[Event]>,
}

struct CachedMemberships {
    state: Weak<State>,
    index: Arc<LifecycleMembershipIndex>,
}

impl AuditKey {
    fn matches(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.state, &other.state)
            && self.file == other.file
            && self.history == other.history
    }
}

impl LifecycleCache {
    /// Issued only by installation of an admitted original disk snapshot. A
    /// changed recovered result yields None before it reaches this boundary.
    pub(in crate::engine) fn install_snapshot(&self, source: Option<SnapshotLifecycleBinding>) {
        if let Ok(mut slot) = self.snapshot.lock() {
            *slot = source;
        }
    }
    fn snapshot(&self, key: &AuditKey) -> Option<Arc<AdmittedLifecycle>> {
        let slot = self.snapshot.lock().ok()?;
        slot.as_ref()?.for_state(&key.state)
    }

    fn lookup(&self, key: &AuditKey) -> Option<Arc<[Event]>> {
        self.slot.lock().ok()?.as_ref()
            .filter(|cached| cached.key.matches(key))
            .map(|cached| Arc::clone(&cached.events))
    }

    fn membership_index(&self, key: &AuditKey) -> Option<Arc<LifecycleMembershipIndex>> {
        self.memberships.lock().ok()?.as_ref()
            .filter(|cached| Weak::ptr_eq(&cached.state, &key.state))
            .map(|cached| Arc::clone(&cached.index))
    }

    fn remember_memberships(&self, key: &AuditKey, index: LifecycleMembershipIndex) {
        if let Ok(mut slot) = self.memberships.lock()
            && key.state.strong_count() != 0
        {
            *slot = Some(CachedMemberships {
                state: key.state.clone(), index: Arc::new(index),
            });
        }
    }

    fn remember(&self, key: &AuditKey, events: Arc<[Event]>) {
        // No cache lock spans classification, decoding, sorting, or response
        // encoding. Concurrent misses do their own verification.
        if let Ok(mut slot) = self.slot.lock()
            && key.state.strong_count() != 0
        {
            *slot = Some(CachedAudit {
                key: AuditKey {
                    state: key.state.clone(),
                    file: key.file.clone(),
                    history: key.history.clone(),
                },
                events,
            });
        }
    }
}

pub struct FileLifecycleRead {
    read: HistoryWindowRead,
    request: Request,
    // Private projection excludes authentication, plans and control events. Only
    // this read needs permanent receipts; ordinary MetadataView still omits them.
    source: State,
    key: AuditKey,
    cache: Arc<LifecycleCache>,
}
impl FileLifecycleRead {
    /// On a cache miss, admit the entire permanent ledger and the target
    /// audit before returning any page. Continuation on the exact same State
    /// and file can reuse the bounded sorted summaries, not a prior authority
    /// check, context, cursor, or encoded response.
    pub fn page(&self) -> Result<Page> {
        self.read.cancellation.checkpoint()?;
        let binding = self.request.binding(&self.read.context, self.source.sequence);
        if self
            .request
            .cursor
            .as_ref()
            .is_some_and(|cursor| cursor.binding != binding)
        {
            return Err(LifecycleError::Changed.into_error().into());
        }
        let reusable = self.source_is_original();
        let events = match reusable.then(|| self.cache.lookup(&self.key)).flatten() {
            Some(events) => events,
            None => {
                let events: Arc<[Event]> = self.admit_events()?.into();
                self.read.cancellation.checkpoint()?;
                if reusable { self.cache.remember(&self.key, Arc::clone(&events)); }
                events
            }
        };
        self.read.cancellation.checkpoint()?;
        let start = match &self.request.cursor {
            Some(cursor) => events.binary_search_by_key(&cursor.after_key(), Event::order_key)
                .map_err(|_| LifecycleError::Cursor.into_error())? + 1,
            None => 0,
        };
        let end = events.len().min(start + self.request.limit());
        let selected = events[start..end].to_vec();
        let more = end < events.len();
        let next_cursor = if more {
            selected.last().map(|event| Cursor::local(binding, event))
        } else {
            None
        };
        let mut page = Page {
            file_id: self.request.file_id.clone(),
            history_root: self.request.history_root,
            context: self.read.context.clone(),
            observation_sequence: self.source.sequence,
            intervals: self.intervals(&selected,reusable)?,
            events: selected,
            next_cursor,
        };
        while json_codec::to_vec(&page)?.len()
            > storage_contract::file_lifecycle::MAX_RESPONSE_BYTES
        {
            self.read.cancellation.checkpoint()?;
            if let Some(interval) = page.intervals.iter_mut().rev().find(|i| matches!(i.body, IntervalBody::Complete { ref nodes, .. } if !nodes.is_empty())) {
                interval.body = IntervalBody::Open {};
                continue;
            }
            if page.events.len() <= 1 {
                return Err(LifecycleError::Bytes.into_error().into());
            }
            page.events.pop();
            page.intervals.pop();
            page.next_cursor = page.events.last().map(|event| Cursor::local(binding, event));
        }
        page.validate(&self.request)?;
        Ok(page)
    }

    fn admit_events(&self) -> Result<Vec<Event>> {
        let reusable = self.source_is_original();
        let admission = reusable.then(|| self.cache.snapshot(&self.key)).flatten();
        let source = if let Some(verified) = &admission {
            verified.source(&self.source, &self.request.file_id)
        } else if let Some(index) = reusable.then(|| self.cache.membership_index(&self.key)).flatten()
        {
            LifecycleSource::from_index(&self.source, &self.request.file_id, &index)
        } else {
            LifecycleSource::capture(&self.source, &self.request.file_id, || {
                self.read.cancellation.checkpoint()
            })?
        };
        self.read.cancellation.checkpoint()?;
        let mut budget = EventBudget::new();
        let mut events = crate::file_deletion::retained::events_from_source(
            source.view(),
            || self.read.cancellation.checkpoint(),
            |event| {
                if event.node.file_id != self.request.file_id.as_str()
                    || event.node.history_tree_id != self.read.context.history_tree_id.as_str()
                    || event.sequence == 0
                    || event.sequence > self.source.sequence
                {
                    return Err(LifecycleError::Response.into_error().into());
                }
                let deletion_id = event.node.node_id.parse()?;
                let detail = match event.action {
                    crate::file_deletion::retained::DeletionAction::Delete => {
                        Detail::Delete { deletion_id }
                    }
                    crate::file_deletion::retained::DeletionAction::Undo => {
                        Detail::Undo { deletion_id }
                    }
                    crate::file_deletion::retained::DeletionAction::Import => {
                        Detail::Import { deletion_id }
                    }
                };
                let occurred_at = matches!(detail, Detail::Delete { .. })
                    .then_some(Timestamp::from_unix_seconds(event.node.created_at));
                let event = Event {
                    operation_id: event.operation_id,
                    sequence: event.sequence,
                    principal: event.author.parse().map_err(|_| LifecycleError::SourceIncomplete.into_error())?,
                    references: self.frozen_heads(&event.node.branch_heads)?,

                    detail,
                    occurred_at,
                };
                budget.admit(&event)?;
                Ok(event)
            },
        )?;
        self.read.cancellation.checkpoint()?;
        if let Some(current) = self.source.file_deletions.get(self.request.file_id.as_str())
            && !events
                .iter()
                .any(|event| event.source_id().to_string() == current.node_id)
        {
            return Err(LifecycleError::SourceIncomplete.into_error().into());
        }
        let checked = admission.as_deref().map(|verified| verified.paths(&self.request.file_id));
        self.append_path_events(source.path_ledger(), checked, &mut events, &mut budget)?;
        self.merge_events(source.view(), &mut events, &mut budget)?;
        events.sort_by_key(Event::order_key);
        self.read.cancellation.checkpoint()?;
        for pair in events.windows(2) {
            self.read.cancellation.checkpoint()?;
            if pair[0].order_key() == pair[1].order_key() {
                return Err(LifecycleError::Response.into_error().into());
            }
        }
        if reusable && let Some(index) = source.into_index() {
            self.cache.remember_memberships(&self.key, index);
        }
        Ok(events)
    }

    /// The State key is weak and gives no read permission. Briefly borrow its
    /// original immutable tables to verify this private projection's lineage;
    /// no State or operation map is kept by either cache. A replaced capture
    /// (even value-equal) must re-admit its own source rather than reuse proof.
    fn source_is_original(&self) -> bool {
        let Some(state) = self.key.state.upgrade() else { return false; };
        macro_rules! unchanged {
            ($($field:ident),+ $(,)?) => {
                $(self.source.$field.changes_since(&state.$field).is_empty())&&+
            };
        }
        self.source.sequence == state.sequence && unchanged!(
            operations, operation_principals, histories, branches, commits,
            versions, file_deletions, directory_git_repositories, file_path_nodes, file_metadata_nodes,
            file_branch_merges, file_node_parents, content_states,
        )
    }

    fn append_path_events(
        &self,
        ledger: &crate::persist::path_changes::LedgerRead<'_>,
        checked: Option<&[crate::persist::path_changes::PathChanged]>,
        events: &mut Vec<Event>,
        budget: &mut EventBudget,
    ) -> Result<()> {
        let mut append = |fact: crate::persist::path_changes::PathChanged| {
            if fact.file_id.to_string() != self.request.file_id.as_str() {
                return Ok(());
            }
            if fact.history_tree_id.to_string() != self.read.context.history_tree_id.as_str()
            {
                return Err(crate::persist::path_changes::invalid());
            }
            let event = Event {
                operation_id: fact.operation_id,
                sequence: fact.sequence.get(),
                principal: fact.author,
                references: References::Complete { links: vec![
                    NodeLink {role:LinkRole::Before, node:self.body_at(&fact.before_version_id.to_string())?},
                    NodeLink {role:LinkRole::After, node:self.body_at(&fact.after_version_id.to_string())?},
                ] },
                occurred_at: Some(fact.occurred_at),
                detail: Detail::PathChanged {
                    event_id: fact.event_id,
                    before: fact.before,
                    after: fact.after,
                },
            };
            budget.admit(&event)?;
            events.push(event);
            Ok(())
        };
        if let Some(facts) = checked {
            for fact in facts {
                self.read.cancellation.checkpoint()?;
                append(fact.clone())?;
            }
            Ok(())
        } else {
            ledger.visit(|| self.read.cancellation.checkpoint(), append)
        }
    }
}
impl Engine {
    pub fn capture_file_lifecycle(
        &self,
        request: Request,
        cancellation: HistoryWindowCancellation,
    ) -> Result<FileLifecycleRead> {
        let read = self.capture_history_window(request.history_request(), cancellation)?;
        let mut source = self.metadata_view().business_projection().clone();
        source.operations = self.state.operations.clone();
        source.operation_principals = self.state.operation_principals.clone();
        let key = AuditKey {
            state: Arc::downgrade(&self.state),
            file: request.file_id.clone(),
            history: read.context.history_tree_id.clone(),
        };
        Ok(FileLifecycleRead {
            read,
            request,
            source,
            key,
            cache: Arc::clone(&self.lifecycle_cache),
        })
    }
    pub fn recheck_file_lifecycle(&self, read: &FileLifecycleRead) -> Result<()> {
        self.recheck_history_window(&read.read)?;
        // History manifests alone do not prove the captured permanent audit.
        // Conservatively restart pagination after ANY publication, even another
        // history's, instead of claiming that a graph root commits its receipts.
        if self.state.sequence != read.source.sequence
            || !Weak::ptr_eq(&Arc::downgrade(&self.state), &read.key.state)
        {
            return Err(LifecycleError::Changed.into_error().into());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/file_lifecycle_tests.rs"]
mod tests;
