//! Request-local branch metadata; no snapshot grants future admission.
use data_serialization::Serialize;
use std::collections::BTreeMap;
use storage_contract::{BranchKind, BranchState, ContentState};

#[derive(Clone, Debug)]
pub struct BranchSnapshot {
    pub branch_id: String,
    pub history_tree_id: String,
    pub name: String,
    pub head_commit_id: String,
    pub fork_commit_id: String,
    pub branch_kind: BranchKind,
    pub file_id: Option<String>,
    pub file_node_id: Option<String>,
    pub pending_file_versions: BTreeMap<String, String>,
    pub generation: u64,
    pub state: BranchState,
    pub file: Option<FileBranchSnapshot>,
}
#[derive(Clone, Debug)]
pub struct FileBranchSnapshot {
    pub content: ContentDispositionSnapshot,
    pub read_only: bool,
    pub alias_of: Option<String>,
    pub node_id: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct BranchResponse {
    branch_id: String,
    history_tree_id: String,
    name: String,
    head_commit_id: String,
    fork_commit_id: String,
    branch_kind: BranchKind,
    #[serialization(skip_serializing_if = "Option::is_none")]
    file_id: Option<String>,
    #[serialization(skip_serializing_if = "Option::is_none")]
    file_node_id: Option<String>,
    pending_file_versions: BTreeMap<String, String>,
    #[serialization(with = "storage_contract::u64_string")]
    generation: u64,
    state: BranchState,
    materialized: bool,
    inherited: bool,
    content_state: &'static str,
    #[serialization(flatten)]
    file: Option<FileBranchResponse>,
}
#[derive(Serialize)]
struct FileBranchResponse {
    version_id: Option<String>,
    own_version_id: Option<String>,
    source_version_id: Option<String>,
    visible_content_state: &'static str,
    read_only: bool,
    alias_of: Option<String>,
    node_id: Option<String>,
}
impl From<BranchSnapshot> for BranchResponse {
    fn from(snapshot: BranchSnapshot) -> Self {
        let BranchSnapshot {
            branch_id,
            history_tree_id,
            name,
            head_commit_id,
            fork_commit_id,
            branch_kind,
            file_id,
            file_node_id,
            pending_file_versions,
            generation,
            state,
            file,
        } = snapshot;
        let mut materialized = pending_file_versions.is_empty();
        let mut inherited = !materialized;
        let mut content_state = if inherited { "PENDING" } else { "LIVE" };
        let file = file.map(|file| {
            let FileBranchSnapshot {
                content,
                read_only,
                alias_of,
                node_id,
            } = file;
            let ContentDispositionResponse {
                version_id,
                own_version_id,
                source_version_id,
                visible_content_state,
                materialized: file_materialized,
                inherited: file_inherited,
                content_state: file_state,
            } = content.into();
            materialized = file_materialized;
            inherited = file_inherited;
            content_state = file_state;
            FileBranchResponse {
                version_id,
                own_version_id,
                source_version_id,
                visible_content_state,
                read_only,
                alias_of,
                node_id,
            }
        });
        Self {
            branch_id,
            history_tree_id,
            name,
            head_commit_id,
            fork_commit_id,
            branch_kind,
            file_id,
            file_node_id,
            pending_file_versions,
            generation,
            state,
            materialized,
            inherited,
            content_state,
            file,
        }
    }
}
#[derive(Serialize)]
pub(crate) struct BranchListResponse {
    pub branches: Vec<BranchResponse>,
}

/// Logical disposition at this admission, distinct from live replica health.
#[derive(Clone, Debug)]
pub struct ContentDispositionSnapshot {
    pub version_id: Option<String>,
    pub source_version_id: Option<String>,
    pub stored_state: Option<ContentState>,
}
#[derive(Debug, Serialize)]
pub(crate) struct ContentDispositionResponse {
    pub(crate) version_id: Option<String>,
    materialized: bool,
    inherited: bool,
    own_version_id: Option<String>,
    source_version_id: Option<String>,
    content_state: &'static str,
    visible_content_state: &'static str,
}
impl From<ContentDispositionSnapshot> for ContentDispositionResponse {
    fn from(snapshot: ContentDispositionSnapshot) -> Self {
        let ContentDispositionSnapshot {
            version_id,
            source_version_id,
            stored_state,
        } = snapshot;
        let inherited = source_version_id.is_some();
        let visible_content_state = stored_state.map(|state| state.as_str()).unwrap_or("LIVE");
        Self {
            own_version_id: (!inherited).then(|| version_id.clone()).flatten(),
            materialized: !inherited && version_id.is_some(),
            inherited,
            version_id,
            source_version_id,
            content_state: if inherited {
                "PENDING"
            } else {
                visible_content_state
            },
            visible_content_state,
        }
    }
}
