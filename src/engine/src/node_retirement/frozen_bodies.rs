//! Content roots are concrete directory tree IDs, not child branch HEADs,
//! historical parent links, display paths or every older tree in the pool.
use super::{State, node_version};
use crate::{ErrorCode, NamespaceEntryKind, Result};
use std::collections::BTreeSet;

#[derive(Debug, error_definition::Error)]
pub enum RetainedBodyError {
    #[error("version is needed by a live inherited file, selected node or frozen directory composition")]
    Selected,
    #[error("a retained directory composition does not contain its referenced immutable tree")]
    MissingTree,
}
impl RetainedBodyError {
    pub(crate) fn code(&self) -> ErrorCode {
        match self {
            Self::Selected => ErrorCode::ContentPinned,
            Self::MissingTree => ErrorCode::PublicationInvalid,
        }
    }
}

pub(crate) fn any_frozen_body<'a>(
    state: &'a State,
    roots: impl IntoIterator<Item = &'a str>,
    selected: impl Fn(&str) -> bool,
) -> Result<bool> {
    let mut trees: BTreeSet<_> = roots.into_iter().collect();
    let mut visited = BTreeSet::new();
    while let Some(id) = trees.pop_first() {
        if !visited.insert(id) {
            continue;
        }
        let tree = state.directory_trees.get(id).ok_or(RetainedBodyError::MissingTree)?;
        if tree.entries.iter().any(|entry| {
            entry.kind == NamespaceEntryKind::File && selected(&entry.version_id)
        }) || tree.file_contexts.values().any(|context| {
            context.node_id.as_deref().and_then(|node| node_version(state, node))
                .is_some_and(&selected)
        }) {
            return Ok(true);
        }
        trees.extend(tree.directory_refs.values().map(|reference| reference.tree_id.as_str()));
    }
    Ok(false)
}
