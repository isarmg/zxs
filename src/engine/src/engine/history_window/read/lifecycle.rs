//! Current-authorized, path-free permanent lifecycle evidence.
use crate::engine::history_window::HistoryCancellationCheckpoint;
use crate::engine::history_window::read::HistoryWindowRead;
use crate::{Engine, Result, State};
use storage_contract::file_lifecycle::{Action, Cursor, Event, LifecycleError, Page, Request, Timestamp};
use storage_contract::history_window::HistoryWindowCancellation;

pub struct FileLifecycleRead {
    read: HistoryWindowRead,
    request: Request,
    // Private projection excludes authentication, plans and control events. Only
    // this read needs permanent receipts; ordinary MetadataView still omits them.
    source: State,
}
impl FileLifecycleRead {
    /// O(State membership + retained lifecycle) work per page, NOT O(page size).
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
        let projection = crate::state_projection::ProjectionSnapshot::new(&self.source);
        self.read.cancellation.checkpoint()?;
        if let Some(rows) = projection
            .view()
            .owners()
            .file_rows(self.request.file_id.as_str())
        {
            for operation in rows.history_copies.keys() {
                self.read.cancellation.checkpoint()?;
                // Graph-only copies predate retained audit evidence. The FILE
                // projection supports them, but this endpoint must not report
                // their unknown lifecycle prefix as an empty proven prefix.
                if self.source.operations[operation].kind == crate::business::history_copy::KIND {
                    return Err(LifecycleError::SourceIncomplete.into_error().into());
                }
            }
        }
        let events = crate::file_deletion::retained::events_projected(
            projection.view(),
            self.request.file_id.as_str(),
            || self.read.cancellation.checkpoint(),
            |event| {
                if event.node.file_id != self.request.file_id.as_str()
                    || event.node.history_tree_id != self.read.context.history_tree_id.as_str()
                    || event.sequence == 0
                    || event.sequence > self.source.sequence
                {
                    return Err(LifecycleError::Response.into_error().into());
                }
                let action = match event.action {
                    crate::file_deletion::retained::DeletionAction::Delete => Action::Delete,
                    crate::file_deletion::retained::DeletionAction::Undo => Action::Undo,
                    crate::file_deletion::retained::DeletionAction::Import => Action::Import,
                };
                Ok(Event {
                    operation_id: event.operation_id,
                    action,
                    sequence: event.sequence,
                    deletion_id: event.node.node_id.parse()?,
                    occurred_at: (action == Action::Delete)
                        .then_some(Timestamp::from_unix_seconds(event.node.created_at)),
                })
            },
        )?;
        self.read.cancellation.checkpoint()?;
        if let Some(current) = self.source.file_deletions.get(self.request.file_id.as_str())
            && !events
                .iter()
                .any(|event| event.deletion_id.to_string() == current.node_id)
        {
            return Err(LifecycleError::SourceIncomplete.into_error().into());
        }
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
                    .is_some_and(|cursor| cursor.after_operation_id == event.operation_id);
                if found_cursor
                    && self
                        .request
                        .cursor
                        .as_ref()
                        .is_some_and(|cursor| cursor.after_sequence != event.sequence)
                {
                    return Err(LifecycleError::Cursor.into_error().into());
                }
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
            })
        } else {
            None
        };
        let page = Page {
            file_id: self.request.file_id.clone(),
            history_root: self.request.history_root,
            context: self.read.context.clone(),
            observation_sequence: self.source.sequence,
            events: selected,
            next_cursor,
        };
        page.validate(&self.request)?;
        Ok(page)
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
