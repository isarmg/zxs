//! Current-authorized deletion evidence and local permanent path facts.
use crate::engine::history_window::HistoryCancellationCheckpoint;
use crate::engine::history_window::read::HistoryWindowRead;
use crate::{Engine, Result, State};
use storage_contract::file_lifecycle::{
    Cursor, Detail, Event, LifecycleError, Page, Request, Timestamp,
};
use storage_contract::history_window::HistoryWindowCancellation;

pub struct FileLifecycleRead {
    read: HistoryWindowRead,
    request: Request,
    // Private projection excludes authentication, plans and control events. Only
    // this read needs permanent receipts; ordinary MetadataView still omits them.
    source: State,
}
impl FileLifecycleRead {
    /// O(permanent receipt ownership + retained lifecycle) work per page, NOT
    /// O(page size). Ownership scanning checks cancellation between receipts;
    /// one receipt's classification and decoding are not interruptible.
    /// Existing source admission caps retained events at 65,536 and copied audit
    /// envelopes at 12 MiB. Full audit admission retains summaries, not accumulated
    /// frontiers; one source receipt/envelope still needs complete decoding.
    /// The service's bounded history slot spans preparation and response encoding.
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
        let source = crate::state_projection::lifecycles::LifecycleSource::capture(
            &self.source,
            &self.request.file_id,
            || self.read.cancellation.checkpoint(),
        )?;
        self.read.cancellation.checkpoint()?;
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
                Ok(Event {
                    operation_id: event.operation_id,
                    sequence: event.sequence,
                    detail,
                    occurred_at,
                })
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
        self.append_path_events(&mut events)?;
        events.sort_by_key(Event::order_key);
        let mut found_cursor = self.request.cursor.is_none();
        let mut selected = Vec::with_capacity(self.request.limit());
        let mut more = false;
        for event in events {
            self.read.cancellation.checkpoint()?;
            if !found_cursor {
                found_cursor = self
                    .request
                    .cursor
                    .as_ref()
                    .is_some_and(|cursor| cursor.after_key() == event.order_key());
                continue;
            }
            if selected.len() == self.request.limit() {
                more = true;
                // The complete source identity was admitted before page selection.
                break;
            }
            selected.push(event);
        }
        if !found_cursor {
            return Err(LifecycleError::Cursor.into_error().into());
        }
        let next_cursor = if more {
            selected.last().map(|event| Cursor {
                binding,
                after_operation_id: event.operation_id,
                after_sequence: event.sequence,
                after_source_id: event.source_id(),
            })
        } else {
            None
        };
        let mut page = Page {
            file_id: self.request.file_id.clone(),
            history_root: self.request.history_root,
            context: self.read.context.clone(),
            observation_sequence: self.source.sequence,
            events: selected,
            next_cursor,
        };
        while json_codec::to_vec(&page)?.len()
            > storage_contract::file_lifecycle::MAX_RESPONSE_BYTES
        {
            self.read.cancellation.checkpoint()?;
            if page.events.len() <= 1 {
                return Err(LifecycleError::Bytes.into_error().into());
            }
            page.events.pop();
            page.next_cursor = page.events.last().map(|event| Cursor {
                binding,
                after_operation_id: event.operation_id,
                after_sequence: event.sequence,
                after_source_id: event.source_id(),
            });
        }
        page.validate(&self.request)?;
        Ok(page)
    }

    fn append_path_events(&self, events: &mut Vec<Event>) -> Result<()> {
        crate::persist::path_changes::visit_ledger(
            &self.source,
            || self.read.cancellation.checkpoint(),
            |fact| {
                if fact.file_id.to_string() != self.request.file_id.as_str() {
                    return Ok(());
                }
                if fact.history_tree_id.to_string() != self.read.context.history_tree_id.as_str()
                {
                    return Err(crate::persist::path_changes::invalid());
                }
                if events.len() == 65_536 {
                    return Err(LifecycleError::Bytes.into_error().into());
                }
                events.push(Event {
                    operation_id: fact.operation_id,
                    sequence: fact.sequence.get(),
                    occurred_at: Some(fact.occurred_at),
                    detail: Detail::PathChanged {
                        event_id: fact.event_id,
                        before: fact.before,
                        after: fact.after,
                    },
                });
                Ok(())
            },
        )
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
        Ok(FileLifecycleRead {
            read,
            request,
            source,
        })
    }
    pub fn recheck_file_lifecycle(&self, read: &FileLifecycleRead) -> Result<()> {
        self.recheck_history_window(&read.read)?;
        // History manifests alone do not prove the captured permanent audit.
        // Conservatively restart pagination after ANY publication, even another
        // history's, instead of claiming that a graph root commits its receipts.
        if self.state.sequence != read.source.sequence {
            return Err(LifecycleError::Changed.into_error().into());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/file_lifecycle_tests.rs"]
mod tests;
