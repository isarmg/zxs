//! Explicit file-branch adoption with immutable bodies and durable provenance.
use super::*;
use json_codec::json;
use std::collections::BTreeSet;

fn reaches(state: &State, start: &str, needle: &str) -> bool {
    let mut queue = vec![start.to_owned()];
    let mut seen = BTreeSet::new();
    while let Some(node) = queue.pop() {
        if node == needle {
            return true;
        }
        if seen.insert(node.clone()) {
            queue.extend(super::file_nodes::node_parents(state, &node));
        }
    }
    false
}

impl Engine {
    pub fn merge_file_branches(
        &mut self,
        request: FileBranchMergeRequest<'_>,
    ) -> Result<OperationResult> {
        let body = request.body();
        let FileBranchMergeRequest {
            file_id: file,
            inputs,
            chosen_node_id: chosen_node,
            surviving_branch_id: survivor,
            location,
            principal,
            operation_id: operation,
        } = request;
        self.mutate("file.branches.merge", principal, operation,
            body,
            |engine,state,_| {
                super::file_requests::validate_location(engine, file, location)?;
                if inputs.len() < 2 || inputs.len() > engine.config.max_entries {
                    return Err(Error::new(storage_contract::ErrorCode::InvalidRequest, "merge requires at least two bounded file branches"));
                }
                let version = super::file_nodes::node_version(state,file,chosen_node)?;
                let body = state.versions[&version].clone();
                engine.repository_for_history(&body.history_tree_id)?;

                engine.valid_replica(&version)?;
                let mut branches = BTreeSet::new();
                let mut heads = Vec::new();
                for input in inputs {
                    if !branches.insert(input.branch_id.clone()) {
                        return Err(Error::new(storage_contract::ErrorCode::InvalidRequest,"merge branch inputs must be distinct"));
                    }
                    let branch = state.branches.get(&input.branch_id)
                        .filter(|branch| branch.file_id.as_deref()==Some(file) && branch.history_tree_id==body.history_tree_id)
                        .ok_or_else(|| Error::new(storage_contract::ErrorCode::FileBranchNotFound,"merge branch does not belong to this file"))?;
                    if branch.state != BranchState::Active || state.file_branch_aliases.contains_key(&input.branch_id) {
                        return Err(Error::new(storage_contract::ErrorCode::FileBranchConflict,"merged branches cannot be merged or written again"));
                    }
                    let head = super::file_nodes::branch_head(state,&input.branch_id)?;
                    if head != input.expected_node_id {
                        return Err(Error::new(storage_contract::ErrorCode::FileBranchConflict,"merge branch head changed; review all inputs again"));
                    }
                    heads.push(head);
                }
                if !branches.contains(survivor) {
                    return Err(Error::new(storage_contract::ErrorCode::InvalidRequest,"the surviving branch must be an explicit input"));
                }
                let reused = heads.iter().any(|head| head==chosen_node);
                let mut parents = heads.iter().cloned().collect::<BTreeSet<_>>();
                parents.insert(chosen_node.into());
                let result = if reused { chosen_node.to_owned() } else { new_id("fmeta") };
                let edges = if reused {
                    parents.into_iter().filter(|parent| parent!=&result && !reaches(state,parent,&result)).collect::<Vec<_>>()
                } else { parents.into_iter().collect::<Vec<_>>() };
                if !reused {
                    state.file_metadata_nodes.insert(result.clone(), FileMetadataNode {
                        node_id:result.clone(),history_tree_id:body.history_tree_id.clone(),file_id:file.into(),
                        version_id:version.clone(),parent_node_ids:edges.clone(),branch_id:survivor.into(),
                        kind:FileMetadataKind::Merge,tags:super::file_nodes::node_tags(state,chosen_node),
                        operation_id:operation,author:principal.into(),sequence:state.sequence,created_at:now(),
                    });
                }
                let merge_id = new_id("fmerge");
                state.file_branch_merges.insert(merge_id.clone(), FileBranchMerge {
                    merge_id:merge_id.clone(),history_tree_id:body.history_tree_id.clone(),file_id:file.into(),
                    input_branch_ids:inputs.iter().map(|input|input.branch_id.clone()).collect(),input_node_ids:heads,
                    chosen_node_id:chosen_node.into(),result_node_id:result.clone(),surviving_branch_id:survivor.into(),
                    reused_node:reused,parent_node_ids:edges,operation_id:operation,sequence:state.sequence,
                });
                super::file_publication::advance_branch_node(state,survivor,&result,true,principal,operation)?;
                let affected = super::file_publication::publish_node(state, crate::business::file_publication::NodePublication { file, matching_branches: &branches, matching_node: None, target_branch: survivor, target_node: &result, principal, operation, maximum: engine.config.max_entries })?;
                for branch in branches.iter().filter(|branch| branch.as_str()!=survivor) {
                    state.file_branch_aliases.insert(branch.clone(),survivor.into());
                    let branch = state.branches.get_mut(branch).expect("checked merge branch");
                    branch.state=BranchState::Merged;
                    branch.generation = branch.generation.checked_add(1).ok_or_else(|| Error::new(storage_contract::ErrorCode::ResourceLimit, "branch generation exhausted"))?;
                }
                // Historical aliases remain frozen, while their writable
                // ownership target follows a later explicit owner merge.
                for target in state.file_branch_aliases.values_mut() {
                    if branches.contains(target) { *target = survivor.into(); }
                }
                Ok(json!({"file_id":file,"merge_id":merge_id,"history_tree_id":body.history_tree_id,
                    "branch_id":survivor,"surviving_branch_id":survivor,"node_id":result,"version_id":version,
                    "chosen_node_id":chosen_node,"reused_node":reused,"directory_branch_ids":affected,
                    "merged_branch_ids":branches.into_iter().filter(|branch|branch!=survivor).collect::<Vec<_>>()}))
            })
    }
}
