//! Version labels are immutable metadata children, never mutable file-global state.
use super::*;
use json_codec::json;
use std::collections::BTreeSet;

pub(crate) struct TagEdit {
    added: BTreeSet<String>,
    removed: BTreeSet<String>,
}

pub(crate) struct TagUpdate {
    pub(crate) tags: BTreeSet<String>,
    pub(crate) changed: bool,
}

pub(crate) struct TagSelection {
    pub(crate) version: String,
    pub(crate) advance: bool,
}

pub(crate) fn target_branch<'state, 'target>(
    state: &'state State,
    file: &str,
    target: &'target FileTagTarget,
    seen: &mut BTreeSet<(&'target str, &'target str)>,
) -> Result<&'state Branch> {
    if !seen.insert((&target.branch_id, &target.source_node_id)) {
        return Err(Error::new(
            storage_contract::ErrorCode::InvalidRequest,
            "duplicate tag target",
        ));
    }
    state
        .branches
        .get(&target.branch_id)
        .filter(|branch| branch.file_id.as_deref() == Some(file))
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::FileBranchNotFound,
                "tag target branch missing",
            )
        })
}

/// Share selection order and branch rules while retaining each publication
/// path's existing head and source-version admission contract.
pub(crate) fn resolve_target(
    state: &State,
    target: &FileTagTarget,
    branch: &Branch,
    head: impl FnOnce() -> Result<String>,
    version: impl FnOnce() -> Result<String>,
) -> Result<TagSelection> {
    if branch.state != BranchState::Active
        || state.file_branch_aliases.contains_key(&target.branch_id)
    {
        return Err(Error::new(
            storage_contract::ErrorCode::BranchNotActive,
            "merged aliases are read-only",
        ));
    }
    let head = head()?;
    if head != target.expected_node_id {
        return Err(Error::new(
            storage_contract::ErrorCode::FileBranchConflict,
            "tag target head changed",
        ));
    }
    let version = version()?;
    if !file_nodes::belongs_to_branch(state, &target.branch_id, &head, &target.source_node_id) {
        return Err(Error::new(
            storage_contract::ErrorCode::FileNodeNotFound,
            "tag target node is outside the selected branch",
        ));
    }
    Ok(TagSelection {
        version,
        advance: head == target.source_node_id,
    })
}

impl TagEdit {
    pub(crate) fn new(
        targets: &[FileTagTarget],
        add: &[String],
        remove: &[String],
        maximum: usize,
    ) -> Result<Self> {
        if targets.is_empty()
            || targets.len() > maximum
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
        Ok(Self { added, removed })
    }

    pub(crate) fn apply(&self, state: &State, source: &str) -> Result<TagUpdate> {
        let old = file_nodes::node_tags(state, source);
        let mut tags = old.clone();
        tags.retain(|tag| !self.removed.contains(tag));
        tags.extend(self.added.iter().cloned());
        if tags.len() > MAX_FILE_TAGS {
            return Err(Error::new(
                storage_contract::ErrorCode::TagLimitExceeded,
                "a file node supports at most 16 tags",
            ));
        }
        Ok(TagUpdate {
            changed: tags != old,
            tags,
        })
    }
}

struct TagChange {
    target: FileTagTarget,
    history: String,
    version: String,
    tags: BTreeSet<String>,
    changed: bool,
    advance: bool,
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
        let edit = TagEdit::new(targets, add, remove, self.config.max_entries)?;
        let body = request.body();
        self.mutate("file.tags.update", principal, operation,
            body,
            |engine,state,_| {
                if let Some(view) = engine.admitted_current_namespaces()? {
                    return view.update_current_file_tags(
                        engine.state(),
                        state,
                        &request,
                        &edit,
                        engine.config.max_entries,
                    );
                }
                super::file_requests::validate_location(engine, file, location)?;
                // Complete preflight before any branch, context or tree advances.
                let mut seen = BTreeSet::new();
                let mut changes = Vec::new();
                for target in targets {
                    let branch = target_branch(state, file, target, &mut seen)?;
                    engine.repository_for_history(&branch.history_tree_id)?;
                    let TagSelection { version, advance } = resolve_target(
                        state,
                        target,
                        branch,
                        || file_nodes::branch_head(state, &target.branch_id),
                        || file_nodes::node_version(state, file, &target.source_node_id),
                    )?;
                    let TagUpdate { tags, changed } = edit.apply(state, &target.source_node_id)?;
                    changes.push(TagChange {
                        target: target.clone(),
                        history: branch.history_tree_id.clone(),
                        version,
                        changed,
                        tags,
                        advance,
                    });
                }
                let mut results = Vec::new();
                for TagChange { target, history, version, tags, changed, advance } in changes {
                    if !changed {
                        results.push(json!({"branch_id":target.branch_id,"source_node_id":target.source_node_id,"node_id":target.source_node_id,"tags":tags,"no_change":true,"advanced":false}));
                        continue;
                    }
                    let node = new_id("fmeta")?;
                    state.file_metadata_nodes.insert(node.clone(),FileMetadataNode {
                        node_id:node.clone(),history_tree_id:history,file_id:file.into(),version_id:version,
                        parent_node_ids:vec![target.source_node_id.clone()],branch_id:target.branch_id.clone(),
                        kind:FileMetadataKind::Tags,tags:tags.clone(),operation_id:operation,author:principal.into(),sequence:state.sequence,created_at:now()?,
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
