//! Admit a history root, project outside the mutex, then re-admit the result.
use super::*;
use crate::backend_graph::{GraphRead, GraphReadControl, HistoryGraphResponse, ReadHistoryGraph};
use storage_contract::history_graph::{DeletionsPage, GraphPage, PendingPage, TextPage};

fn changed() -> ApiError {
    ApiError::conflict(
        storage_contract::ErrorCode::HistoryChanged,
        "history changed; reload the first page",
    )
}
fn read_error(source: storage_engine::Error) -> ApiError {
    if source.code() == storage_contract::ErrorCode::RequestTimeout {
        ApiError::new(
            ::http::StatusCode::REQUEST_TIMEOUT,
            source.code(),
            source.message(),
        )
        .with_source(source)
    } else {
        error(source)
    }
}
fn checkpoint(control: &GraphReadControl) -> Result<(), ApiError> {
    control
        .checkpoint()
        .map_err(storage_engine::Error::from)
        .map_err(read_error)
}

pub(super) fn read(
    backend: &EngineBackend,
    identity: &Identity,
    request: ReadHistoryGraph,
    control: &GraphReadControl,
) -> Result<HistoryGraphResponse, ApiError> {
    let (history, observed, selection) = request.into_parts();
    checkpoint(control)?;
    let plan = {
        let engine = backend.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        repo_for_history(&engine, &history)?;
        checkpoint(control)?;
        let plan = engine
            .history_graph_read_plan(&history)
            .map_err(read_error)?;
        if let Some(observed) = &observed
            && observed.as_str() != plan.root().as_str()
        {
            return Err(changed());
        }
        plan
    };
    let root = plan.root().as_str().to_owned();
    let page = project(plan, selection, &history, &root, control);
    // Projection consumes the old State owner before waiting for re-admission.
    checkpoint(control)?;
    let engine = backend.engine.lock().expect("service state lock poisoned");
    current_protected_identity(&engine, identity)?;
    repo_for_history(&engine, &history)?;
    checkpoint(control)?;
    if engine.history_root(&history).map_err(read_error)?.as_str() != root {
        return Err(changed());
    }
    checkpoint(control)?;
    page
}

fn project(
    plan: storage_engine::history_graph_pages::HistoryGraphReadPlan,
    request: GraphRead,
    history: &str,
    root: &str,
    control: &GraphReadControl,
) -> Result<HistoryGraphResponse, ApiError> {
    let observer = || control.checkpoint().map_err(storage_engine::Error::from);
    Ok(match request {
        GraphRead::Page {
            collection,
            after,
            limit,
        } => {
            let selected = plan
                .read_page(collection, after.as_ref(), limit, observer)
                .map_err(read_error)?;
            let next_cursor = selected
                .next
                .map(|key| crate::backend_graph::cursor(history, root, collection, limit, key))
                .transpose()?;
            HistoryGraphResponse::Graph(GraphPage {
                history: selected.history,
                history_root: root.to_owned(),
                items: selected.items,
                next_cursor,
            })
        }
        GraphRead::Text {
            target,
            offset,
            limit,
        } => {
            let selected = plan
                .read_text(&target, offset, limit, observer)
                .map_err(read_error)?;
            HistoryGraphResponse::Text(TextPage {
                history_tree_id: history.to_owned(),
                history_root: root.to_owned(),
                target,
                total_bytes: selected.total_bytes as u64,
                offset: selected.offset as u64,
                text: selected.text,
                next_offset: selected.next_offset.map(|offset| offset as u64),
            })
        }
        GraphRead::Pending {
            target,
            after,
            limit,
        } => {
            let selected = plan
                .read_pending(&target, after.as_deref(), limit, observer)
                .map_err(read_error)?;
            HistoryGraphResponse::Pending(PendingPage {
                history_tree_id: history.to_owned(),
                history_root: root.to_owned(),
                target,
                records: selected.items,
                next_after: selected.next,
            })
        }
        GraphRead::Deletions {
            commit_id,
            offset,
            limit,
        } => {
            let selected = plan
                .read_deletions(&commit_id, offset, limit, observer)
                .map_err(read_error)?;
            HistoryGraphResponse::Deletions(DeletionsPage {
                history_tree_id: history.to_owned(),
                history_root: root.to_owned(),
                commit_id,
                timestamp: selected.timestamp,
                total: selected.total as u64,
                offset: selected.offset as u64,
                records: selected.items,
                next_offset: selected.next_offset.map(|offset| offset as u64),
            })
        }
    })
}
