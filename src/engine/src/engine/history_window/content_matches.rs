//! Content equality is a metadata observation. No links or identities are changed.
use super::{HistoryCancellationCheckpoint, query};
use crate::{Error, Result};
use storage_contract::content_matches::{ContentMatch, ContentMatchPage, ContentMatchRequest};
use storage_contract::history_window::{HistoryFileId, HistoryTreeId, HistoryVersionId};

pub(super) fn project(
    query: &query::Query<'_>,
    request: &ContentMatchRequest,
) -> Result<ContentMatchPage> {
    let source = query.node(request.node_id.as_str())?.version(query.state);
    let matching = |version: &crate::FileVersion| {
        source.logical_size != 0
            && !crate::node_retirement::is_retired(query.state, &version.version_id)
            && version.history_tree_id == source.history_tree_id
            && version.file_id != source.file_id
            && version.logical_size == source.logical_size
            && version.content_hash == source.content_hash
    };
    if let Some(after) = &request.after_version_id
        && !query
            .state
            .versions
            .get(after.as_str())
            .is_some_and(matching)
    {
        return Err(Error::new(
            storage_contract::ErrorCode::InvalidRequest,
            "content match continuation is not a matching version in this history",
        ));
    }
    let mut matches = Vec::new();
    let mut more = false;
    if source.logical_size != 0 {
        for version in query.state.versions.values() {
            query.cancel.checkpoint()?;
            if request
                .after_version_id
                .as_ref()
                .is_some_and(|after| version.version_id.as_str() <= after.as_str())
                || !matching(version)
            {
                continue;
            }
            if matches.len() == request.limit() {
                more = true;
                break;
            }
            matches.push(ContentMatch {
                creation_name: version.creation_name.clone(),
                file_id: HistoryFileId::parse(&version.file_id)?,
                version_id: HistoryVersionId::parse(&version.version_id)?,
                content_state: query.state.content_states.get(&version.version_id).copied(),
            });
        }
    }
    query.cancel.checkpoint()?;
    let next_after_version_id = if more {
        Some(
            matches
                .last()
                .expect("invariant: an extra match follows a nonempty full page")
                .version_id
                .clone(),
        )
    } else {
        None
    };
    Ok(ContentMatchPage {
        file_id: request.file_id.clone(),
        source_node_id: request.node_id.clone(),
        source_version_id: HistoryVersionId::parse(&source.version_id)?,
        history_tree_id: HistoryTreeId::parse(&source.history_tree_id)?,
        history_root: request.history_root,
        logical_size: source.logical_size,
        content_hash: source.content_hash,
        source_content_state: query.state.content_states.get(&source.version_id).copied(),
        matches,
        next_after_version_id,
    })
}
