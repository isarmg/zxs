//! Version labels are immutable metadata children, never mutable file-global state.
use super::*;
use json_codec::json;
use std::collections::BTreeSet;

struct TagChange {
    target: FileTagTarget,
    history: String,
    version: String,
    tags: BTreeSet<String>,
    changed: bool,
    advance: bool,
}

fn belongs_to_branch(state: &State, branch: &str, head: &str, source: &str) -> bool {
    if state
        .file_metadata_nodes
        .get(source)
        .is_some_and(|n| n.branch_id == branch)
        || state
            .file_path_nodes
            .get(source)
            .is_some_and(|n| n.branch_id == branch)
    {
        return true;
    }
    let mut todo = vec![head.to_owned()];
    let mut visited = BTreeSet::new();
    while let Some(node) = todo.pop() {
        if node == source {
            return true;
        }
        if visited.insert(node.clone()) {
            todo.extend(file_nodes::node_parents(state, &node));
        }
    }
    false
}

impl Engine {
    pub fn update_file_node_tags(
        &mut self,
        request: FileNodeTagsUpdateRequest<'_>,
    ) -> Result<OperationResult> {
        let FileNodeTagsUpdateRequest {
            file_id: file,
            targets,
            add,
            remove,
            location,
            principal,
            operation_id: operation,
        } = request;
        if targets.is_empty()
            || targets.len() > self.config.max_entries
            || add.len() > MAX_FILE_TAGS
            || remove.len() > MAX_FILE_TAGS
        {
            return Err(Error::new(
                storage_contract::ErrorCode::InvalidRequest,
                "a bounded nonempty tag target list is required",
            ));
        }
        for tag in add.iter().chain(remove) {
            validate_tag(tag)?;
        }
        let added: BTreeSet<_> = add.iter().cloned().collect();
        let removed: BTreeSet<_> = remove.iter().cloned().collect();
        if !added.is_disjoint(&removed) {
            return Err(Error::new(
                storage_contract::ErrorCode::InvalidTag,
                "a tag cannot be added and removed in one operation",
            ));
        }
        let body = request.body();
        self.mutate("file.tags.update", principal, operation,
            body,
            |engine,state,_| {
                super::file_requests::validate_location(engine, file, location)?;
                // Complete preflight before any branch, context or tree advances.
                let mut seen = BTreeSet::new();
                let mut changes = Vec::new();
                for target in targets {
                    if !seen.insert((target.branch_id.clone(),target.source_node_id.clone())) {
                        return Err(Error::new(storage_contract::ErrorCode::InvalidRequest, "duplicate tag target"));
                    }
                    let branch = state.branches.get(&target.branch_id)
                        .filter(|b| b.file_id.as_deref() == Some(file))
                        .ok_or_else(|| Error::new(storage_contract::ErrorCode::FileBranchNotFound, "tag target branch missing"))?;
                    engine.repository_for_history(&branch.history_tree_id)?;
                    if branch.state != BranchState::Active || state.file_branch_aliases.contains_key(&target.branch_id) {
                        return Err(Error::new(storage_contract::ErrorCode::BranchNotActive, "merged aliases are read-only"));
                    }
                    let head = file_nodes::branch_head(state,&target.branch_id)?;
                    if head != target.expected_node_id {
                        return Err(Error::new(storage_contract::ErrorCode::FileBranchConflict, "tag target head changed"));
                    }
                    let version = file_nodes::node_version(state,file,&target.source_node_id)?;
                    if !belongs_to_branch(state,&target.branch_id,&head,&target.source_node_id) {
                        return Err(Error::new(storage_contract::ErrorCode::FileNodeNotFound, "tag target node is outside the selected branch"));
                    }
                    let old = file_nodes::node_tags(state,&target.source_node_id);
                    let mut tags = old.clone();
                    tags.retain(|tag| !removed.contains(tag));
                    tags.extend(added.clone());
                    if tags.len() > MAX_FILE_TAGS { return Err(Error::new(storage_contract::ErrorCode::TagLimitExceeded, "a file node supports at most 16 tags")); }
                    changes.push(TagChange {
                        target: target.clone(),
                        history: branch.history_tree_id.clone(),
                        version,
                        changed: tags != old,
                        tags,
                        advance: head == target.source_node_id,
                    });
                }
                let mut results = Vec::new();
                for TagChange { target, history, version, tags, changed, advance } in changes {
                    if !changed {
                        results.push(json!({"branch_id":target.branch_id,"source_node_id":target.source_node_id,"node_id":target.source_node_id,"tags":tags,"no_change":true,"advanced":false}));
                        continue;
                    }
                    let node = new_id("fmeta");
                    state.file_metadata_nodes.insert(node.clone(),FileMetadataNode {
                        node_id:node.clone(),history_tree_id:history,file_id:file.into(),version_id:version,
                        parent_node_ids:vec![target.source_node_id.clone()],branch_id:target.branch_id.clone(),
                        kind:FileMetadataKind::Tags,tags:tags.clone(),operation_id:operation,author:principal.into(),sequence:state.sequence,created_at:now(),
                    });
                    if advance {
                        file_publication::advance_branch_node(state,&target.branch_id,&node,false,principal,operation)?;
                    }
                    let directories = file_publication::publish_node(state, crate::business::file_publication::NodePublication { file, matching_branches: &BTreeSet::from([target.branch_id.clone()]), matching_node: Some(&target.source_node_id), target_branch: &target.branch_id, target_node: &node, principal, operation, maximum: engine.config.max_entries })?;
                    results.push(json!({"branch_id":target.branch_id,"source_node_id":target.source_node_id,"node_id":node,"tags":tags,"no_change":false,"advanced":advance,"directory_branch_ids":directories}));
                }
                Ok(json!({"file_id":file,"nodes":results,"content_versions_created":"0"}))
            })
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/file_tags/tests.rs"]
mod tests;
