//! Closed persistent row shapes. External locations are intentionally absent.
use crate::{
    BranchState, ContentState, FileBranchMerge, FileMetadataNode, FileVersion, MergeLink,
    OperationId,
};
use data_serialization::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct FileRow {
    pub container_edit: storage_contract::container_edit::ContainerEdit,
    pub container_tags: storage_contract::file_container_tags::ContainerTags,
    pub file_id: String,
    pub note: Option<String>,
    pub selected_branch_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct VersionRow {
    pub version: FileVersion,
    pub parent_node_ids: Vec<String>,
    pub note: Option<String>,
    pub content_state: ContentState,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct BranchRow {
    pub branch_id: String,
    pub history_tree_id: String,
    pub file_id: String,
    pub name: String,
    pub state: BranchState,
    #[serialization(with = "storage_contract::u64_string")]
    pub generation: u64,
    pub head_node_id: String,
    pub pending_version_id: Option<String>,
    pub alias_of: Option<String>,
}
/// Original audit identity and mapped ownership, never exterior locations.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct PathChangeRow {
    pub event_id: storage_contract::FilePathEventId,
    pub source_file_id: storage_contract::FileId,
    pub source_history_tree_id: storage_contract::HistoryTreeId,
    pub source_branch_id: storage_contract::BranchId,
    pub source_entry_id: storage_contract::EntryId,
    pub source_before_version_id: storage_contract::VersionId,
    pub source_after_version_id: storage_contract::VersionId,
    pub operation_id: OperationId,
    pub author: storage_contract::Principal,
    pub sequence: storage_contract::Sequence,
    pub occurred_at: storage_contract::Timestamp,
    pub file_id: storage_contract::FileId,
    pub history_tree_id: storage_contract::HistoryTreeId,
    pub branch_id: storage_contract::BranchId,
    pub before_version_id: storage_contract::VersionId,
    pub after_version_id: storage_contract::VersionId,
}

impl From<&crate::business::history_copy::retained::RetainedPathChange> for PathChangeRow {
    fn from(value: &crate::business::history_copy::retained::RetainedPathChange) -> Self {
        let source = &value.source;
        Self {
            event_id: source.event_id,
            source_file_id: source.file_id,
            source_history_tree_id: source.history_tree_id,
            source_branch_id: source.branch_id,
            source_entry_id: source.entry_id,
            source_before_version_id: source.before_version_id,
            source_after_version_id: source.after_version_id,
            operation_id: source.operation_id,
            author: source.author.clone(),
            sequence: source.sequence,
            occurred_at: source.occurred_at,
            file_id: value.file_id,
            history_tree_id: value.history_tree_id,
            branch_id: value.branch_id,
            before_version_id: value.before_version_id,
            after_version_id: value.after_version_id,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct MetadataRow {
    pub node: FileMetadataNode,
    pub parent_node_ids: Vec<String>,
    pub note: Option<String>,
}
pub(super) use crate::file_deletion::retained::{DeletionAction, DeletionRow, LifecycleRow};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct LinkRow {
    pub file_id: String,
    pub link: MergeLink,
}

pub(super) struct Validation<'a> {
    file: &'a str,
    history: &'a str,
    sequence: u64,
    branches: BTreeMap<String, BranchRow>,
    path_branches: BTreeSet<storage_contract::BranchId>,
    nodes: BTreeMap<String, u64>,
    file_row: bool,
    selected_branch: Option<String>,
    versions: usize,
    storage_ids: BTreeSet<storage_contract::VersionStorageId>,
    current: Option<DeletionRow>,
    lifecycle: BTreeMap<u64, LifecycleRow>,
}
impl<'a> Validation<'a> {
    pub(super) fn new(file: &'a str, history: &'a str, sequence: u64) -> Self {
        Self {
            file,
            history,
            sequence,
            branches: BTreeMap::new(),
            path_branches: BTreeSet::new(),
            nodes: BTreeMap::new(),
            file_row: false,
            selected_branch: None,
            versions: 0,
            storage_ids: BTreeSet::new(),
            current: None,
            lifecycle: BTreeMap::new(),
        }
    }
    fn scope(&self, history: &str) -> crate::Result<()> {
        if history != self.history {
            return Err(super::invalid(
                "file database row belongs to another history",
            ));
        }
        Ok(())
    }
    fn sequence(&self, sequence: u64) -> crate::Result<()> {
        if sequence == 0 || sequence > self.sequence {
            return Err(super::invalid(
                "file database row is outside its captured sequence",
            ));
        }
        Ok(())
    }
    fn node(&mut self, id: &str, history: &str, sequence: u64) -> crate::Result<()> {
        self.scope(history)?;
        self.sequence(sequence)?;
        if self.nodes.insert(id.to_owned(), sequence).is_some() {
            return Err(super::invalid("file database repeats a graph node"));
        }
        Ok(())
    }
    pub(super) fn admit(&mut self, kind: &str, id: &str, body: &str) -> crate::Result<()> {
        let prefix = match kind {
            "file" => "file",
            "version" => "ver",
            "branch" => "br",
            "path_change" => "fpath",
            "metadata" => "fmeta",
            "deletion" => "fdel",
            "lifecycle" => "op",
            "merge" => "fmerge",
            "merge_link" => "link",
            _ => return Err(super::invalid("file database record kind is unsupported")),
        };
        storage_contract::validate_id(id, prefix)?;
        let (owner, own_id) = match kind {
            "file" => {
                let row: FileRow = decode_local(body.as_bytes())?;
                if let Some(branch) = &row.selected_branch_id {
                    storage_contract::validate_id(branch, "br")?;
                }
                self.file_row = true;
                self.selected_branch = row.selected_branch_id;
                (row.file_id.clone(), row.file_id)
            }
            "version" => {
                let row: VersionRow = decode_local(body.as_bytes())?;
                crate::container_storage::validate_version(&row.version)?;
                if !self.storage_ids.insert(row.version.storage_id) {
                    return Err(crate::Error::VersionNames(
                        crate::container_storage::VersionNameError::Collision,
                    ));
                }
                self.node(
                    &row.version.version_id,
                    &row.version.history_tree_id,
                    row.version.version_sequence,
                )?;
                self.versions += 1;
                (row.version.file_id, row.version.version_id)
            }
            "branch" => {
                let row: BranchRow = decode_local(body.as_bytes())?;
                storage_contract::validate_branch_name(&row.name)?;
                storage_contract::history_window::HistoryNodeId::parse(&row.head_node_id)?;
                self.scope(&row.history_tree_id)?;
                let identity = (row.file_id.clone(), row.branch_id.clone());
                self.branches.insert(row.branch_id.clone(), row);
                identity
            }
            "path_change" => {
                let row: PathChangeRow = decode_local(body.as_bytes())?;
                self.scope(&row.history_tree_id.to_string())?;
                self.sequence(row.sequence.get())?;
                self.path_branches.insert(row.branch_id);
                // Audit endpoints are not graph nodes. A historical pending
                // version may belong to its original source file; the complete
                // authoritative projection verifies that provenance separately.
                (row.file_id.to_string(), row.event_id.to_string())
            }
            "metadata" => {
                let row: MetadataRow = decode_local(body.as_bytes())?;
                self.node(
                    &row.node.node_id,
                    &row.node.history_tree_id,
                    row.node.sequence,
                )?;
                (row.node.file_id, row.node.node_id)
            }
            "deletion" => {
                let row: DeletionRow = decode_local(body.as_bytes())?;
                validate_deletion(&row)?;
                self.scope(&row.history_tree_id)?;
                self.sequence(row.sequence)?;
                let identity = (row.file_id.clone(), row.node_id.clone());
                if self.current.replace(row).is_some() {
                    return Err(super::invalid(
                        "file database has multiple current deletions",
                    ));
                }
                identity
            }
            "lifecycle" => {
                let row: LifecycleRow = decode_local(body.as_bytes())?;
                validate_deletion(&row.node)?;
                self.scope(&row.node.history_tree_id)?;
                self.sequence(row.sequence)?;
                self.sequence(row.node.sequence)?;
                let identity = (row.node.file_id.clone(), row.operation_id.to_string());
                if self.lifecycle.insert(row.sequence, row).is_some() {
                    return Err(super::invalid("file database repeats a lifecycle sequence"));
                }
                identity
            }
            "merge" => {
                let row: FileBranchMerge = decode_local(body.as_bytes())?;
                self.scope(&row.history_tree_id)?;
                self.sequence(row.sequence)?;
                (row.file_id, row.merge_id)
            }
            "merge_link" => {
                let row: LinkRow = decode_local(body.as_bytes())?;
                self.scope(&row.link.history_tree_id)?;
                (row.file_id, row.link.merge_link_id)
            }
            _ => unreachable!("invariant: record kind was matched above"),
        };
        if owner != self.file || own_id != id {
            return Err(super::invalid(
                "file database record belongs to another file or row identity",
            ));
        }
        Ok(())
    }
    pub(super) fn finish(self) -> crate::Result<()> {
        if !self.file_row || self.versions == 0 {
            return Err(super::invalid(
                "file database is missing its file or retained versions",
            ));
        }
        // A retained selection must name this object's own admitted branch.
        // Archived/retired branches remain valid history; this check grants no
        // live branch access and never guesses a replacement selection.
        if let Some(selected) = &self.selected_branch
            && !self.branches.contains_key(selected)
        {
            return Err(super::invalid(
                "file database selection references a missing local branch",
            ));
        }
        for branch in &self.path_branches {
            if !self.branches.contains_key(&branch.to_string()) {
                return Err(crate::persist::path_changes::invalid());
            }
        }
        let heads: BTreeMap<_, _> = self
            .branches
            .values()
            .map(|branch| (branch.branch_id.clone(), branch.head_node_id.clone()))
            .collect();
        for head in heads.values() {
            if !self.nodes.contains_key(head) {
                return Err(super::invalid(
                    "file database branch references a missing local head",
                ));
            }
        }
        if let Some(current) = &self.current
            && current.branch_heads != heads
        {
            return Err(super::invalid(
                "file database current deletion omits or changes a retained branch head",
            ));
        }
        let mut active = None;
        let mut nodes = BTreeSet::new();
        for row in self.lifecycle.into_values() {
            // Old D frontiers are historical snapshots, not today's HEADs. They
            // must still name retained local endpoints no newer than that D.
            for (branch, head) in &row.node.branch_heads {
                if !self.branches.contains_key(branch)
                    || !self
                        .nodes
                        .get(head)
                        .is_some_and(|sequence| *sequence <= row.node.sequence)
                {
                    return Err(super::invalid(
                        "file database historical deletion references a missing or later endpoint",
                    ));
                }
            }
            match row.action {
                DeletionAction::Delete => {
                    if active.is_some()
                        || !nodes.insert(row.node.node_id.clone())
                        || row.node.operation_id != row.operation_id
                        || row.node.sequence != row.sequence
                        || row.node.author != row.author
                    {
                        return Err(super::invalid(
                            "file database deletion history is inconsistent",
                        ));
                    }
                    active = Some(row.node);
                }
                DeletionAction::Import => {
                    let previous = active.as_ref().ok_or_else(|| {
                        super::invalid("imported deletion has no retained active source")
                    })?;
                    if previous.file_id != row.node.file_id
                        || previous.history_tree_id != row.node.history_tree_id
                        || previous.branch_heads != row.node.branch_heads
                        || previous.parent_node_ids != row.node.parent_node_ids
                        || row.node.operation_id != row.operation_id
                        || row.node.sequence != row.sequence
                        || row.node.author != row.author
                        || previous.sequence >= row.sequence
                        || !nodes.insert(row.node.node_id.clone())
                    {
                        return Err(super::invalid(
                            "imported deletion changes its retained frontier",
                        ));
                    }
                    active = Some(row.node);
                }
                DeletionAction::Undo => {
                    if active.as_ref() != Some(&row.node) || row.node.sequence >= row.sequence {
                        return Err(super::invalid(
                            "file database undo does not consume its exact deletion",
                        ));
                    }
                    active = None;
                }
            }
        }
        if active != self.current {
            return Err(super::invalid(
                "file database current deletion differs from its retained lifecycle",
            ));
        }
        Ok(())
    }
}
fn validate_deletion(row: &DeletionRow) -> crate::Result<()> {
    use storage_contract::history_window::HistoryNodeId;
    storage_contract::validate_id(&row.node_id, "fdel")?;
    storage_contract::validate_id(&row.history_tree_id, "history")?;
    if row.branch_heads.is_empty() {
        return Err(super::invalid("file database deletion has no frontier"));
    }
    let mut heads = BTreeSet::new();
    for (branch, node) in &row.branch_heads {
        storage_contract::validate_id(branch, "br")?;
        HistoryNodeId::parse(node)?;
        heads.insert(node.clone());
    }
    if heads.into_iter().collect::<Vec<_>>() != row.parent_node_ids {
        return Err(super::invalid(
            "file database deletion parents differ from its stored frontier",
        ));
    }
    Ok(())
}

fn decode_local<T: for<'de> Deserialize<'de> + Serialize>(bytes: &[u8]) -> crate::Result<T> {
    let row: T = storage_contract::decode_strict(bytes)?;
    if crate::disk::encode_metadata(&row, super::projection::MAX_RECORD_BYTES)? != bytes {
        return Err(super::invalid(
            "file database row is not canonical typed metadata",
        ));
    }
    Ok(row)
}
