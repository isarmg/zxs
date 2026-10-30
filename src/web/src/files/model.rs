use super::{
    history::{FileBranch, FileHistory},
    page::{DirectoryBranch, Entry},
};
use json_codec::Value;
use std::{collections::BTreeSet, sync::Arc};
use web_interface::i18n::t;

/// The file-manager disk contract is distinct from monitor/topology records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Disk {
    pub slot_id: String,
    pub online: bool,
    pub writable: bool,
    pub available_bytes: u64,
}

pub(crate) fn parse_disks(value: &Value) -> Result<Vec<Disk>, crate::api::Error> {
    let parsed = value
        .get("disks")
        .and_then(Value::as_array)
        .and_then(|disks| {
            disks
                .iter()
                .map(|disk| {
                    Some(Disk {
                        slot_id: disk
                            .get("slot_id")?
                            .as_str()
                            .filter(|id| !id.is_empty())?
                            .to_owned(),
                        online: disk.get("online")?.as_bool()?,
                        writable: disk.get("writable")?.as_bool()?,
                        available_bytes: crate::format::parse_u64(disk.get("available_bytes")?)?,
                    })
                })
                .collect::<Option<Vec<_>>>()
        });
    parsed.ok_or_else(|| crate::api::Error {
        status: 200,
        code: storage_contract::ErrorCode::InvalidDiskList,
        message: "The disk listing did not match the server contract.".into(),
        retryable: false,
        request_id: None,
        source: None,
    })
}

/// Owned destinations supplied by disk rows, directory rows or breadcrumbs.
/// Directory navigation keeps the currently selected disk.
pub(crate) enum Navigation {
    Root,
    Disk(String),
    Directory(String),
}

#[derive(Clone, Default)]
pub(crate) struct Model {
    pub disks: Vec<Disk>,
    pub disks_loaded: bool,
    pub loading_disks: bool,
    pub disks_generation: u64,
    pub slot: String,
    pub directory: String,
    pub writable: bool,
    /// One bounded server page, shared by listing rows.
    pub entries: Arc<[Entry]>,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub previous_cursor: Option<String>,
    pub total_entries: u64,
    pub listing_failed: bool,
    pub listing_cancellation: Option<crate::api::ReadCancellation>,
    pub search_delay: Option<Arc<send_wrapper::SendWrapper<gloo_timers::callback::Timeout>>>,
    pub directory_branches: Vec<DirectoryBranch>,
    pub directory_file_count: u64,
    pub directory_branches_supported: bool,
    pub repository_id: String,
    pub repository_path: String,
    pub default_branch_scope: bool,
    pub loaded: bool,
    pub loading: bool,
    pub generation: u64,
    pub selected: Option<Entry>,
    pub file_history: crate::snapshot::Snapshot<FileHistory>,
    pub history_loading: bool,
    pub history_generation: u64,
    pub history_cancellation: Option<crate::api::ReadCancellation>,
    pub history_editors: usize,
    pub history_aux: super::history::Reads,
    pub checked_entries: BTreeSet<String>,
    pub batch: Option<super::batch::Draft>,
    pub merge: Option<super::merge::Draft>,
    pub backup: Option<super::backup::Draft>,
    pub tag_edit: Option<super::tags::Draft>,
    pub checked_nodes: super::tags::Selection,
    pub publications: super::review::Publications,
    pub operations: Vec<FileOperation>,
    pub polling_operations: Vec<String>,
    pub mutation_pending: bool,
    pub query: String,
}
impl Model {
    /// Reset location-owned state without discarding disk reads or operation
    /// recovery. Session admission and starting the next read belong to actions.
    pub(crate) fn navigate(&mut self, destination: Navigation) {
        self.cancel_listing();
        self.reset_page();
        match destination {
            Navigation::Root => {
                self.slot.clear();
                self.directory.clear();
                // The disk view does not consume the directory query/readiness.
                // Opening a disk establishes them for the next directory read.
            }
            Navigation::Disk(slot) => {
                self.slot = slot;
                self.directory.clear();
                self.query.clear();
                self.loaded = false;
            }
            Navigation::Directory(directory) => {
                self.directory = directory;
                self.query.clear();
                self.loaded = false;
            }
        }
        self.writable = false;
        self.entries = Default::default();
        self.directory_branches.clear();
        self.directory_file_count = 0;
        self.directory_branches_supported = false;
        self.repository_id.clear();
        self.repository_path.clear();
        self.default_branch_scope = false;
        self.clear_selection();
        self.batch = None;
    }

    /// A recovered store needs a fresh disk snapshot, even if an older read is
    /// still in flight. Only this generation may mark the listing loaded again.
    pub(crate) fn invalidate_disks(&mut self) {
        self.disks_generation += 1;
        self.disks_loaded = false;
    }

    /// Page-local selection is never carried to unloaded entries.
    pub(crate) fn cancel_listing(&mut self) {
        self.search_delay = None;
        if let Some(cancellation) = self.listing_cancellation.take() {
            cancellation.abort();
        }
        self.generation += 1;
        self.loading = false;
    }

    pub(crate) fn reset_page(&mut self) {
        self.cursor = None;
        self.next_cursor = None;
        self.previous_cursor = None;
        self.total_entries = 0;
        self.checked_entries.clear();
        self.listing_failed = false;
    }

    pub(crate) fn clear_selection(&mut self) {
        self.cancel_history();
        self.merge = None;
        self.backup = None;
        self.tag_edit = None;
        self.checked_nodes.clear();
        self.selected = None;
        self.file_history = Default::default();
    }

    pub(crate) fn cancel_history(&mut self) {
        self.history_aux.cancel_all();
        if let Some(cancellation) = self.history_cancellation.take() {
            cancellation.abort();
        }
        self.history_loading = false;
        self.history_generation += 1;
    }

    pub(crate) fn prune_checked_entries(&mut self) {
        let visible = self.entries.iter().map(entry_key).collect::<BTreeSet<_>>();
        self.checked_entries.retain(|key| visible.contains(key));
    }

    pub(crate) fn reconcile_selection(&mut self, entry: Option<Entry>) {
        self.prune_checked_entries();
        let Some(selected) = self.selected.as_ref() else {
            return;
        };
        if let Some(entry) = entry {
            let context_changed = selected.file_id() != entry.file_id()
                || selected.path() != entry.path()
                || selected.context() != entry.context()
                || selected.backup() != entry.backup();
            let content_changed = selected.version_id() != entry.version_id()
                || selected.node_id() != entry.node_id()
                || selected.selected_branch_id() != entry.selected_branch_id();
            if context_changed {
                self.clear_selection();
            } else if content_changed {
                self.cancel_history();
                self.file_history.invalidate();
                // Selections and action drafts retain their captured heads.
                // A new root requires a complete explicit review before reuse.
            }
            self.selected = Some(entry);
        } else {
            self.clear_selection();
        }
    }
}
#[derive(Clone, PartialEq)]
pub(crate) enum NamespaceAction {
    CreateDirectory,
}
pub(crate) use storage_contract::client_protocol::OperationStatus as SaveStatus;

/// An operation observation, not a receipt or an authorization grant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileOperation {
    id: String,
    status: SaveStatus,
}
impl FileOperation {
    pub(crate) fn from_receipt(id: &str, value: &Value) -> Self {
        if value["operation_id"].as_str() == Some(id)
            && let Some(status) = storage_contract::client_protocol::operation_status(value)
        {
            Self {
                id: id.into(),
                status,
            }
        } else {
            // A malformed or unrelated response cannot settle the original write.
            Self::unknown(id)
        }
    }

    pub(crate) fn unknown(id: &str) -> Self {
        Self {
            id: id.into(),
            status: SaveStatus::Unknown,
        }
    }

    pub(crate) fn rejected(id: &str) -> Self {
        Self {
            id: id.into(),
            status: SaveStatus::Rejected,
        }
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn status(&self) -> SaveStatus {
        self.status
    }
}
pub(crate) fn publication_notice(status: SaveStatus) -> String {
    match status {
        SaveStatus::Published => t("更改已保存。", "Changes saved."),
        SaveStatus::Degraded => t(
            "文件已发布，但发布时保护不足；永久损失的内容无法恢复。请检查硬盘状态。",
            "The file was published with reduced protection. Permanently lost content cannot be recovered. Check disk status.",
        ),
        SaveStatus::Rejected => t(
            "保存未完成，服务端暂存内容已保留。",
            "The save did not complete. Staged content was retained on the server.",
        ),
        SaveStatus::Unknown => t(
            "保存结果尚未确认，正在查询原操作。请勿重复提交。",
            "The save is not yet confirmed. The original operation is being checked; do not submit it again.",
        ),
    }
}
pub(crate) fn join_path(directory: &str, name: &str) -> String {
    if directory.is_empty() {
        name.to_owned()
    } else {
        format!("{directory}/{name}")
    }
}
pub(crate) fn breadcrumbs(path: &str) -> Vec<(String, String)> {
    let mut result = Vec::new();
    let mut full = String::new();
    for name in path.split('/').filter(|part| !part.is_empty()) {
        full = join_path(&full, name);
        result.push((name.to_owned(), full.clone()));
    }
    result
}
pub(crate) fn valid_name(name: &str) -> bool {
    name_policy::validate_name(name).is_ok()
}
pub(crate) fn file_kind(entry: &Entry) -> bool {
    entry.kind() == storage_contract::NamespaceEntryKind::File
}
/// A stable file ID can appear at several frozen logical paths. History and
/// branch selection must carry the view path, independently of physical owner.
pub(crate) fn entry_key(entry: &Entry) -> String {
    format!("{}\0{}", entry.entry_id(), entry.path())
}
pub(crate) fn file_context(entry: &Entry) -> Option<(String, String)> {
    entry
        .context()
        .map(|(repository, path)| (repository.to_owned(), path.to_owned()))
}

pub(crate) fn selected_entry_read_only(model: &Model) -> bool {
    model.selected.as_ref().is_some_and(Entry::read_only)
}

pub(crate) fn selected_branch(model: &Model) -> String {
    model
        .file_history
        .selected()
        .or_else(|| model.selected.as_ref().and_then(Entry::selected_branch_id))
        .unwrap_or_default()
        .to_owned()
}
pub(crate) fn selected_file_branch_record(model: &Model) -> Option<(String, String, String)> {
    let file = model
        .selected
        .as_ref()
        .filter(|entry| file_kind(entry))
        .map(Entry::file_id)?;
    let id = selected_branch(model);
    let branch = model
        .file_history
        .branches()
        .iter()
        .find(|branch| branch.id() == id)
        .or_else(|| model.file_history.current_branch().filter(|branch| branch.id() == id))?;
    if !branch.writable() {
        return None;
    }
    Some((file.to_owned(), id, branch.name().to_owned()))
}
pub(crate) fn directory_branch_name(branch: &DirectoryBranch) -> String {
    display_branch_name(branch.name())
}
fn display_branch_name(name: &str) -> String {
    match name {
        "main" => t("主版本", "Main"),
        "" => t("未命名分支", "Unnamed branch"),
        name => name.to_owned(),
    }
}
pub(crate) fn branch_name(branch: &FileBranch) -> String {
    let name = display_branch_name(branch.name());
    if branch.pending() {
        format!("{name} · {}", t("待写入", "Pending content"))
    } else {
        name
    }
}
pub(crate) fn directory_branch_value(branch: &DirectoryBranch, default_scope: bool) -> String {
    if default_scope {
        branch.id()
    } else {
        branch.name()
    }
    .to_owned()
}
pub(crate) fn selected_directory_branch(model: &Model) -> Option<(String, String, String)> {
    let branch = model
        .directory_branches
        .iter()
        .find(|branch| branch.current())?;
    Some((
        model.repository_id.clone(),
        branch.id().to_owned(),
        branch.name().to_owned(),
    ))
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/model/tests.rs"]
mod tests;
