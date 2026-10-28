use super::{DecodeError, FileBranch};
use data_serialization::{Deserialize, Deserializer};
use json_codec::Value;
use std::collections::BTreeMap;
use storage_contract::{BranchKind, BranchState};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Branch {
    branch_id: String,
    history_tree_id: String,
    name: String,
    head_commit_id: String,
    fork_commit_id: String,
    branch_kind: BranchKind,
    file_id: String,
    // The server omits this field when no explicit file node is recorded.
    #[serialization(default, deserialize_with = "present_identity")]
    file_node_id: Option<String>,
    pending_file_versions: BTreeMap<String, String>,
    #[serialization(rename = "generation", with = "storage_contract::u64_string")]
    _generation: u64,
    state: BranchState,
    materialized: bool,
    inherited: bool,
    content_state: String,
    #[serialization(deserialize_with = "required_nullable")]
    version_id: Option<String>,
    #[serialization(deserialize_with = "required_nullable")]
    own_version_id: Option<String>,
    #[serialization(deserialize_with = "required_nullable")]
    source_version_id: Option<String>,
    visible_content_state: String,
    read_only: bool,
    #[serialization(deserialize_with = "required_nullable")]
    alias_of: Option<String>,
    #[serialization(deserialize_with = "required_nullable")]
    node_id: Option<String>,
    current: bool,
}

fn required_nullable<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}
fn present_identity<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}
fn identity(value: &str) -> Result<(), DecodeError> {
    if value.is_empty() || value.len() > 256 {
        Err(DecodeError::Relation(
            "identities must be nonempty and at most 256 bytes",
        ))
    } else {
        Ok(())
    }
}
pub(super) fn selected(value: Value) -> Result<Option<String>, DecodeError> {
    let selected: Option<String> = json_codec::from_value(value).map_err(DecodeError::Wire)?;
    if let Some(id) = &selected {
        identity(id)?;
    }
    Ok(selected)
}

pub(super) fn branch(
    value: Value,
    file: &str,
    selected: Option<&str>,
) -> Result<FileBranch, DecodeError> {
    let Branch {
        branch_id,
        history_tree_id,
        name,
        head_commit_id,
        fork_commit_id,
        branch_kind,
        file_id,
        file_node_id,
        pending_file_versions,
        _generation: _,
        state,
        materialized,
        inherited,
        content_state,
        version_id,
        own_version_id,
        source_version_id,
        visible_content_state,
        read_only,
        alias_of,
        node_id,
        current,
    } = json_codec::from_value(value).map_err(DecodeError::Wire)?;
    for id in [
        &branch_id,
        &history_tree_id,
        &head_commit_id,
        &fork_commit_id,
        &file_id,
    ]
    .into_iter()
    .chain(file_node_id.iter())
    .chain(version_id.iter())
    .chain(own_version_id.iter())
    .chain(alias_of.iter())
    .chain(node_id.iter().filter(|id| !id.is_empty()))
    {
        identity(id)?;
    }
    for (file, source) in &pending_file_versions {
        identity(file)?;
        if !source.is_empty() {
            identity(source)?;
        }
    }
    if let Some(source) = source_version_id
        .as_ref()
        .filter(|source| !source.is_empty())
    {
        identity(source)?;
    }
    storage_contract::validate_branch_name(&name).map_err(DecodeError::Name)?;
    if branch_kind != BranchKind::File || file_id != file {
        return Err(DecodeError::Relation(
            "branch must belong to the requested file",
        ));
    }
    if current != (selected == Some(branch_id.as_str())) {
        return Err(DecodeError::Relation(
            "current differs from selected_branch_id",
        ));
    }
    if read_only != (state != BranchState::Active || alias_of.is_some()) {
        return Err(DecodeError::Relation(
            "read_only differs from branch state or alias",
        ));
    }
    let source = source_version_id.as_deref();
    if inherited != source.is_some()
        || materialized != (!inherited && version_id.is_some())
        || own_version_id.as_deref()
            != if inherited {
                None
            } else {
                version_id.as_deref()
            }
        || inherited && version_id.as_deref() != source.filter(|id| !id.is_empty())
        || !matches!(
            visible_content_state.as_str(),
            "LIVE" | "AVAILABLE" | "PURGED" | "MIGRATED"
        )
        || content_state
            != if inherited {
                "PENDING"
            } else {
                visible_content_state.as_str()
            }
        || source != pending_file_versions.get(file).map(String::as_str)
        || file_node_id
            .as_ref()
            .is_some_and(|id| node_id.as_ref() != Some(id))
    {
        return Err(DecodeError::Relation(
            "branch content projection is inconsistent",
        ));
    }
    Ok(FileBranch {
        id: branch_id,
        name,
        // An empty visible entry is an explicit current wire representation;
        // it is not a usable node identity inside the controls.
        head: node_id.filter(|id| !id.is_empty()).or(version_id),
        pending: !materialized,
        writable: !read_only,
    })
}
