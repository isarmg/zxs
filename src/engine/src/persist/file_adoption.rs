//! The original native merge decision is independent of immutable FILE bodies.
//! Imported/copy-remapped graphs do not acquire a local native operation.
use crate::{BranchState, Engine, Error, ErrorCode, FileBranchMerge, OperationResult, Result, State};
use data_serialization::Deserialize;
use std::collections::BTreeSet;
use storage_contract::{BranchName, FileHistoryMergeInput, FileLocation, FileMergeInput};

fn invalid() -> Error {
    Error::new(ErrorCode::PublicationInvalid, "FILE adoption does not match its original selection")
}
pub(crate) fn is_kind(kind: &str) -> bool { matches!(kind, "file.branches.merge" | "file.history.merge") }

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Location { repository_id: String, path: String }
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct BranchSelection {
    file_id: String, inputs: Vec<FileMergeInput>, chosen_node_id: String,
    surviving_branch_id: String,
    #[serialization(deserialize_with = "nullable")]
    location: Option<Location>,
}
fn nullable<'de,D,T>(d:D) -> std::result::Result<Option<T>,D::Error>
where D: data_serialization::Deserializer<'de>, T: Deserialize<'de> { Option::<T>::deserialize(d) }
fn location(value: Option<Location>) -> Result<Option<FileLocation>> {
    value.map(|v| {
        let _: storage_contract::RepositoryId = v.repository_id.parse()?;
        name_policy::validate_path(&v.path)?;
        Ok(FileLocation { repository_id: v.repository_id, path: v.path })
    }).transpose()
}

pub(crate) enum Sources {
    Branches(Vec<FileMergeInput>),
    Historical { inputs: Vec<FileHistoryMergeInput>, name: BranchName, location: Option<FileLocation> },
}
impl Sources {
    pub(crate) fn len(&self) -> usize {
        match self { Self::Branches(v) => v.len(), Self::Historical {inputs,..} => inputs.len() }
    }
    pub(crate) fn at(&self, index: usize) -> (&str,&str) {
        match self {
            Self::Branches(v) => (&v[index].branch_id,&v[index].expected_node_id),
            Self::Historical {inputs,..} => (&inputs[index].branch_id,&inputs[index].source_node_id),
        }
    }
}

fn real_body<'a>(state:&'a State, event:&FileBranchMerge, id:&str) -> Result<&'a crate::FileVersion> {
    let version = if let Some(path) = state.file_path_nodes.get(id) {
        if path.file_id != event.file_id || path.history_tree_id != event.history_tree_id {return Err(invalid())}
        path.version_id.as_str()
    } else { id };
    state.versions.get(version).filter(|v| v.file_id == event.file_id && v.history_tree_id == event.history_tree_id)
        .ok_or_else(invalid)
}

/// Strict original request, original actor, result and actual branch/node membership.
/// Later legitimate branch advances never replace the original head constraints.
pub(crate) fn original(state:&State, receipt:&OperationResult) -> Result<Sources> {
    let field = |key:&str| receipt.result.get(key).and_then(json_codec::Value::as_str).ok_or_else(invalid);
    let event = state.file_branch_merges.get(field("merge_id")?).ok_or_else(invalid)?;
    let author = state.operation_principals.get(&receipt.operation_id).ok_or_else(invalid)?;
    if event.operation_id != receipt.operation_id || event.sequence != receipt.sequence
        || event.sequence == 0 || event.principal.as_str() != author || !event.is_existing_adoption()
        || !matches!(receipt.commit_state, crate::CommitState::Published | crate::CommitState::PublishedDegraded)
        || receipt.result["reused_node"].as_bool() != Some(true)
        || field("merge_id")? != event.merge_id || field("history_tree_id")? != event.history_tree_id
        || field("file_id")? != event.file_id || field("chosen_node_id")? != event.chosen_node_id
        || field("node_id")? != event.result_node_id || field("surviving_branch_id")? != event.surviving_branch_id
        || field("branch_id")? != event.surviving_branch_id
        || receipt.history_tree_id.as_deref() != Some(event.history_tree_id.as_str())
        || receipt.branch_id.as_deref() != Some(event.surviving_branch_id.as_str())
        || field("version_id")? != real_body(state,event,&event.chosen_node_id)?.version_id
        || event.input_branch_ids.len() < 2 || event.input_node_ids.len() != event.input_branch_ids.len()
        || !event.input_branch_ids.contains(&event.surviving_branch_id)
        || event.input_branch_ids.iter().collect::<BTreeSet<_>>().len() != event.input_branch_ids.len() {
        return Err(invalid())
    }
    let retired:Vec<_> = event.input_branch_ids.iter().filter(|b| *b != &event.surviving_branch_id)
        .collect::<BTreeSet<_>>().into_iter().collect();
    if receipt.result["merged_branch_ids"] != json_codec::json!(retired) {return Err(invalid())}
    let sources = match receipt.kind.as_str() {
        "file.branches.merge" => {
            let body = receipt.result.get("merge_request").ok_or_else(invalid)?;
            if Engine::native_request_digest(&receipt.kind,author,body)? != receipt.request_digest {return Err(invalid())}
            let input:BranchSelection = json_codec::from_value(body.clone())?;
            let _ = location(input.location)?;
            if input.file_id != event.file_id || input.chosen_node_id != event.chosen_node_id
                || input.surviving_branch_id != event.surviving_branch_id
                || !input.inputs.iter().map(|v| &v.branch_id).eq(event.input_branch_ids.iter()) {return Err(invalid())}
            Sources::Branches(input.inputs)
        }
        "file.history.merge" => {
            let inputs:Vec<FileHistoryMergeInput> = json_codec::from_value(receipt.result.get("source_inputs").ok_or_else(invalid)?.clone())?;
            let name=BranchName::parse(field("name")?)?;
            let location = location(json_codec::from_value(receipt.result.get("location").ok_or_else(invalid)?.clone())?)?;
            let request = crate::FileHistoryMergeRequest { file_id:&event.file_id, inputs:&inputs,
                chosen_node_id:&event.chosen_node_id, name:&name, location:location.as_ref(),
                principal:author, operation_id:event.operation_id };
            if request.native_digest()? != receipt.request_digest || event.input_branch_ids.first() != Some(&event.surviving_branch_id) {return Err(invalid())}
            Sources::Historical {inputs,name,location}
        }
        _ => return Err(invalid()),
    };
    if sources.len() != event.input_node_ids.len() {return Err(invalid())}
    for (n,head) in event.input_node_ids.iter().enumerate() {
        let (branch,node) = sources.at(n);
        if head != node {return Err(invalid())}
        for id in [branch, &event.input_branch_ids[n]] {
            let b=state.branches.get(id).ok_or_else(invalid)?;
            if b.file_id.as_deref()!=Some(event.file_id.as_str()) || b.history_tree_id!=event.history_tree_id {return Err(invalid())}
        }
        real_body(state,event,head)?;
    }
    Ok(sources)
}

pub(crate) fn transition(previous:&State,next:&State,receipt:&OperationResult,has_material:bool) -> Result<()> {
    if !is_kind(&receipt.kind) {return Ok(())}
    let sources=original(next,receipt)?;
    let event=&next.file_branch_merges[receipt.result["merge_id"].as_str().ok_or_else(invalid)?];
    if has_material || receipt.sequence != next.sequence || previous.operations.contains_key(&event.operation_id)
        || previous.file_branch_merges.contains_key(&event.merge_id)
        || previous.file_branch_merges.len().checked_add(1) != Some(next.file_branch_merges.len())
        || previous.file_branch_merges.iter().any(|(k,v)| next.file_branch_merges.get(k) != Some(v))
        || previous.versions != next.versions || previous.placements != next.placements
        || previous.file_node_parents != next.file_node_parents || previous.file_path_nodes != next.file_path_nodes
        || !next.file_metadata_nodes.is_empty() || !previous.file_metadata_nodes.is_empty() {return Err(invalid())}
    real_body(previous,event,&event.chosen_node_id)?;
    match &sources {
        Sources::Branches(inputs) => for v in inputs {
            let branch=previous.branches.get(&v.branch_id).ok_or_else(invalid)?;
            if branch.state != BranchState::Active || previous.file_branch_aliases.contains_key(&v.branch_id)
                || crate::business::file_nodes::branch_head(previous,&v.branch_id)? != v.expected_node_id {
                return Err(invalid())
            }
        },
        Sources::Historical{inputs,name,location} => {
            let request=crate::FileHistoryMergeRequest { file_id:&event.file_id, inputs,
                chosen_node_id:&event.chosen_node_id, name, location:location.as_ref(),
                principal:event.principal.as_str(),operation_id:event.operation_id };
            request.prepared_sources(previous)?;
            for (n,source) in inputs.iter().enumerate() {
                if previous.branches.contains_key(&event.input_branch_ids[n])
                    || next.branches.get(&source.branch_id) != previous.branches.get(&source.branch_id) {return Err(invalid())}
            }
        }
    }
    for id in &event.input_branch_ids {
        let b=next.branches.get(id).ok_or_else(invalid)?;
        if id==&event.surviving_branch_id {
            if b.state!=BranchState::Active || next.file_branch_aliases.contains_key(id)
                || crate::business::file_nodes::branch_head(next,id)?!=event.result_node_id {return Err(invalid())}
        } else if b.state!=BranchState::Merged || next.file_branch_aliases.get(id)!=Some(&event.surviving_branch_id) {return Err(invalid())}
    }
    Ok(())
}
