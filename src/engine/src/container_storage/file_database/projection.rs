//! File-owned retained graph and historical D/undo, with no exterior locations.
//! Namespace commits, routing/control data and authority receipts remain in
//! their original scopes; local lifecycle rows are pathless audit projections.
use super::{rows::*, *};
use crate::{ContentState, business::file_nodes};

pub(super) const MAX_RECORD_BYTES: usize = 12 * 1024 * 1024;
struct Records {
    rows: Vec<Record>,
    bytes: usize,
}
impl Records {
    fn add<T: Serialize>(&mut self, kind: &'static str, id: &str, value: &T) -> Result<()> {
        if self.rows.len() == MAX_ROWS {
            return Err(budget());
        }
        let encoded = crate::disk::encode_metadata(value, MAX_RECORD_BYTES - self.bytes)?;
        self.bytes += encoded.len() + id.len();
        if self.bytes > MAX_RECORD_BYTES {
            return Err(budget());
        }
        self.rows.push(Record {
            kind,
            id: id.to_owned(),
            body: String::from_utf8(encoded).expect("invariant: metadata encoder emits UTF-8"),
        });
        Ok(())
    }
}
pub(super) fn records(view: StateProjection<'_>, file: &str) -> Result<Vec<Record>> {
    let events = crate::file_deletion::retained::events(view, file)?;
    with_events(view, file, &events)
}

/// Unpublished copied facts supplied by the exact joint move projection. No
/// operation receipt is fabricated just to make its derived SQLite rows exist.
pub(super) fn move_records(
    view: StateProjection<'_>,
    file: &str,
    events: &[LifecycleRow],
) -> Result<Vec<Record>> {
    if view.state().file_deletions.contains_key(file)
        || view.owners().file_rows(file).is_some_and(|rows| {
            !rows.history_copies.is_empty() || !rows.deletion_operations.is_empty()
        })
    {
        return Err(invalid(
            "move target database requires a fresh undeleted identity",
        ));
    }
    with_events(view, file, events)
}

/// The same retained source graph with its proposed all-frontier D and one
/// appended native move audit row. This is staging, not a published receipt.
pub(super) fn move_source_records(
    view: StateProjection<'_>,
    node: &crate::FileDeletionNode,
    events: &[LifecycleRow],
) -> Result<Vec<Record>> {
    if view.state().file_deletions.get(&node.file_id) != Some(node) {
        return Err(invalid(
            "move source database requires its exact proposed deletion",
        ));
    }
    with_events(view, &node.file_id, events)
}

fn with_events(
    view: StateProjection<'_>,
    file: &str,
    events: &[LifecycleRow],
) -> Result<Vec<Record>> {
    if view.owners().malformed_deletion_receipts() {
        return Err(invalid(
            "retained deletion receipt has no complete file ownership",
        ));
    }
    let state = view.state();
    let memberships = view.owners().file_rows(file);
    let mut rows = Records {
        rows: Vec::new(),
        bytes: 0,
    };
    let mut nodes = file_nodes::NodeProjection::from_merge_events(
        state,
        memberships
            .into_iter()
            .flat_map(|rows| rows.merges.keys())
            .map(|id| &state.file_branch_merges[id]),
    );
    rows.add(
        "file",
        file,
        &FileRow {
            file_id: file.to_owned(),
            note: state.file_notes.get(file).cloned(),
            selected_branch_id: state.file_selections.get(file).cloned(),
        },
    )?;
    for id in view.owners().versions(file) {
        rows.add(
            "version",
            id,
            &VersionRow {
                version: state.versions[id].clone(),
                parent_node_ids: nodes.parents(id),
                tags: nodes.tags(id).clone(),
                note: state.version_notes.get(id).cloned(),
                content_state: state
                    .content_states
                    .get(id)
                    .copied()
                    .unwrap_or(ContentState::Available),
            },
        )?;
    }
    if let Some(memberships) = memberships {
        for id in memberships.branches.keys() {
            let branch = &state.branches[id];
            rows.add(
                "branch",
                id,
                &BranchRow {
                    branch_id: id.clone(),
                    history_tree_id: branch.history_tree_id.clone(),
                    file_id: file.to_owned(),
                    name: branch.name.clone(),
                    state: branch.state,
                    generation: branch.generation,
                    head_node_id: file_nodes::branch_head(state, id)?,
                    pending_version_id: branch.pending_file_versions.get(file).cloned(),
                    alias_of: state.file_branch_aliases.get(id).cloned(),
                },
            )?;
        }
        for id in memberships.path_nodes.keys() {
            let node = &state.file_path_nodes[id];
            rows.add(
                "anchor",
                id,
                &AnchorRow {
                    node_id: id.clone(),
                    history_tree_id: node.history_tree_id.clone(),
                    file_id: file.to_owned(),
                    version_id: node.version_id.clone(),
                    branch_id: node.branch_id.clone(),
                    parent_node_ids: nodes.parents(id),
                    tags: nodes.tags(id).clone(),
                    note: state.file_path_notes.get(id).cloned(),
                    operation_id: node.operation_id,
                    author: node.author.clone(),
                    sequence: node.sequence,
                    created_at: node.created_at,
                },
            )?;
        }
        for id in memberships.metadata_nodes.keys() {
            rows.add(
                "metadata",
                id,
                &MetadataRow {
                    node: state.file_metadata_nodes[id].clone(),
                    parent_node_ids: nodes.parents(id),
                    note: state.file_path_notes.get(id).cloned(),
                },
            )?;
        }
        for id in memberships.merges.keys() {
            rows.add("merge", id, &state.file_branch_merges[id])?;
        }
        for id in memberships.merge_links.keys() {
            rows.add(
                "merge_link",
                id,
                &LinkRow {
                    file_id: file.to_owned(),
                    link: state.merge_links[id].clone(),
                },
            )?;
        }
    }
    for event in events {
        rows.add("lifecycle", &event.operation_id.to_string(), event)?;
    }
    if let Some(node) = state.file_deletions.get(file) {
        rows.add("deletion", &node.node_id, &DeletionRow::from(node))?;
    }
    rows.rows
        .sort_by(|a, b| (a.kind, &a.id).cmp(&(b.kind, &b.id)));
    Ok(rows.rows)
}
